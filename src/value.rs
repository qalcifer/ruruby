//! Values and heap objects: `include/mruby/value.h`, `object.h`, `class.h`,
//! `proc.h`, `hash.h` in mruby.
//!
//! mruby packs a value into one machine word (NaN/word boxing). Here a
//! `Value` is a Rust enum; heap objects live in the [`Heap`](crate::gc::Heap)
//! and a `Value` holds a [`Gc`](crate::gc::Gc) handle to them, classes
//! included, so a `Value` is `Copy`.

use crate::gc::{ArrayRef, ClassRef, EnvRef, HashRef, Heap, ObjRef, ProcRef, RangeRef, StrRef};
use crate::opcode::Irep;
use crate::symbol::{Sym, SymMap};
use crate::vm::{Vm, VmResult};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::cell::Cell;
use std::rc::Rc;

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
    /// `Proc#call` (also `()`, `yield`, `[]`): runs the receiver. A marker
    /// rather than a native so the VM can run Ruby blocks without recursing
    /// (mruby: the OP_CALL irep behind `Proc#call`).
    ProcCall,
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
            .map(|(_, v)| *v)
    }

    pub fn set(&mut self, k: Sym, v: Value) {
        match self.items.iter_mut().find(|(s, _)| *s == k) {
            Some(slot) => slot.1 = v,
            None => self.items.push((k, v)),
        }
    }
}

/// Changes whenever method lookup could find something else: a method
/// table, include or prepend list changed, or a class was created. Inline
/// caches on `Send` are valid only for the epoch they were filled in. One
/// counter for all VMs (a change in one only invalidates the others' caches
/// needlessly); an atomic rather than a thread-local because macOS makes
/// every thread-local read a function call, and this is read on every call.
static METHOD_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn method_epoch() -> u64 {
    METHOD_EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

fn methods_changed() {
    METHOD_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A class's methods. Mutation goes through `insert`/`remove`, which
/// invalidate the method caches.
#[derive(Default)]
pub struct MethodTable(RefCell<SymMap<Method>>);

impl MethodTable {
    pub fn borrow(&self) -> std::cell::Ref<'_, SymMap<Method>> {
        self.0.borrow()
    }

    pub fn insert(&self, name: Sym, m: Method) {
        methods_changed();
        self.0.borrow_mut().insert(name, m);
    }

    pub fn remove(&self, name: &Sym) -> Option<Method> {
        methods_changed();
        self.0.borrow_mut().remove(name)
    }
}

/// Included or prepended modules; `push` invalidates the method caches.
#[derive(Default)]
pub struct ModuleList(RefCell<Vec<ClassRef>>);

impl ModuleList {
    pub fn borrow(&self) -> std::cell::Ref<'_, Vec<ClassRef>> {
        self.0.borrow()
    }

    pub fn push(&self, m: ClassRef) {
        methods_changed();
        self.0.borrow_mut().push(m);
    }
}

/// Class or module (mruby: `struct RClass`). A heap object, referred to by
/// [`ClassRef`] handles.
pub struct RClass {
    /// Use [`RClass::name`]; anonymous classes get their name on first constant assignment.
    pub name_cell: RefCell<String>,
    pub superclass: Option<ClassRef>,
    pub is_module: bool,
    pub methods: MethodTable,
    pub consts: RefCell<SymMap<Value>>,
    /// Included modules (mruby inserts `MRB_TT_ICLASS` proxies into the superclass chain).
    pub includes: ModuleList,
    /// Modules prepended with `prepend`, searched before the class itself.
    pub prepends: ModuleList,
    pub ivars: RefCell<IvTable>,
    pub cvars: RefCell<IvTable>,
    /// The singleton class ("metaclass"), created on demand (mruby: `MRB_TT_SCLASS`).
    pub meta: Cell<Option<ClassRef>>,
    /// Set on singleton classes.
    pub is_singleton: bool,
    /// For a class's singleton class: the class it belongs to.
    pub attached: Cell<Option<ClassRef>>,
    /// The class/module this one was defined in (an approximation of mruby's lexical `cref`).
    pub outer: Cell<Option<ClassRef>>,
}

impl RClass {
    pub fn new(name: &str, superclass: Option<ClassRef>, is_module: bool) -> RClass {
        methods_changed();
        RClass {
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
        }
    }

