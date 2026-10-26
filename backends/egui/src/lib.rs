//! A first renderer for ruruby's declarative UI, on egui (lang-fork-ui).
//!
//! The script builds a component tree; this backend keeps a mirror of the
//! tree's host nodes, built from the interpreter's change list
//! (`Interpreter::take_ui_mutations`), draws it with egui every frame, and
//! sends what the user did back as events (`Interpreter::ui_event`). After
//! the script's handlers run, the new changes come in and the next frame
//! shows them.
//!
//! A prototype: egui's own layout and look, the elements of `ui.rb`
//! (Window, Column, Row, Text, Button, TextInput, CheckBox); unknown kinds
//! are drawn as a placeholder with their children.

use eframe::egui;
use ruruby::Interpreter;
use ruruby::ui::{Mutation, PropValue, UiEvent};
use std::collections::HashMap;

/// A host node: kind, props, children.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub kind: String,
    pub props: HashMap<String, PropValue>,
    pub children: Vec<u64>,
}

/// The mirror of the script's tree.
#[derive(Default)]
pub struct Tree {
    pub nodes: HashMap<u64, Node>,
    pub root: Option<u64>,
}

impl Tree {
    pub fn apply(&mut self, mutations: Vec<Mutation>) {
        for m in mutations {
            match m {
                Mutation::Create { id, kind } => {
                    if kind == "Root" && self.root.is_none() {
                        self.root = Some(id);
                    }
                    self.nodes.insert(id, Node { kind, ..Node::default() });
                }
                Mutation::Set { id, prop, value } => {
                    if let Some(n) = self.nodes.get_mut(&id) {
                        n.props.insert(prop, value);
                    }
                }
                Mutation::Insert { parent, index, child } => {
                    if let Some(n) = self.nodes.get_mut(&parent) {
                        let at = index.min(n.children.len());
                        n.children.insert(at, child);
                    }
                }
                Mutation::Remove { parent, child } => {
                    if let Some(n) = self.nodes.get_mut(&parent) {
                        n.children.retain(|c| *c != child);
                    }
                    self.drop_subtree(child);
                }
            }
        }
    }

    fn drop_subtree(&mut self, id: u64) {
        if let Some(n) = self.nodes.remove(&id) {
            for c in n.children {
                self.drop_subtree(c);
            }
        }
    }

    /// The first node whose `id` prop is the symbol `id`.
    pub fn find(&self, id: &str) -> Option<u64> {
        self.nodes.iter().find(|(_, n)| matches!(n.props.get("id"), Some(PropValue::Sym(s)) if s == id)).map(|(k, _)| *k)
    }
}

fn str_prop(n: &Node, name: &str) -> String {
    match n.props.get(name) {
        Some(PropValue::Str(s)) | Some(PropValue::Sym(s)) | Some(PropValue::Other(s)) => s.clone(),
        Some(PropValue::Int(i)) => i.to_string(),
        Some(PropValue::Float(f)) => f.to_string(),
        _ => String::new(),
    }
}

fn num_prop(n: &Node, name: &str, default: f32) -> f32 {
    match n.props.get(name) {
        Some(PropValue::Int(i)) => *i as f32,
        Some(PropValue::Float(f)) => *f as f32,
        _ => default,
    }
}

fn bool_prop(n: &Node, name: &str, default: bool) -> bool {
    match n.props.get(name) {
        Some(PropValue::Bool(b)) => *b,
        Some(PropValue::None) => false,
        _ => default,
    }
}

fn color(ui: &egui::Ui, n: &Node) -> egui::Color32 {
    match n.props.get("color") {
        Some(PropValue::Sym(c)) => match c.as_str() {
            "gray" | "grey" => egui::Color32::GRAY,
            "red" => egui::Color32::RED,
            "green" => egui::Color32::GREEN,
            "blue" => egui::Color32::LIGHT_BLUE,
            "yellow" => egui::Color32::YELLOW,
            "white" => egui::Color32::WHITE,
            _ => ui.visuals().text_color(),
        },
        _ => ui.visuals().text_color(),
    }
}

/// A script's UI, shown with egui.
pub struct UiApp {
    pub interp: Interpreter,
    pub tree: Tree,
    /// Text being edited, by node, and the prop value it last matched
    /// (egui edits a String in place).
    buffers: HashMap<u64, (String, String)>,
    /// Where each node was drawn last frame (for tests and tools).
    pub rects: HashMap<u64, egui::Rect>,
    /// The last error a handler raised, shown at the top.
    pub error: Option<String>,
    title: Option<String>,
}

impl UiApp {
    /// Runs the script `src` (as file `file`): it makes the root component
    /// (`App::new`, or `App` is made if it defines a component of that name).
    pub fn from_source(src: &str, file: &str) -> Result<UiApp, String> {
        let mut interp = Interpreter::new();
        let irep = interp.compile_file(src, file, 1).map_err(|e| e.to_string())?;
        interp.run(irep).map_err(|e| e.to_string())?;
        let mut app = UiApp {
            interp,
            tree: Tree::default(),
            buffers: HashMap::new(),
            rects: HashMap::new(),
            error: None,
            title: None,
        };
        app.sync();
        if app.tree.root.is_none() {
            app.interp
                .eval("$__ui_root = App::new if defined?(App)")
                .map_err(|e| format!("making App: {e}"))?;
            app.sync();
        }
        if app.tree.root.is_none() {
            return Err(format!("{file}: no UI: make a component with new, or define `component App`"));
        }
        Ok(app)
    }

