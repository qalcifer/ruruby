# The declarative UI's elements (lang-fork-ui), without a renderer yet:
# a tree of objects whose props bindings keep up to date. A program imports
# them with `use ui::{Column, Text, Button}` (the built-in module `ui`).
#
#   use ui::{Column, Text, Button}
#
#   component Counter {
#     prop mut count = 0
#     Column(spacing: 8) {
#       Text(text: "Count: #{count}")          # a binding: follows count
#       Button(text: "+1") { on(:click) { count += 1 } }
#     }
#   }
#
# The renderer (to come) will be Rust, reading this tree; until then
# `dump` shows it and `emit` plays events.

module UI {
  # The tree's changes for a renderer since the last call: [:create, id,
  # kind], [:set, id, prop, value], [:insert, parent, index, child],
  # [:remove, parent, child] (src/ui.rs; Rust: Interpreter::take_ui_mutations).
  fn mutations() = __ui_mutations

  # What happens to an element, given to its handlers: `on(:click) { |e| e.x }`
  # (or `on(Event::Click)`). The renderer (Rust) makes them.
  enum Event {
    Click { x, y, button }
    PointerMove { x, y }
    Scroll { dx, dy }
    Key { key, mods }
    Text(text)
    Submit
    Focus(focused)
  }

  class Element {
    prop mut id = none

    # the events an element of this type emits (a subclass adds its own)
    EVENTS = [:Click, :PointerMove, :Scroll, :Key, :Focus]
    fn self.events() = self::EVENTS

    # the elements under this one: an `if`/`for` part counts as what it holds
    fn children() = __ui_raw.flat_map(|c| if c.is_a?(Fragment) { c.children } else { [c] })
    fn parent() = @__parent
    fn __ui_raw() = @__children || []

    # adds a child (the code a component's tree compiles to calls it); not
    # a dependency of the effect building the tree (which would re-run it)
    fn __ui_add(child) {
      @__children = untracked { __ui_raw } + [child]
      child.__ui_set_parent(self)
      untracked { __ui_attached(child) }
      child
    }

    # -- the renderer's view (src/ui.rs) --

    # an if/for part or a component: not a node of its own for a renderer
    fn __ui_transparent?() = false

    # the nodes under this one a renderer sees (through transparent ones)
    fn host_children() = __ui_raw.flat_map(|c| if c.__ui_transparent? { c.host_children } else { [c] })

    # the node a renderer puts this one in: the nearest visible ancestor,
    # or the top of the tree
    fn host_parent() {
      p = parent
      while p && p.__ui_transparent? && p.parent { p = p.parent }
      p
    }

    # whether a renderer has this node: announced, or the top of a tree made
    # with `new` (a component there is a Root)
    fn __ui_shown?() {
      return true if __ui_log_node(self)
      return false if parent || @__member
      if __ui_transparent? { __ui_log_create(self, "Root", []) } else { __ui_announce }
      true
    }

    # the child's nodes go to the renderer, if this part is shown
    fn __ui_attached(child) {
      host = if __ui_transparent? { host_parent || self } else { self }
      return none unless host.__ui_shown?
      nodes = if child.__ui_transparent? { child.host_children } else { [child] }
      siblings = host.host_children
      nodes.each { |n|
        n.__ui_announce
        __ui_log_insert(host, siblings.index(n), n)
      }
      none
    }

    # Create, its props, and its children (once)
    fn __ui_announce() {
      return none if __ui_log_node(self)
      props = self.class.props.filter_map(|n| if n == :id && id.none? { none } else { [n, send(n)] })
      __ui_log_create(self, kind, props)
      host_children.each_with_index { |c, i|
        c.__ui_announce
        __ui_log_insert(self, i, c)
      }
      none
    }

    # the tree as a renderer has it (no components or if/for parts)
    fn host_dump(indent = 0) {
      if __ui_transparent? {
        return host_children.map(|c| c.host_dump(indent)).join("\n")
      }
      shown = self.class.props.filter_map(|name| {
        v = send(name)
        if name == :id && v.none? { none } else { "#{name}: #{v.inspect}" }
      })
      line = "  " * indent + kind + "(#{shown.join(", ")})"
      ([line] + host_children.map(|c| c.host_dump(indent + 1))).join("\n")
    }

    # this node and what it shows aren't nodes any more
    fn __ui_forget() {
      __ui_log_forget(self)
      host_children.each(&:__ui_forget)
    }
    fn __ui_set_parent(p) = @__parent = p

    # `on(:click) { |e| ... }`, `on(:pointer_move)`, `on(Event::Click)`:
    # the handler gets the Event
    fn on(event, &handler) {
      name = __ui_event(event)
      @__handlers = {} unless @__handlers
      @__handlers[name] = (@__handlers[name] || []) + [handler]
      self
    }

