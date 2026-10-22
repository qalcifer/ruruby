//! Symbol table — the counterpart of mruby's `src/symbol.c`.
//!
//! Every identifier (method names, ivar names, constants, `:symbols`) is
//! interned once and afterwards handled as a small integer, which makes
//! method-table lookups and comparisons cheap.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Sym(pub u32);

/// A map keyed by symbols (method tables, constants, globals). Symbols are
/// small distinct integers, so they need no SipHash (std's default, built
/// to resist hash flooding): a multiplicative hash is enough and much faster.
pub type SymMap<V> = HashMap<Sym, V, BuildHasherDefault<SymHasher>>;

#[derive(Default)]
pub struct SymHasher(u64);

impl Hasher for SymHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 << 8 | *b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u32(&mut self, n: u32) {
        // Fibonacci hashing: spreads consecutive symbols over the high bits
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

#[derive(Default)]
pub struct SymbolTable {
    map: HashMap<Rc<str>, Sym>,
    names: Vec<Rc<str>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Sym(self.names.len() as u32);
        let name: Rc<str> = Rc::from(s);
        self.names.push(name.clone());
        self.map.insert(name, sym);
        sym
    }

    pub fn name(&self, sym: Sym) -> &str {
        &self.names[sym.0 as usize]
    }
}
