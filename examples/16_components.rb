# 16 — Components: a declarative UI tree
#
# A component is a class with props and an element tree. The tree is built
# once; each element argument is a binding (see 15_reactive.rb), so the tree
# follows the state by itself. There's no renderer yet (it will be Rust):
# `dump` prints the tree and `emit(:click)` plays what a click would do.

use ui::{Column, Row, Text, Button, CheckBox, TextInput}

class Todo {
  prop title
  prop mut done = false
}

# A component used inside another takes its props as arguments
component TodoRow {
  prop todo

  Row(spacing: 4) {
    CheckBox(checked <=> todo.done)          # two-way: the box and the model
    Text(text: todo.title, color: if todo.done { :gray } else { :black })
  }
}

component TodoApp {
  prop mut todos = []
  prop mut draft = ""

  fn remaining() = todos.count(|t| !t.done)

  Column(spacing: 8) {
    Row {
      TextInput(id: :input, text <=> draft, placeholder: "What needs doing?")
      Button(id: :add, text: "Add", enabled: !draft.empty?) {
        on(:click) {                           # a handler; props are written bare
          todos = todos + [Todo::new(title: draft)]
          draft = ""
        }
      }
    }
    for todo in todos {                      # one row per todo, rebuilt when todos changes
      TodoRow(todo: todo)
    }
    if todos.empty? {                        # switches when the condition changes
      Text(text: "Nothing to do")
    } else {
      Text(id: :left, text: "#{remaining} left")
    }
  }
}

app = TodoApp::new
puts app.dump

# what a user would do: type, click
["buy milk", "write docs"].each { |title|
  app.find(:input).text = title
  app.find(:add).emit(:click)
}
puts "", app.dump

# change the model: the tree follows
app.todos[0].done = true
puts "", app.dump
p app.find(:left).text

# What a renderer receives (Rust: Interpreter::take_ui_mutations): the
# changes since last time, for the nodes it shows — components and if/for
# parts are transparent, so it only sees Column, Row, Text, ...
UI::mutations                      # everything so far: as if applied
app.todos[1].done = true
UI::mutations.each { |m| p m }
puts app.host_dump                  # the tree as the renderer has it
