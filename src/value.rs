//! Values and heap objects: `include/mruby/value.h`, `object.h`, `class.h`,
//! `proc.h`, `hash.h` in mruby.
//!
//! mruby packs a value into one machine word (NaN/word boxing) and manages
//! heap objects with its own incremental GC. Here a `Value` is a Rust enum
//! and heap objects are reference counted (`Rc`). That is simple and safe,
//! but leaks reference cycles; replacing `Rc` with a tracing GC (`gc.c`) is
//! one of the roadmap items.

use crate::opcode::Irep;
use crate::symbol::Sym;
use crate::vm::{Vm, VmResult};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

/// A method implemented in Rust (mruby: `mrb_func_t`).
pub type NativeFn = fn(&mut Vm, &Value, &[Value], Option<&Value>) -> VmResult;

#[derive(Clone)]
pub enum Method {
    Native(NativeFn),
    Ruby(Rc<Irep>),
    AttrReader(Sym),
    AttrWriter(Sym),
    /// `define_method(:name) { ... }`
    Proc(Value),
    /// `undef_method` marker: stops the lookup.
    Undefined,
}

/// Instance-variable table (mruby: `iv_tbl`). Small and ordered.
#[derive(Default)]
pub struct IvTable {
    pub items: Vec<(Sym, Value)>,
}

impl IvTable {
    pub fn get(&self, k: Sym) -> Option<Value> {
        self.items
            .iter()
            .find(|(s, _)| *s == k)
            .map(|(_, v)| v.clone())
    }

    pub fn set(&mut self, k: Sym, v: Value) {
        match self.items.iter_mut().find(|(s, _)| *s == k) {
            Some(slot) => slot.1 = v,
            None => self.items.push((k, v)),
        }
    }
}

/// Class or module (mruby: `struct RClass`).
pub struct RClass {
    /// Use [`RClass::name`]; anonymous classes get their name on first constant assignment.
    pub name_cell: RefCell<String>,
    pub superclass: Option<Rc<RClass>>,
    pub is_module: bool,
    pub methods: RefCell<HashMap<Sym, Method>>,
    pub consts: RefCell<HashMap<Sym, Value>>,
    /// Included modules (mruby inserts `MRB_TT_ICLASS` proxies into the superclass chain).
    pub includes: RefCell<Vec<Rc<RClass>>>,
    /// Modules prepended with `prepend`, searched before the class itself.
    pub prepends: RefCell<Vec<Rc<RClass>>>,
    pub ivars: RefCell<IvTable>,
    pub cvars: RefCell<IvTable>,
    /// The singleton class ("metaclass"), created on demand (mruby: `MRB_TT_SCLASS`).
    pub meta: RefCell<Option<Rc<RClass>>>,
    /// Set on singleton classes.
    pub is_singleton: bool,
    /// For a class's singleton class: the class it belongs to.
    pub attached: RefCell<Option<Weak<RClass>>>,
    /// The class/module this one was defined in (an approximation of mruby's lexical `cref`).
    pub outer: RefCell<Option<Weak<RClass>>>,
}

impl RClass {
    pub fn new(name: &str, superclass: Option<Rc<RClass>>, is_module: bool) -> Rc<RClass> {
        Rc::new(RClass {
            name_cell: RefCell::new(name.to_string()),
            superclass,
            is_module,
            methods: Default::default(),
            consts: Default::default(),
            includes: Default::default(),
            prepends: Default::default(),
            ivars: Default::default(),
            cvars: Default::default(),
            meta: Default::default(),
            is_singleton: false,
            attached: Default::default(),
            outer: Default::default(),
        })
    }

    pub fn new_singleton(name: String, superclass: Rc<RClass>) -> Rc<RClass> {
        Rc::new(RClass {
            name_cell: RefCell::new(name),
            superclass: Some(superclass),
            is_module: false,
            methods: Default::default(),
            consts: Default::default(),
            includes: Default::default(),
            prepends: Default::default(),
            ivars: Default::default(),
            cvars: Default::default(),
            meta: Default::default(),
            is_singleton: true,
            attached: Default::default(),
            outer: Default::default(),
        })
    }

