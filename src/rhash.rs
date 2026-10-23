//! `Hash` storage (mruby: `struct RHash`, `src/hash.c`): pairs in insertion
//! order, compared with `eql?`.

use crate::gc::Heap;
use crate::value::{Value, eql};

#[derive(Clone)]
pub struct RHash {
    pairs: Vec<(Value, Value)>,
    pub default: Value,
    /// `Hash.new { |h, k| ... }`
    pub default_proc: Option<Value>,
    /// Built from keyword arguments (`f(a: 1)`), so it binds to keyword parameters.
    pub kw: bool,
}

impl RHash {
    pub fn new() -> Self {
        RHash {
            pairs: Vec::new(),
            default: Value::NIL,
            default_proc: None,
            kw: false,
        }
    }

    /// The pairs in insertion order.
    pub fn pairs(&self) -> &[(Value, Value)] {
        &self.pairs
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    pub fn clear(&mut self) {
        self.pairs.clear();
    }

    pub fn get(&self, heap: &Heap, k: &Value) -> Option<Value> {
        self.pairs
            .iter()
            .find(|(key, _)| eql(heap, key, k))
            .map(|(_, v)| *v)
    }

    pub fn contains(&self, heap: &Heap, k: &Value) -> bool {
        self.pairs.iter().any(|(key, _)| eql(heap, key, k))
    }

    /// Sets `k`. A new String key must already be a private copy (Ruby
    /// dups string keys); [`Vm::hash_set`](crate::vm::Vm::hash_set) does that.
    pub fn set(&mut self, heap: &Heap, k: Value, v: Value) {
        match self.pairs.iter_mut().find(|(key, _)| eql(heap, key, &k)) {
            Some(slot) => slot.1 = v,
            None => self.pairs.push((k, v)),
        }
    }

    pub fn remove(&mut self, heap: &Heap, k: &Value) -> Option<Value> {
        let i = self.pairs.iter().position(|(key, _)| eql(heap, key, k))?;
        Some(self.pairs.remove(i).1)
    }
}

impl Default for RHash {
    fn default() -> Self {
        Self::new()
    }
}