    pub fn new_singleton(name: String, superclass: ClassRef) -> RClass {
        RClass {
            is_singleton: true,
            ..RClass::new(&name, Some(superclass), false)
        }
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

    pub fn outer_class(&self) -> Option<ClassRef> {
        self.outer.get()
    }

    /// The class a singleton class of a class belongs to.
    pub fn attached_class(&self) -> Option<ClassRef> {
        self.attached.get()
    }
}

pub struct RObject {
    pub class: ClassRef,
    pub ivars: RefCell<IvTable>,
    /// Per-object singleton class (`def obj.m`, `obj.extend(M)`).
    pub singleton: Cell<Option<ClassRef>>,
}

impl RObject {
    pub fn new(class: ClassRef) -> RObject {
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
    pub regs: RefCell<EnvRegs>,
    pub upper: Option<EnvRef>,
}

/// Where an [`Env`]'s variables are: while the frame that owns them runs,
/// they are its registers on the VM stack; when it returns they are copied
/// into the env (mruby: an REnv pointing into the stack, then "unshared").
pub enum EnvRegs {
    Stack { base: usize, len: usize },
    Heap(Vec<Value>),
}

#[derive(Clone)]
pub enum ProcBody {
    Ruby {
        irep: Rc<Irep>,
        env: EnvRef,
    },
    /// `&:sym` (Symbol#to_proc)
    SymCall(Sym),
    /// Internal: collects yielded values (used to run Enumerable over `each`).
    Collect(ArrayRef),
}

pub struct RProc {
    pub body: ProcBody,
    pub self_: Value,
    /// Block of the method the proc was created in (so `yield` works inside blocks).
    pub block: Option<Value>,
    /// Frame id of the enclosing method, the target of `return` inside the block.
    pub home: usize,
    pub method: Option<(ClassRef, Sym)>,
    pub target: ClassRef,
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
            default: V::Nil,
            default_proc: None,
            kw: false,
        }
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

/// A value as code inspects it: what `match heap.kind(v)` gives.
#[derive(Clone, Copy, Default)]
pub enum V {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Sym(Sym),
    Str(StrRef),
    Array(ArrayRef),
    Hash(HashRef),
    Range(RangeRef),
    Proc(ProcRef),
    Class(ClassRef),
    Object(ObjRef),
}

/// For now the stored form is the view itself.
pub type Value = V;

impl V {
    pub fn truthy(&self) -> bool {
        !matches!(self, V::Nil | V::Bool(false))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, V::Int(_) | V::Float(_))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, V::Nil)
    }

    /// Identity (`equal?`).
    pub fn identical(&self, other: &Value) -> bool {
        use V::*;
        match (self, other) {
            (Nil, Nil) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Sym(a), Sym(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Hash(a), Hash(b)) => a == b,
            (Range(a), Range(b)) => a == b,
            (Proc(a), Proc(b)) => a == b,
            (Class(a), Class(b)) => a == b,
            (Object(a), Object(b)) => a == b,
            _ => false,
        }
    }

    /// Identity as a number (`object_id`, recursion guards); 0 for immediates.
    pub fn heap_id(&self) -> u64 {
        match self {
            V::Str(h) => h.id(),
            V::Array(h) => h.id(),
            V::Hash(h) => h.id(),
            V::Range(h) => h.id(),
            V::Proc(h) => h.id(),
            V::Object(h) => h.id(),
            V::Class(h) => h.id(),
            _ => 0,
        }
    }
}

/// Structural `==` for built-in types (objects compare by identity here;
/// the VM dispatches user-defined `==` before getting here).
pub fn values_eq(heap: &Heap, a: &Value, b: &Value) -> bool {
    eq_rec(heap, a, b, &mut Vec::new())
}

/// `in_progress` holds the container pairs currently being compared. Like
/// Ruby, a pair met again while comparing itself is assumed equal, so
/// recursive structures compare without looping forever.
type InProgress = Vec<(u64, u64)>;

fn guarded(
    pa: u64,
    pb: u64,
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

fn eq_rec(heap: &Heap, a: &Value, b: &Value, in_progress: &mut InProgress) -> bool {
    use V::*;
    match (a, b) {
        (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
        (Str(x), Str(y)) => x == y || *heap[*x].borrow() == *heap[*y].borrow(),
        (Array(x), Array(y)) => {
            let (xv, yv) = (heap[*x].borrow().clone(), heap[*y].borrow().clone());
            guarded(x.id(), y.id(), in_progress, |ip| {
                xv.len() == yv.len() && xv.iter().zip(&yv).all(|(p, q)| eq_rec(heap, p, q, ip))
            })
        }
        (Hash(x), Hash(y)) => {
            let (xh, yh) = (heap[*x].borrow().clone(), heap[*y].borrow().clone());
            guarded(x.id(), y.id(), in_progress, |ip| {
                xh.pairs.len() == yh.pairs.len()
                    && xh.pairs.iter().all(|(k, v)| {
                        yh.get(heap, k)
                            .is_some_and(|w| eq_rec(heap, v, &w, ip))
                    })
            })
        }
        (Range(x), Range(y)) => {
            let (x, y) = (&heap[*x], &heap[*y]);
            eq_rec(heap, &x.0, &y.0, in_progress) && eq_rec(heap, &x.1, &y.1, in_progress) && x.2 == y.2
        }
        _ => a.identical(b),
    }
}

/// `eql?` — hash-key equality (`1.eql?(1.0)` is false).
pub fn eql(heap: &Heap, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (V::Int(_), V::Float(_)) | (V::Float(_), V::Int(_)) => false,
        _ => values_eq(heap, a, b),
    }
}

pub fn num_cmp(x: &Value, y: &Value) -> Option<Ordering> {
    match (x, y) {
        (V::Int(a), V::Int(b)) => Some(a.cmp(b)),
        (V::Int(a), V::Float(b)) => (*a as f64).partial_cmp(b),
        (V::Float(a), V::Int(b)) => a.partial_cmp(&(*b as f64)),
        (V::Float(a), V::Float(b)) => a.partial_cmp(b),
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