    pub fn name(&self) -> String {
        self.name_cell.borrow().clone()
    }

    /// Anonymous classes (`Class.new`) are named by their first constant.
    pub fn is_anonymous(&self) -> bool {
        self.name_cell.borrow().starts_with("#<")
    }

    pub fn set_name(&self, name: String) {
        *self.name_cell.borrow_mut() = name;
    }

    pub fn outer_class(&self) -> Option<Rc<RClass>> {
        self.outer.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// The class a singleton class of a class belongs to.
    pub fn attached_class(&self) -> Option<Rc<RClass>> {
        self.attached.borrow().as_ref().and_then(Weak::upgrade)
    }
}

pub struct RObject {
    pub class: Rc<RClass>,
    pub ivars: RefCell<IvTable>,
    /// Per-object singleton class (`def obj.m`, `obj.extend(M)`).
    pub singleton: RefCell<Option<Rc<RClass>>>,
}

impl RObject {
    pub fn new(class: Rc<RClass>) -> RObject {
        RObject {
            class,
            ivars: Default::default(),
            singleton: Default::default(),
        }
    }
}

/// Captured registers of a frame (mruby: `struct REnv`). Blocks keep a
/// reference to their defining frame's env so they can read/write its locals.
pub struct Env {
    pub regs: RefCell<Vec<Value>>,
    pub upper: Option<Rc<Env>>,
}

#[derive(Clone)]
pub enum ProcBody {
    Ruby {
        irep: Rc<Irep>,
        env: Rc<Env>,
    },
    /// `&:sym` (Symbol#to_proc)
    SymCall(Sym),
    /// Internal: collects yielded values (used to run Enumerable over `each`).
    Collect(Rc<RefCell<Vec<Value>>>),
}

pub struct RProc {
    pub body: ProcBody,
    pub self_: Value,
    /// Block of the method the proc was created in (so `yield` works inside blocks).
    pub block: Option<Value>,
    /// Frame id of the enclosing method, the target of `return` inside the block.
    pub home: usize,
    pub method: Option<(Rc<RClass>, Sym)>,
    pub target: Rc<RClass>,
    /// Lambdas check arity strictly and `return` from themselves.
    pub lambda: bool,
}

/// Insertion-ordered hash (mruby: `struct RHash`; it uses open addressing
/// for big tables and a flat array for small ones — this is the flat array only).
#[derive(Clone)]
pub struct RHash {
    pub pairs: Vec<(Value, Value)>,
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
            default: Value::Nil,
            default_proc: None,
            kw: false,
        }
    }

    pub fn get(&self, k: &Value) -> Option<Value> {
        self.pairs
            .iter()
            .find(|(key, _)| eql(key, k))
            .map(|(_, v)| v.clone())
    }

    pub fn set(&mut self, k: Value, v: Value) {
        match self.pairs.iter_mut().find(|(key, _)| eql(key, &k)) {
            Some(slot) => slot.1 = v,
            None => {
                // string keys are copied, like Ruby's frozen key dup
                let k = match &k {
                    Value::Str(s) => Value::str(s.borrow().clone()),
                    _ => k,
                };
                self.pairs.push((k, v))
            }
        }
    }

    pub fn remove(&mut self, k: &Value) -> Option<Value> {
        let i = self.pairs.iter().position(|(key, _)| eql(key, k))?;
        Some(self.pairs.remove(i).1)
    }
}

impl Default for RHash {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Sym(Sym),
    Str(Rc<RefCell<String>>),
    Array(Rc<RefCell<Vec<Value>>>),
    Hash(Rc<RefCell<RHash>>),
    Range(Rc<(Value, Value, bool)>),
    Proc(Rc<RProc>),
    Class(Rc<RClass>),
    Object(Rc<RObject>),
}

