//! `Hash` storage (mruby: `struct RHash`, `src/hash.c`): entries in
//! insertion order, compared with `eql?`.
//!
//! Like mruby, a small hash is just its entries, scanned on lookup (the
//! `eql?` of immediates is a bit compare). Above [`SCAN_MAX`] entries an
//! index finds them: an open-addressing table of positions in `entries`,
//! probed linearly. The keys' hashes are kept beside the entries, so the
//! table can be rebuilt without hashing again; small hashes don't hash
//! built-in keys at all.
//!
//! Deleting from an indexed hash leaves a dead entry (key
//! [`Value::DELETED`]) so positions stay put; the entries are compacted
//! once the dead outnumber the live.
//!
//! Like Ruby, a key mutated after insertion (a String or an Array) isn't
//! found under its new contents until `rehash`.
//!
//! This level compares keys with the built-in `eql?`; keys with their own
//! `hash`/`eql?` go through the VM ([`Vm::hash_find`](crate::vm::Vm::hash_find)),
//! which uses the positional methods here.

use crate::gc::Heap;
use crate::value::{Value, eql, hash_key};

/// Up to this many entries, lookups scan.
const SCAN_MAX: usize = 8;
const EMPTY: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Entry {
    key: Value,
    val: Value,
}

/// What a hash needs once it's big (or holds a key hashed by the VM).
#[derive(Clone, Default)]
struct Big {
    /// Each entry's key hash ([`stored_hash`], or 0: not computed yet),
    /// as long as `entries`.
    hashes: Vec<u64>,
    /// Positions in `entries`, or `EMPTY`; empty while not indexed. The
    /// length is a power of two, at least twice `entries.len()`.
    slots: Vec<u32>,
}

#[derive(Clone)]
pub struct RHash {
    entries: Vec<Entry>,
    big: Option<Box<Big>>,
    /// Live entries (`entries` also holds dead ones while indexed).
    live: u32,
    /// Changes whenever entries are added, removed or moved (not when a
    /// value is replaced): a lookup that runs Ruby code checks it.
    version: u32,
    pub default: Value,
    /// `Hash.new { |h, k| ... }`
    pub default_proc: Option<Value>,
    /// Built from keyword arguments (`f(a: 1)`), so it binds to keyword parameters.
    pub kw: bool,
}

/// A hash as stored: never 0, which means "not computed".
pub fn stored_hash(h: u64) -> u64 {
    if h == 0 { 1 } else { h }
}

impl RHash {
    pub fn new() -> Self {
        RHash {
            entries: Vec::new(),
            big: None,
            live: 0,
            version: 0,
            default: Value::NIL,
            default_proc: None,
            kw: false,
        }
    }

