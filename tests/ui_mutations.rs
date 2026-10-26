//! lang-fork-ui: the element tree's changes for a renderer (src/ui.rs):
//! applied to a mirror tree, as a backend would, they must give the tree
//! the script has.

use ruruby::Interpreter;
use ruruby::ui::{Mutation, PropValue};
use std::collections::HashMap;

/// A node: kind, props, children.
type Node = (String, Vec<(String, PropValue)>, Vec<u64>);

/// What a backend keeps: nodes with props and children.
#[derive(Default)]
struct Mirror {
    nodes: HashMap<u64, Node>,
}

impl Mirror {
    fn apply(&mut self, ms: Vec<Mutation>) {
        for m in ms {
            match m {
                Mutation::Create { id, kind } => {
                    assert!(self.nodes.insert(id, (kind, vec![], vec![])).is_none(), "created twice: {id}");
                }
                Mutation::Set { id, prop, value } => {
                    let props = &mut self.nodes.get_mut(&id).expect("set on a node").1;
                    match props.iter_mut().find(|(p, _)| *p == prop) {
                        Some(slot) => slot.1 = value,
                        None => props.push((prop, value)),
                    }
                }
                Mutation::Insert { parent, index, child } => {
                    assert!(self.nodes.contains_key(&child), "insert of an unknown node");
                    let kids = &mut self.nodes.get_mut(&parent).expect("insert into a node").2;
                    assert!(index <= kids.len(), "index {index} past {} children", kids.len());
                    kids.insert(index, child);
                }
                Mutation::Remove { parent, child } => {
                    let kids = &mut self.nodes.get_mut(&parent).expect("remove from a node").2;
                    let at = kids.iter().position(|c| *c == child).expect("remove of a child");
                    kids.remove(at);
                    self.drop_subtree(child);
                }
            }
        }
    }

    fn drop_subtree(&mut self, id: u64) {
        if let Some((_, _, kids)) = self.nodes.remove(&id) {
            for k in kids {
                self.drop_subtree(k);
            }
        }
    }

    /// The tree under the root, one node a line, like Element#dump's
    /// visible part.
    fn render(&self) -> String {
        let root = self.nodes.iter().find(|(_, n)| n.0 == "Root").map(|(id, _)| *id).expect("a Root");
        let mut out = vec![];
        for k in &self.nodes[&root].2 {
            self.render_node(*k, 0, &mut out);
        }
        out.join("\n")
    }

    fn render_node(&self, id: u64, depth: usize, out: &mut Vec<String>) {
        let (kind, props, kids) = &self.nodes[&id];
        let shown: Vec<String> = props.iter().map(|(p, v)| format!("{p}: {}", show(v))).collect();
        out.push(format!("{}{kind}({})", "  ".repeat(depth), shown.join(", ")));
        for k in kids {
            self.render_node(*k, depth + 1, out);
        }
    }
}

fn show(v: &PropValue) -> String {
    match v {
        PropValue::None => "none".into(),
        PropValue::Bool(b) => b.to_string(),
        PropValue::Int(n) => n.to_string(),
        PropValue::Float(f) => f.to_string(),
        PropValue::Str(s) => format!("{s:?}"),
        PropValue::Sym(s) => format!(":{s}"),
        PropValue::List(l) => format!("[{}]", l.iter().map(show).collect::<Vec<_>>().join(", ")),
        PropValue::Other(s) => s.clone(),
    }
}

const APP: &str = r##"
    use ui::{Column, Row, Text, Button, CheckBox, TextInput, Event}
    class Todo { prop title; prop mut done = false }
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
      Column(spacing: 8) {
        Row {
          TextInput(id: :input, text <=> draft)
          Button(id: :add, text: "Add", enabled: !draft.empty?) {
            on(:click) { todos = todos + [Todo::new(title: draft)]; draft = "" }
          }
        }
        for t in todos { TodoRow(todo: t) }
        if todos.empty? { Text(text: "Nothing to do") } else { Text(id: :left, text: "#{todos.count(|t| !t.done)} left") }
      }
    }
    $app = TodoApp::new
"##;

#[test]
fn mutations_rebuild_the_tree() {
    let mut i = Interpreter::new();
    i.eval(APP).unwrap();
    let mut m = Mirror::default();
    m.apply(i.take_ui_mutations());
    let first = m.render();
    assert_eq!(
        first,
        [
            "Column(spacing: 8, padding: 0)",
            "  Row(spacing: 0, padding: 0)",
            r#"    TextInput(id: :input, text: "", placeholder: "")"#,
            r#"    Button(id: :add, text: "Add", enabled: false)"#,
            r#"  Text(text: "Nothing to do", color: :black)"#,
        ]
        .join("\n")
    );
    for (step, script) in [
        ("type", r#"$app.find(:input).emit(Event::Text("milk"))"#),
        ("add", "$app.find(:add).emit(:click)"),
        ("type", r#"$app.find(:input).emit(Event::Text("eggs"))"#),
        ("add", "$app.find(:add).emit(:click)"),
        ("check", "$app.todos[0].done = true"),
        ("clear", "$app.todos = []"),
    ] {
        i.eval(script).unwrap();
        m.apply(i.take_ui_mutations());
        let want = i.eval("$app.host_dump").unwrap();
        let want = i.inspect(&want);
        // the script's dump shows the same nodes and props
        let want: String = serde_like(&want);
        assert_eq!(m.render(), want, "after {step}");
    }
}

/// Element#dump's text (as a Ruby string literal) back to plain lines.
fn serde_like(inspected: &str) -> String {
    let s = &inspected[1..inspected.len() - 1];
    s.replace("\\n", "\n").replace("\\\"", "\"")
}

#[test]
fn only_changes_after_the_first_build() {
    let mut i = Interpreter::new();
    i.eval(APP).unwrap();
    let _ = i.take_ui_mutations();
    i.eval(r#"$app.find(:input).emit(Event::Text("x"))"#).unwrap();
    let ms = i.take_ui_mutations();
    // the input's text, the button becoming enabled: two sets, nothing else
    assert_eq!(ms.len(), 2, "{ms:?}");
    assert!(ms.iter().all(|m| matches!(m, Mutation::Set { .. })), "{ms:?}");
    assert!(ms.contains(&Mutation::Set { id: 4, prop: "text".into(), value: PropValue::Str("x".into()) }), "{ms:?}");
    assert!(ms.iter().any(|m| matches!(m, Mutation::Set { prop, value: PropValue::Bool(true), .. } if prop == "enabled")), "{ms:?}");
}