    # What a renderer does when something happens: the element's own
    # behaviour (a TextInput takes the text), then the handlers, in one
    # batch. `emit(:click)` makes a plain Event::Click.
    fn emit(event) {
      event = __ui_plain_event(__ui_event(event)) unless event.is_a?(Event)
      name = __ui_event(event)
      handlers = (@__handlers || {})[name] || []
      batch {
        __ui_default(event)
        handlers.each { |h| h.call(event) }
      }
      handlers.size
    }

    # an event from a renderer (Interpreter::ui_event): (name, fields)
    fn __ui_host_event(name, f) {
      event = case name {
        when :click { Event::Click(x: f[0], y: f[1], button: f[2]) }
        when :pointer_move { Event::PointerMove(x: f[0], y: f[1]) }
        when :scroll { Event::Scroll(dx: f[0], dy: f[1]) }
        when :key { Event::Key(key: f[0], mods: f[1]) }
        when :text { Event::Text(f[0]) }
        when :submit { Event::Submit }
        when :focus { Event::Focus(f[0]) }
      }
      emit(event)
    }

    # the built-in reaction to an event (none)
    fn __ui_default(_event) = none

    # the variant an event, a variant or a name stands for, if this element emits it
    fn __ui_event(event) {
      name = if event.is_a?(Event) {
        event.variant
      } else if event.is_a?(Class) && event < Event {
        event.name.split("::").last.to_sym
      } else if event.is_a?(Symbol) {
        s = event.to_s
        if s[0] == s[0].upcase { event } else { s.split("_").map(&:capitalize).join.to_sym }
      } else {
        panic("on/emit: #{event.inspect} isn't an event: write on(:click) or on(Event::Click)")
      }
      unless self.class.events.include?(name) {
        panic("#{kind} doesn't emit #{name} (it emits #{self.class.events.join(", ")})")
      }
      name
    }

    fn __ui_plain_event(name) = case name {
      when :Click { Event::Click(x: 0, y: 0, button: :left) }
      when :PointerMove { Event::PointerMove(x: 0, y: 0) }
      when :Scroll { Event::Scroll(dx: 0, dy: 0) }
      when :Key { Event::Key(key: :none, mods: []) }
      when :Text { Event::Text("") }
      when :Submit { Event::Submit }
      when :Focus { Event::Focus(true) }
    }

    # the first element (this one, or below) with the id
    fn find(id) {
      return self if self.id == id
      children.each { |c|
        found = c.find(id)
        return found if found
      }
      none
    }

    fn kind() = self.class.name.split("::").last

    # the tree, one element a line: `Text(text: "Count: 0")`
    fn dump(indent = 0) {
      shown = self.class.props.filter_map(|name| {
        v = send(name)
        if name == :id && v.none? { none } else { "#{name}: #{v.inspect}" }
      })
      line = "  " * indent + kind + (if shown.empty? { "" } else { "(#{shown.join(", ")})" })
      ([line] + children.map(|c| c.dump(indent + 1))).join("\n")
    }
  }

  # The part of a tree an `if` or `for` makes: rebuilt when what it depends
  # on changes; transparent (its children count as its parent's).
  class Fragment < Element {
    fn __ui_transparent?() = true

    # before a rebuild: what it showed leaves the renderer's tree
    fn __ui_clear() {
      untracked {
        host = host_parent
        if host && __ui_log_node(host) {
          host_children.each { |c|
            __ui_log_remove(host, c)
            c.__ui_forget
          }
        }
      }
      @__children = []
    }
  }

  # what `component Name { }` makes: its props are its state, its tree its
  # children
  class Component < Element {
    fn __ui_transparent?() = true
  }

  class Window < Element {
    prop mut title: String = ""
    prop mut width: Integer = 640
    prop mut height: Integer = 480
  }
  class Column < Element {
    prop mut spacing: Integer = 0
    prop mut padding: Integer = 0
  }
  class Row < Element {
    prop mut spacing: Integer = 0
    prop mut padding: Integer = 0
  }
  class Text < Element {
    prop mut text: String = ""
    prop mut color: Symbol = :black
  }
  class Button < Element {
    prop mut text: String = ""
    prop mut enabled = true
  }
  class TextInput < Element {
    prop mut text: String = ""
    prop mut placeholder: String = ""
    EVENTS = Element::EVENTS + [:Text, :Submit]
    fn __ui_default(event) = case event {
      in Event::Text(t) { self.text = t }
      else { none }
    }
  }
  class CheckBox < Element {
    prop mut checked = false
    prop mut text: String = ""
    fn __ui_default(event) = case event {
      in Event::Click { self.checked = !checked }
      else { none }
    }
  }
}
