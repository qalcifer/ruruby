//! The UI's backend protocol (lang-fork-ui): what changes in the element
//! tree, as a list a renderer applies (create a node, set a prop, insert or
//! remove a child). Backend-neutral and testable: a host drains it with
//! `Interpreter::take_ui_mutations` (scripts with `UI::mutations`).
//!
//! The list is of *host* nodes: an `if`/`for` part (a Fragment) and a
//! component used inside another are transparent (their elements count as
//! their parent's), so indices are positions among a host node's visible
//! children. A component made with `new` (no parent) is a `Root` node. An
//! element is announced — `Create`, then a `Set` per prop, then its
//! children — when it's attached under an announced node; later writes of
//! its props are `Set`s.

use crate::value::Value;
use std::collections::HashMap;

/// A prop's value as a renderer sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// A Symbol (`:gray`, `:left`).
    Sym(String),
    List(Vec<PropValue>),
    /// Anything else, as `inspect` shows it.
    Other(String),
}

/// What a renderer reports happened to a node (the script sees an
/// `UI::Event` of the same name).
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Click { x: f64, y: f64, button: String },
    PointerMove { x: f64, y: f64 },
    Scroll { dx: f64, dy: f64 },
    Key { key: String, mods: Vec<String> },
    Text(String),
    Submit,
    Focus(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    Create { id: u64, kind: String },
    Set { id: u64, prop: String, value: PropValue },
    Insert { parent: u64, index: usize, child: u64 },
    Remove { parent: u64, child: u64 },
}

#[derive(Default)]
pub struct UiLog {
    pub mutations: Vec<Mutation>,
    /// Announced elements, by object identity: their node id.
    nodes: HashMap<u64, u64>,
    /// And back: each node's element (GC roots while shown).
    elements: HashMap<u64, Value>,
    next: u64,
}

impl UiLog {
    pub fn node(&self, object: u64) -> Option<u64> {
        self.nodes.get(&object).copied()
    }

    /// The element shown as node `id`.
    pub fn element(&self, id: u64) -> Option<Value> {
        self.elements.get(&id).copied()
    }

    /// For the GC: the shown elements (a Root may be referenced nowhere else).
    pub fn roots(&self) -> impl Iterator<Item = &Value> {
        self.elements.values()
    }

    /// Announces an element: a new node id, and its `Create`.
    pub fn create(&mut self, object: u64, element: Value, kind: &str) -> Option<u64> {
        if self.nodes.contains_key(&object) {
            return None;
        }
        self.next += 1;
        self.nodes.insert(object, self.next);
        self.elements.insert(self.next, element);
        self.mutations.push(Mutation::Create { id: self.next, kind: kind.to_string() });
        Some(self.next)
    }

    pub fn set(&mut self, id: u64, prop: &str, value: PropValue) {
        // a later write of the same prop in the same frame replaces this one
        if let Some(Mutation::Set { id: i, prop: p, value: v }) = self.mutations.last_mut()
            && *i == id
            && p == prop
        {
            *v = value;
            return;
        }
        self.mutations.push(Mutation::Set { id, prop: prop.to_string(), value });
    }

    pub fn insert(&mut self, parent: u64, index: usize, child: u64) {
        self.mutations.push(Mutation::Insert { parent, index, child });
    }

    pub fn remove(&mut self, parent: u64, child: u64) {
        self.mutations.push(Mutation::Remove { parent, child });
    }

    /// An element left the tree: it's no node any more.
    pub fn forget(&mut self, object: u64) {
        if let Some(id) = self.nodes.remove(&object) {
            self.elements.remove(&id);
        }
    }

    pub fn take(&mut self) -> Vec<Mutation> {
        std::mem::take(&mut self.mutations)
    }
}
