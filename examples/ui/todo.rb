# A todo list in a window: cargo run --release -p ruruby-egui -- examples/ui/todo.rb
use ui::{Window, Column, Row, Text, Button, CheckBox, TextInput}

class Todo {
  prop title
  prop mut done = false
}

component TodoRow {
  prop todo
  prop on_delete                         # a closure from the parent
  Row(spacing: 6) {
    CheckBox(checked <=> todo.done)
    Text(text: todo.title, color: if todo.done { :gray } else { :black })
    Button(text: "x") { on(:click) { on_delete.call(todo) } }
  }
}

component App {
  prop mut todos = [Todo::new(title: "try ruruby-ui")]
  prop mut draft = ""

  fn add() {
    return none if draft.strip.empty?
    todos = todos + [Todo::new(title: draft.strip)]
    draft = ""
  }
  fn remaining() = todos.count(|t| !t.done)

  Window(title: "Todos", width: 360, height: 420) {
    Column(spacing: 8, padding: 8) {
      Row(spacing: 6) {
        TextInput(id: :input, text <=> draft, placeholder: "What needs doing?") { on(:submit) { add } }
        Button(id: :add, text: "Add", enabled: !draft.strip.empty?) { on(:click) { add } }
      }
      for t in todos {
        TodoRow(todo: t, on_delete: |gone| { todos = todos.reject(|x| x.equal?(gone)) })
      }
      if todos.empty? {
        Text(text: "Nothing to do", color: :gray)
      } else {
        Text(text: "#{remaining} of #{todos.size} left")
      }
    }
  }
}