    /// The pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (Value, Value)> + '_ {
        self.entries.iter().filter(|e| !e.key.is_deleted()).map(|e| (e.key, e.val))
    }

    pub fn to_vec(&self) -> Vec<(Value, Value)> {
        self.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.live as usize
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.big = None;
        self.live = 0;
        self.changed();
    }

    fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// The table, when indexed.
    fn indexed(&self) -> Option<&Big> {
        self.big.as_deref().filter(|b| !b.slots.is_empty())
    }

    /// Position of `k` (by the built-in `eql?`).
    pub fn find(&self, heap: &Heap, k: &Value) -> Option<usize> {
        match self.indexed() {
            None => self.entries.iter().position(|e| eql(heap, &e.key, k)),
            Some(big) => {
                let h = stored_hash(hash_key(heap, k));
                self.probe(big, h).find(|&p| eql(heap, &self.entries[p].key, k))
            }
        }
    }

    /// Positions of the live entries whose hash is `h`, in probe order.
    fn probe<'a>(&'a self, big: &'a Big, h: u64) -> impl Iterator<Item = usize> + 'a {
        let mask = big.slots.len() - 1;
        let mut i = h as usize & mask;
        std::iter::from_fn(move || {
            loop {
                let p = big.slots[i];
                if p == EMPTY {
                    return None;
                }
                i = (i + 1) & mask;
                let p = p as usize;
                if big.hashes[p] == h && !self.entries[p].key.is_deleted() {
                    return Some(p);
                }
            }
        })
    }

    /// The positions a key with stored hash `h` may be at: the live
    /// entries with hash `h`, and while the hash is small also those whose
    /// hash isn't computed (built-in keys; like mruby, a small hash asks
    /// `eql?` of them).
    pub fn candidates(&self, h: u64) -> Vec<usize> {
        match (self.indexed(), &self.big) {
            (Some(big), _) => self.probe(big, h).collect(),
            (None, Some(big)) => (0..self.entries.len())
                .filter(|&p| big.hashes[p] == 0 || big.hashes[p] == h)
                .collect(),
            (None, None) => (0..self.entries.len()).collect(),
        }
    }

    /// The key at `p`, if that entry is live.
    pub fn key_at(&self, p: usize) -> Option<Value> {
        self.entries.get(p).map(|e| e.key).filter(|k| !k.is_deleted())
    }

    pub fn value_at(&self, p: usize) -> Value {
        self.entries[p].val
    }

    pub fn set_value_at(&mut self, p: usize, v: Value) {
        self.entries[p].val = v;
    }

    pub fn get(&self, heap: &Heap, k: &Value) -> Option<Value> {
        self.find(heap, k).map(|p| self.entries[p].val)
    }

    pub fn contains(&self, heap: &Heap, k: &Value) -> bool {
        self.find(heap, k).is_some()
    }

    /// Sets `k`. A new String key must already be a private copy (Ruby
    /// dups string keys); [`Vm::hash_set`](crate::vm::Vm::hash_set) does that.
    pub fn set(&mut self, heap: &Heap, k: Value, v: Value) {
        match self.find(heap, &k) {
            Some(p) => self.entries[p].val = v,
            None => {
                let h = if self.indexed().is_some() { stored_hash(hash_key(heap, &k)) } else { 0 };
                self.push(heap, k, v, h)
            }
        }
    }

    /// Adds a key that isn't in the hash. `h` is its [`stored_hash`], or 0
    /// for a built-in key while the hash is small.
    pub fn push(&mut self, heap: &Heap, k: Value, v: Value, h: u64) {
        self.entries.push(Entry { key: k, val: v });
        self.live += 1;
        self.changed();
        if h != 0 && self.big.is_none() {
            let mut big = Box::<Big>::default();
            big.hashes.resize(self.entries.len() - 1, 0);
            self.big = Some(big);
        }
        let n = self.entries.len();
        match &mut self.big {
            None if n > SCAN_MAX => self.reindex(heap),
            None => {}
            Some(big) => {
                big.hashes.push(h);
                if big.slots.is_empty() {
                    if n > SCAN_MAX {
                        self.reindex(heap);
                    }
                } else if n * 2 <= big.slots.len() {
                    place(&mut big.slots, h, n - 1);
                } else {
                    self.reindex(heap);
                }
            }
        }
    }

    pub fn remove(&mut self, heap: &Heap, k: &Value) -> Option<Value> {
        let p = self.find(heap, k)?;
        Some(self.remove_at(heap, p))
    }

    /// Removes the live entry at `p`; returns its value.
    pub fn remove_at(&mut self, heap: &Heap, p: usize) -> Value {
        let val = self.entries[p].val;
        self.live -= 1;
        self.changed();
        if self.indexed().is_none() {
            self.entries.remove(p);
            if let Some(big) = &mut self.big {
                big.hashes.remove(p);
            }
            return val;
        }
        self.entries[p] = Entry {
            key: Value::DELETED,
            val: Value::NIL,
        };
        if self.len() <= SCAN_MAX {
            self.compact();
            if let Some(big) = &mut self.big {
                big.slots = Vec::new();
            }
        } else if self.entries.len() - self.len() > self.len() {
            self.compact();
            self.reindex(heap);
        }
        val
    }

    fn compact(&mut self) {
        if let Some(big) = &mut self.big {
            let mut live = self.entries.iter().map(|e| !e.key.is_deleted());
            big.hashes.retain(|_| live.next().unwrap_or(false));
        }
        self.entries.retain(|e| !e.key.is_deleted());
    }

    /// Builds the table for the current entries.
    #[cold]
    #[inline(never)]
    fn reindex(&mut self, heap: &Heap) {
        let big = self.big.get_or_insert_with(Box::default);
        big.hashes.resize(self.entries.len(), 0);
        let cap = (self.entries.len() * 2).next_power_of_two().max(4 * SCAN_MAX);
        big.slots.clear();
        big.slots.resize(cap, EMPTY);
        for (p, (e, h)) in self.entries.iter().zip(&mut big.hashes).enumerate() {
            if !e.key.is_deleted() {
                if *h == 0 {
                    *h = stored_hash(hash_key(heap, &e.key));
                }
                place(&mut big.slots, *h, p);
            }
        }
    }
}

/// Puts position `p` in the first free slot of its probe sequence.
fn place(slots: &mut [u32], h: u64, p: usize) {
    let mask = slots.len() - 1;
    let mut i = h as usize & mask;
    while slots[i] != EMPTY {
        i = (i + 1) & mask;
    }
    slots[i] = p as u32;
}

impl Default for RHash {
    fn default() -> Self {
        Self::new()
    }
}
