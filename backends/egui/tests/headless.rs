//! The egui backend without a window: frames run on a bare egui::Context
//! with synthetic input.

use eframe::egui;
use ruruby::ui::PropValue;
use ruruby_egui::UiApp;

const APP: &str = r##"
use ui::{Window, Column, Row, Text, Button, TextInput, CheckBox}

component App {
  prop mut count = 0
  prop mut name = ""
  prop mut agreed = false
  Window(title: "Counter", width: 300, height: 200) {
    Column(spacing: 6) {
      Text(id: :label, text: "Count: #{count}")
      Row {
        Button(id: :plus, text: "+1") { on(:click) { count += 1 } }
        Button(id: :reset, text: "reset", enabled: count > 0) { on(:click) { count = 0 } }
      }
      TextInput(id: :name, text <=> name, placeholder: "name")
      Text(id: :hello, text: "hello #{name}")
      CheckBox(id: :agree, checked <=> agreed, text: "agree")
    }
  }
}
"##;

struct Harness {
    ctx: egui::Context,
    app: UiApp,
}

impl Harness {
    fn new() -> Harness {
        let app = UiApp::from_source(APP, "app.rb").unwrap();
        Harness { ctx: egui::Context::default(), app }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run_ui(input, |ui| app.show(ui));
    }

    fn center(&self, id: &str) -> egui::Pos2 {
        let node = self.app.tree.find(id).unwrap_or_else(|| panic!("no node {id}"));
        self.app.rects[&node].center()
    }

    fn click(&mut self, id: &str) {
        let pos = self.center(id);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        self.frame(vec![egui::Event::PointerMoved(pos), button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![]);
    }

    fn text(&self, id: &str) -> String {
        let node = self.app.tree.find(id).unwrap();
        match self.app.tree.nodes[&node].props.get("text") {
            Some(PropValue::Str(s)) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn eval(&mut self, src: &str) -> String {
        let v = self.app.interp.eval(src).unwrap();
        self.app.interp.inspect(&v)
    }
}

#[test]
fn clicks_and_typing_reach_the_script() {
    let mut h = Harness::new();
    assert_eq!(h.app.window(), Some(("Counter".into(), 300.0, 200.0)));
    h.frame(vec![]);
    assert_eq!(h.text("label"), "Count: 0");
    h.click("plus");
    h.click("plus");
    assert_eq!(h.text("label"), "Count: 2");
    assert_eq!(h.eval("$__ui_root.count"), "2");
    h.click("reset");
    assert_eq!(h.text("label"), "Count: 0");
    // a disabled button doesn't click
    h.click("reset");
    assert_eq!(h.eval("$__ui_root.count"), "0");
    // typing: focus the input, then text
    h.click("name");
    h.frame(vec![egui::Event::Text("Ann".into())]);
    h.frame(vec![]);
    assert_eq!(h.eval("$__ui_root.name"), "\"Ann\"");
    assert_eq!(h.text("hello"), "hello Ann");
    // the script changes the text: the input shows it
    h.eval("$__ui_root.name = \"Bob\"");
    h.app.sync();
    h.frame(vec![]);
    assert_eq!(h.text("name"), "Bob");
    h.click("agree");
    assert_eq!(h.eval("$__ui_root.agreed"), "true");
    assert!(h.app.error.is_none(), "{:?}", h.app.error);
}

#[test]
fn errors() {
    let e = UiApp::from_source("x = 1", "empty.rb").err().unwrap();
    assert!(e.contains("no UI"), "{e}");
    let e = UiApp::from_source("component App { nope( }", "bad.rb").err().unwrap();
    assert!(e.contains("syntax error"), "{e}");
}

/// The todo example runs: add by typing and Enter, check, delete.
#[test]
fn todo_example() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/ui/todo.rb")).unwrap();
    let mut app = UiApp::from_source(&src, "todo.rb").unwrap();
    let ctx = egui::Context::default();
    let frame = |app: &mut UiApp, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 500.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.show(ui));
    };
    frame(&mut app, vec![]);
    let eval = |app: &mut UiApp, src: &str| {
        let v = app.interp.eval(src).unwrap();
        app.interp.inspect(&v)
    };
    // the input: the first TextInput
    let input = *app.tree.nodes.iter().find(|(_, n)| n.kind == "TextInput").unwrap().0;
    let pos = app.rects[&input].center();
    let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, vec![egui::Event::PointerMoved(pos), press(true)]);
    frame(&mut app, vec![press(false)]);
    frame(&mut app, vec![egui::Event::Text("buy milk".into())]);
    let enter = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    frame(&mut app, vec![enter(true), enter(false)]);
    frame(&mut app, vec![]);
    assert_eq!(eval(&mut app, "$__ui_root.todos.map(&:title)"), r#"["try ruruby-ui", "buy milk"]"#, "{:?}", app.error);
    assert_eq!(eval(&mut app, "$__ui_root.draft"), r#""""#);
    eval(&mut app, "$__ui_root.todos[0].done = true");
    app.sync();
    let texts: Vec<String> = app.tree.nodes.values().filter(|n| n.kind == "Text").map(|n| format!("{:?}", n.props.get("text"))).collect();
    assert!(texts.iter().any(|t| t.contains("1 of 2 left")), "{texts:?}");
    assert!(app.error.is_none(), "{:?}", app.error);
}

/// --screenshot's PNG writer: a valid file of the right size.
#[test]
fn png_writer() {
    let img = egui::ColorImage::new([3, 2], vec![egui::Color32::RED; 6]);
    let path = std::env::temp_dir().join(format!("ruruby-png-{}.png", std::process::id()));
    ruruby_egui::write_png(path.to_str().unwrap(), &img).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&bytes[12..16], b"IHDR");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 3);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 2);
    assert!(bytes.ends_with(&[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]), "IEND with its CRC");
}
