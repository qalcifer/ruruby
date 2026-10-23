//! `Hash` storage (mruby: `struct RHash`, `src/hash.c`): pairs in insertion
//! order, compared with `eql?`.
//!
//! Like mruby, a small hash is just its pairs, scanned on lookup (the
//! `eql?` of immediates is a bit compare). Above [`SCAN_MAX`] pairs an
//! index finds them: an open-addressing table of positions in `pairs`,
//! probed linearly, with each key's hash kept next to the pairs.
//!
//! Like Ruby, a key mutated after insertion (a String or an Array) isn't
//! found under its new contents.

use crate::gc::Heap;
use crate::value::{Value, eql, hash_key};

/// Up to this many pairs, lookups scan.
const SCAN_MAX: usize = 8;
const EMPTY: u32 = u32::MAX;

#[derive(Clone)]
pub struct RHash {
    pairs: Vec<(Value, Value)>,
    /// `None` while the hash is small.
    index: Option<Box<Index>>,
    pub default: Value,
    /// `Hash.new { |h, k| ... }`
    pub default_proc: Option<Value>,
    /// Built from keyword arguments (`f(a: 1)`), so it binds to keyword parameters.
    pub kw: bool,
}

#[derive(Clone)]
struct Index {
    /// Positions in `pairs`, or `EMPTY`; the length is a power of two, at
    /// least twice the number of pairs.
    slots: Vec<u32>,
    /// `hash_key` of each pair's key, parallel to `pairs`.
    hashes: Vec<u64>,
}

impl Index {
    /// Fills `slots` from `hashes`, sized for `hashes.len()` pairs.
    fn rebuild(&mut self) {
        let cap = (self.hashes.len() * 2).next_power_of_two().max(4 * SCAN_MAX);
        self.slots.clear();
        self.slots.resize(cap, EMPTY);
        for p in 0..self.hashes.len() {
            self.place(p);
        }
    }

    /// Puts pair `p` in the first free slot of its probe sequence.
    fn place(&mut self, p: usize) {
        let mask = self.slots.len() - 1;
        let mut i = self.hashes[p] as usize & mask;
        while self.slots[i] != EMPTY {
            i = (i + 1) & mask;
        }
        self.slots[i] = p as u32;
    }
}

impl RHash {
    pub fn new() -> Self {
        RHash {
            pairs: Vec::new(),
            index: None,
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
        self.index = None;
    }

    /// Position of `k` in `pairs`.
    fn find(&self, heap: &Heap, k: &Value) -> Option<usize> {
        match &self.index {
            None => self.pairs.iter().position(|(key, _)| eql(heap, key, k)),
            Some(index) => self.probe(index, heap, k, hash_key(heap, k)),
        }
    }

    /// Position of `k` (with hash `h`) through the index.
    #[inline(never)]
    fn probe(&self, index: &Index, heap: &Heap, k: &Value, h: u64) -> Option<usize> {
        let mask = index.slots.len() - 1;
        let mut i = h as usize & mask;
        loop {
            let p = index.slots[i];
            if p == EMPTY {
                return None;
            }
            let p = p as usize;
            if index.hashes[p] == h && eql(heap, &self.pairs[p].0, k) {
                return Some(p);
            }
            i = (i + 1) & mask;
        }
    }

    pub fn get(&self, heap: &Heap, k: &Value) -> Option<Value> {
        self.find(heap, k).map(|p| self.pairs[p].1)
    }

    pub fn contains(&self, heap: &Heap, k: &Value) -> bool {
        self.find(heap, k).is_some()
    }

    /// Sets `k`. A new String key must already be a private copy (Ruby
    /// dups string keys); [`Vm::hash_set`](crate::vm::Vm::hash_set) does that.
    pub fn set(&mut self, heap: &Heap, k: Value, v: Value) {
        if self.index.is_some() {
            return self.set_indexed(heap, k, v);
        }
        match self.pairs.iter_mut().find(|(key, _)| eql(heap, key, &k)) {
            Some(slot) => slot.1 = v,
            None => {
                self.pairs.push((k, v));
                if self.pairs.len() > SCAN_MAX {
                    self.build_index(heap);
                }
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn build_index(&mut self, heap: &Heap) {
        let mut index = Box::new(Index {
            slots: Vec::new(),
            hashes: self.pairs.iter().map(|(k, _)| hash_key(heap, k)).collect(),
        });
        index.rebuild();
        self.index = Some(index);
    }

    #[inline(never)]
    fn set_indexed(&mut self, heap: &Heap, k: Value, v: Value) {
        let index = self.index.as_deref().expect("indexed");
        let h = hash_key(heap, &k);
        if let Some(p) = self.probe(index, heap, &k, h) {
            self.pairs[p].1 = v;
            return;
        }
        self.pairs.push((k, v));
        let index = self.index.as_deref_mut().expect("indexed");
        index.hashes.push(h);
        if self.pairs.len() * 2 > index.slots.len() {
            index.rebuild();
        } else {
            index.place(self.pairs.len() - 1);
        }
    }

    pub fn remove(&mut self, heap: &Heap, k: &Value) -> Option<Value> {
        let p = self.find(heap, k)?;
        let (_, v) = self.pairs.remove(p);
        if let Some(index) = &mut self.index {
            // positions after `p` moved: O(n), like the shift itself
            index.hashes.remove(p);
            if self.pairs.len() > SCAN_MAX {
                index.rebuild();
            } else {
                self.index = None;
            }
        }
        Some(v)
    }

    /// `Hash#rehash`: hashes the keys again, after they were mutated.
    /// Keys that became `eql?` keep the last value, at the first's place.
    pub fn rehash(&mut self, heap: &Heap) {
        let pairs = std::mem::take(&mut self.pairs);
        self.index = None;
        for (k, v) in pairs {
            self.set(heap, k, v);
        }
    }
}

impl Default for RHash {
    fn default() -> Self {
        Self::new()
    }
}
