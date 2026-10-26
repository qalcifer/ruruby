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
#       Button(text: "+1") { on_click { count += 1 } }
#     }
#   }
#
# The renderer (to come) will be Rust, reading this tree; until then
# `dump` shows it and `emit` plays events.

module UI {
  class Element {
    prop mut id = none

    # the elements under this one: an `if`/`for` part counts as what it holds
    fn children() = __ui_raw.flat_map(|c| if c.is_a?(Fragment) { c.children } else { [c] })
    fn parent() = @__parent
    fn __ui_raw() = @__children || []

    # adds a child (the code a component's tree compiles to calls it); not
    # a dependency of the effect building the tree (which would re-run it)
    fn __ui_add(child) {
      @__children = untracked { __ui_raw } + [child]
      child.__ui_set_parent(self)
      child
    }
    fn __ui_set_parent(p) = @__parent = p

    # `on_click { }` in a tree; `on(:click) { }` in code
    fn on(event, &handler) {
      @__handlers = {} unless @__handlers
      @__handlers[event] = (@__handlers[event] || []) + [handler]
      self
    }

    # what a renderer does on a click: the handlers run in one batch
    fn emit(event, *args) {
      handlers = (@__handlers || {})[event] || []
      batch { handlers.each { |h| h.call(*args) } }
      handlers.size
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
    fn __ui_clear() = @__children = []
  }

  # what `component Name { }` makes: its props are its state, its tree its
  # children
  class Component < Element { }

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
  }
  class CheckBox < Element {
    prop mut checked = false
    prop mut text: String = ""
  }
}
