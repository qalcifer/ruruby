//! lang-fork-ui: components and element trees (src/core/ui.rb, src/core/ui.rs,
//! Parser::parse_component).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const COUNTER: &str = r##"
    use ui::{Column, Row, Text, Button, TextInput}

    component Counter {
      prop mut count = 0
      prop mut draft = ""
      fn label() = "Count: #{count}"

      Column(spacing: 8) {
        Text(id: :label, text: label)
        Row {
          Button(id: :minus, text: "-1", enabled: count > 0) { on_click { count -= 1 } }
          Button(id: :plus, text: "+1") { on_click { count += 1 } }
        }
        TextInput(id: :input, text <=> draft, placeholder: "type")
        Text(id: :echo, text: "draft: #{draft}")
      }
    }
"##;

/// A tree is built once per component; arguments are bindings (literals
/// set once); handlers change state with bare prop writes; `<=>` binds
/// both ways.
#[test]
fn counter() {
    let out = run(&format!("{COUNTER}{}", r##"
        c = Counter::new
        puts c.dump
        c.find(:plus).emit(:click)
        c.find(:plus).emit(:click)
        c.find(:minus).emit(:click)
        c.find(:input).text = "hello"
        p c.count, c.draft, c.find(:label).text, c.find(:minus).enabled, c.find(:echo).text
        c.draft = "back"
        p c.find(:input).text, c.find(:echo).text, c.find(:plus).parent.kind, c.children.size
        p Counter::new(count: 5).find(:label).text
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Counter(count: 0, draft: "")"#,
        "  Column(spacing: 8, padding: 0)",
        r#"    Text(id: :label, text: "Count: 0", color: :black)"#,
        "    Row(spacing: 0, padding: 0)",
        r#"      Button(id: :minus, text: "-1", enabled: false)"#,
        r#"      Button(id: :plus, text: "+1", enabled: true)"#,
        r#"    TextInput(id: :input, text: "", placeholder: "type")"#,
        r#"    Text(id: :echo, text: "draft: ", color: :black)"#,
        "1",
        r#""hello""#,
        r#""Count: 1""#,
        "true",
        r#""draft: hello""#,
        r#""back""#,
        r#""draft: back""#,
        r#""Row""#,
        "1",
        r#""Count: 5""#,
    ]);
}

const TODOS: &str = r##"
    use ui::{Column, Row, Text, Button, CheckBox, TextInput}

    class Todo {
      prop title
      prop mut done = false
    }

    component TodoRow {
      prop todo
      Row {
        CheckBox(checked <=> todo.done)
        Text(text: todo.title, color: if todo.done { :gray } else { :black })
      }
    }

    component TodoApp {
      prop mut todos = []
      prop mut draft = ""
      fn remaining() = todos.count(|t| !t.done)

      Column {
        Row {
          TextInput(id: :input, text <=> draft)
          Button(id: :add, text: "Add", enabled: !draft.empty?) {
            on_click {
              todos = todos + [Todo::new(title: draft)]
              draft = ""
            }
          }
        }
        for t in todos {
          TodoRow(todo: t)
        }
        if todos.empty? {
          Text(text: "Nothing to do")
        } else {
          Text(id: :left, text: "#{remaining} left")
        }
      }
    }
"##;

/// `for` repeats elements per item (each with its own item), `if`
/// switches them; both rebuild when what they read changes, stopping the
/// replaced bindings; components nest, taking props.
#[test]
fn todos() {
    let out = run(&format!("{TODOS}{}", r##"
        app = TodoApp::new
        p app.find(:left), app.dump.include?("Nothing to do")
        ["milk", "eggs"].each { |t|
          app.find(:input).text = t
          app.find(:add).emit(:click)
        }
        p app.find(:left).text, app.draft, app.find(:add).enabled
        rows = app.children[0].children.filter(|c| c.kind == "TodoRow")
        p rows.map(|r| r.todo.title)
        app.todos[0].done = true
        p app.find(:left).text, app.dump.include?("Text(text: \"milk\", color: :gray)")
        p Reactive::stats
        app.todos = []
        p app.find(:left), app.dump.include?("Nothing to do"), Reactive::stats
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "none",
        "true",
        r#""2 left""#,
        r#""""#,
        "false",
        r#"["milk", "eggs"]"#,
        r#""1 left""#,
        "true",
        "[16, 17]",
        "none",
        "true",
        "[5, 4]",
    ]);
}

/// A checkbox's change goes back to the model through `<=>`.
#[test]
fn two_way_to_a_model() {
    let out = run(&format!("{TODOS}{}", r##"
        app = TodoApp::new(todos: [Todo::new(title: "a"), Todo::new(title: "b")])
        row = app.children[0].children.filter(|c| c.kind == "TodoRow")[1]
        box = row.children[0].children[0]
        box.checked = true
        p app.todos.map(&:done), app.find(:left).text
    "##));
    assert_eq!(out, "[false, true]\n\"1 left\"\n");
}

#[test]
fn errors() {
    let base = "use ui::{Text, Column}\n";
    let cases = [
        ("component C { Integer(x: 1) }\nC::new", "Integer isn't an element"),
        ("component C { Text(\"x\") }", "arguments are `prop: value` or `prop <=> target`"),
        ("component C { Text(text <=> 1) }", "`text <=> ...` needs a prop to bind to"),
        ("component C { Column { |x| Text(text: x) } }", "an element's block takes no parameters"),
        ("component C { Text(nope: 1) }\nC::new", "undefined method 'nope='"),
        ("component C { prop mut n = 0\n Text(text: n) }\nC::new", "UI::Text#text"),
    ];
    for (src, want) in cases {
        let err = run_capture(&format!("{base}{src}")).map(|o| format!("no error: {o}")).unwrap_or_else(|e| e.to_string());
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
}