impl Value {
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(Rc::new(RefCell::new(s.into())))
    }

    pub fn array(v: Vec<Value>) -> Value {
        Value::Array(Rc::new(RefCell::new(v)))
    }

    pub fn hash(h: RHash) -> Value {
        Value::Hash(Rc::new(RefCell::new(h)))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Identity (`equal?`).
    pub fn identical(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b),
            (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
            (Value::Hash(a), Value::Hash(b)) => Rc::ptr_eq(a, b),
            (Value::Range(a), Value::Range(b)) => Rc::ptr_eq(a, b),
            (Value::Proc(a), Value::Proc(b)) => Rc::ptr_eq(a, b),
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
            _ => values_eq(self, other),
        }
    }
}

/// Structural `==` for built-in types (objects compare by identity here;
/// the VM dispatches user-defined `==` before getting here).
pub fn values_eq(a: &Value, b: &Value) -> bool {
    eq_rec(a, b, &mut Vec::new())
}

/// `in_progress` holds the container pairs currently being compared. Like
/// Ruby, a pair met again while comparing itself is assumed equal, so
/// recursive structures compare without looping forever.
type InProgress = Vec<(usize, usize)>;

fn guarded(
    pa: usize,
    pb: usize,
    ip: &mut InProgress,
    f: impl FnOnce(&mut InProgress) -> bool,
) -> bool {
    if pa == pb || ip.contains(&(pa, pb)) {
        return true;
    }
    ip.push((pa, pb));
    let r = f(ip);
    ip.pop();
    r
}

fn eq_rec(a: &Value, b: &Value, in_progress: &mut InProgress) -> bool {
    use Value::*;
    match (a, b) {
        (Nil, Nil) => true,
        (Bool(x), Bool(y)) => x == y,
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => x == y,
        (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
        (Sym(x), Sym(y)) => x == y,
        (Str(x), Str(y)) => *x.borrow() == *y.borrow(),
        (Array(x), Array(y)) => {
            let (xv, yv) = (x.borrow().clone(), y.borrow().clone());
            guarded(
                Rc::as_ptr(x) as usize,
                Rc::as_ptr(y) as usize,
                in_progress,
                |ip| xv.len() == yv.len() && xv.iter().zip(&yv).all(|(p, q)| eq_rec(p, q, ip)),
            )
        }
        (Hash(x), Hash(y)) => {
            let (xh, yh) = (x.borrow().clone(), y.borrow().clone());
            guarded(
                Rc::as_ptr(x) as usize,
                Rc::as_ptr(y) as usize,
                in_progress,
                |ip| {
                    xh.pairs.len() == yh.pairs.len()
                        && xh
                            .pairs
                            .iter()
                            .all(|(k, v)| yh.get(k).is_some_and(|w| eq_rec(v, &w, ip)))
                },
            )
        }
        (Range(x), Range(y)) => {
            eq_rec(&x.0, &y.0, in_progress) && eq_rec(&x.1, &y.1, in_progress) && x.2 == y.2
        }
        (Proc(x), Proc(y)) => Rc::ptr_eq(x, y),
        (Class(x), Class(y)) => Rc::ptr_eq(x, y),
        (Object(x), Object(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

/// `eql?` — hash-key equality (`1.eql?(1.0)` is false).
pub fn eql(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(_), Value::Float(_)) | (Value::Float(_), Value::Int(_)) => false,
        _ => values_eq(a, b),
    }
}

pub fn num_cmp(x: &Value, y: &Value) -> Option<Ordering> {
    match (x, y) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        _ => None,
    }
}

/// Float formatting compatible with Ruby's `Float#to_s`.
pub fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        let s = format!("{f:e}");
        let (mant, exp) = s.split_once('e').unwrap();
        let mant = if mant.contains('.') {
            mant.to_string()
        } else {
            format!("{mant}.0")
        };
        let (sign, digits) = match exp.strip_prefix('-') {
            Some(d) => ('-', d),
            None => ('+', exp),
        };
        return format!("{mant}e{sign}{digits:0>2}");
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}
