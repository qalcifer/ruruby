//! Symbol table — the counterpart of mruby's `src/symbol.c`.
//!
//! Every identifier (method names, ivar names, constants, `:symbols`) is
//! interned once and afterwards handled as a small integer, which makes
//! method-table lookups and comparisons cheap.

use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Sym(pub u32);

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