    pub fn load(path: &str) -> Result<UiApp, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        UiApp::from_source(&src, path)
    }

    /// The script's changes into the mirror.
    pub fn sync(&mut self) -> bool {
        let ms = self.interp.take_ui_mutations();
        let any = !ms.is_empty();
        self.tree.apply(ms);
        any
    }

    /// The first Window under the root: (title, width, height).
    pub fn window(&self) -> Option<(String, f32, f32)> {
        let root = self.tree.nodes.get(&self.tree.root?)?;
        let w = root.children.iter().filter_map(|c| self.tree.nodes.get(c)).find(|n| n.kind == "Window")?;
        Some((str_prop(w, "title"), num_prop(w, "width", 640.0), num_prop(w, "height", 480.0)))
    }

    /// One frame: draws the tree, then sends what happened to the script.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::RED, e);
        }
        let mut events = vec![];
        if let Some(root) = self.tree.root {
            let children = self.tree.nodes.get(&root).map(|n| n.children.clone()).unwrap_or_default();
            ui.vertical(|ui| {
                for c in children {
                    self.draw(ui, c, &mut events);
                }
            });
        }
        let mut changed = false;
        for (node, event) in events {
            match self.interp.ui_event(node, event) {
                Ok(_) => {}
                Err(e) => self.error = Some(e.to_string()),
            }
            changed |= self.sync();
        }
        if changed {
            ui.ctx().request_repaint();
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, id: u64, events: &mut Vec<(u64, UiEvent)>) {
        let Some(n) = self.tree.nodes.get(&id).cloned() else { return };
        let response = match n.kind.as_str() {
            "Window" => {
                let title = str_prop(&n, "title");
                if self.title.as_ref() != Some(&title) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                    self.title = Some(title);
                }
                self.container(ui, &n, false, events)
            }
            "Column" => self.container(ui, &n, false, events),
            "Row" => self.container(ui, &n, true, events),
            "Text" => {
                let c = color(ui, &n);
                ui.label(egui::RichText::new(str_prop(&n, "text")).color(c))
            }
            "Button" => {
                let r = ui.add_enabled(bool_prop(&n, "enabled", true), egui::Button::new(str_prop(&n, "text")));
                if r.clicked() {
                    events.push((id, click(&r)));
                }
                r
            }
            "TextInput" => {
                let text = str_prop(&n, "text");
                let (buffer, synced) = self.buffers.entry(id).or_insert_with(|| (text.clone(), text.clone()));
                // the script changed it: take its value
                if *synced != text {
                    *buffer = text.clone();
                    *synced = text;
                }
                let edit = egui::TextEdit::singleline(buffer).hint_text(str_prop(&n, "placeholder"));
                let r = ui.add(edit);
                if r.changed() {
                    *synced = buffer.clone();
                    events.push((id, UiEvent::Text(buffer.clone())));
                }
                if r.gained_focus() {
                    events.push((id, UiEvent::Focus(true)));
                }
                if r.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        events.push((id, UiEvent::Submit));
                    }
                    events.push((id, UiEvent::Focus(false)));
                }
                r
            }
            "CheckBox" => {
                // the script decides (its click toggles `checked`)
                let mut checked = bool_prop(&n, "checked", false);
                let r = ui.checkbox(&mut checked, str_prop(&n, "text"));
                if r.clicked() {
                    events.push((id, click(&r)));
                }
                r
            }
            other => {
                let other = other.to_string();
                ui.group(|ui| {
                    ui.weak(format!("[{other}]"));
                    for c in &n.children {
                        self.draw(ui, *c, events);
                    }
                })
                .response
            }
        };
        self.rects.insert(id, response.rect);
    }

    /// Column / Row: padding around, spacing between.
    fn container(&mut self, ui: &mut egui::Ui, n: &Node, horizontal: bool, events: &mut Vec<(u64, UiEvent)>) -> egui::Response {
        let (padding, spacing) = (num_prop(n, "padding", 0.0), num_prop(n, "spacing", 4.0));
        let children = n.children.clone();
        egui::Frame::new()
            .inner_margin(padding)
            .show(ui, |ui| {
                let body = |ui: &mut egui::Ui| {
                    if horizontal {
                        ui.spacing_mut().item_spacing.x = spacing;
                    } else {
                        ui.spacing_mut().item_spacing.y = spacing;
                    }
                    for c in &children {
                        self.draw(ui, *c, events);
                    }
                };
                if horizontal { ui.horizontal(body) } else { ui.vertical(body) }
            })
            .response
    }
}

/// A click where the pointer was, relative to the widget.
fn click(r: &egui::Response) -> UiEvent {
    let p = r.interact_pointer_pos().unwrap_or(r.rect.center()) - r.rect.min;
    UiEvent::Click { x: p.x as f64, y: p.y as f64, button: "left".into() }
}

impl eframe::App for UiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default_margins().show(ui, |ui| self.show(ui));
    }
}
