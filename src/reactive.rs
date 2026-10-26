//! Reactivity (lang-fork-ui): the VM's side of `ruruby-reactive`.
//!
//! An `effect { }` or `bind(obj, :prop) { }` is an external computation of
//! the runtime, identified by a tag; this table holds what the VM needs to
//! run it (the block, the binding's object and writer). The fields it reads
//! are keyed sources: an object's identity and the instance variable. A
//! binding's key is its target field, so what reads that field runs after
//! the binding (glitch-free, see the crate's docs).
//!
//! The cost when nothing is reactive: a read checks one flag
//! (`Runtime::tracking`); a write checks the object's `observed` flag, set
//! once a computation has read one of its fields.

use crate::symbol::Sym;
use crate::value::Value;
use ruruby_reactive::{NodeId, Runtime};
use std::collections::HashMap;

pub struct Computation {
    pub block: Value,
    /// A binding: the object and the writer its value goes to (`text=`).
    pub target: Option<(Value, Sym)>,
    pub node: NodeId,
}

#[derive(Default)]
pub struct Reactive {
    pub rt: Runtime,
    pub comps: HashMap<u64, Computation>,
    next_tag: u64,
}

/// The source key of an object's field.
pub fn key(object_id: u64, ivar: Sym) -> u128 {
    (object_id as u128) << 32 | ivar.0 as u128
}

impl Reactive {
    pub fn new_tag(&mut self) -> u64 {
        self.next_tag += 1;
        self.next_tag
    }

    /// For the GC: what live computations hold. Drops the entries of
    /// computations the runtime disposed.
    pub fn roots(&mut self) -> Vec<Value> {
        let rt = self.rt.clone();
        self.comps.retain(|_, c| rt.is_alive(c.node));
        let mut v = vec![];
        for c in self.comps.values() {
            v.push(c.block);
            if let Some((obj, _)) = c.target {
                v.push(obj);
            }
        }
        v
    }
}
