//! Values and heap objects: `include/mruby/value.h`, `object.h`, `class.h`,
//! `proc.h`, `hash.h` in mruby.
//!
//! mruby packs a value into one machine word (NaN/word boxing). Here a
//! `Value` is a Rust enum; heap objects live in the [`Heap`](crate::gc::Heap)
//! and a `Value` holds a [`Gc`](crate::gc::Gc) handle to them, classes
//! included, so a `Value` is `Copy`.

use crate::gc::{
    ArrayRef, ClassRef, EnvRef, GEN_BITS, Gc, HashRef, Heap, IntRef, ObjRef, ProcRef, RangeRef, StrRef,
};
use crate::opcode::Irep;
pub use crate::rhash::RHash;
use crate::symbol::{Sym, SymMap};
use crate::vm::{Vm, VmResult};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;
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
pub struct MethodTable {
    methods: RefCell<SymMap<Method>>,
    /// Visibility of the methods that aren't public (most are).
    vis: RefCell<SymMap<Vis>>,
}

/// Method visibility (mruby: `MRB_METHOD_VISIBILITY`). `ModuleFunction` is
/// only a scope's default (`module_function` with no arguments); a method
/// itself is never that.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Vis {
    #[default]
    Public,
    Private,
    Protected,
    ModuleFunction,
}

impl MethodTable {
    pub fn borrow(&self) -> std::cell::Ref<'_, SymMap<Method>> {
        self.methods.borrow()
    }

    /// Defines a public method (or replaces one, which makes it public).
    pub fn insert(&self, name: Sym, m: Method) {
        self.insert_vis(name, m, Vis::Public);
    }

    pub fn insert_vis(&self, name: Sym, m: Method, vis: Vis) {
        methods_changed();
        self.methods.borrow_mut().insert(name, m);
        self.store_vis(name, vis);
    }

    pub fn remove(&self, name: &Sym) -> Option<Method> {
        methods_changed();
        self.vis.borrow_mut().remove(name);
        self.methods.borrow_mut().remove(name)
    }

    pub fn vis(&self, name: Sym) -> Vis {
        self.vis.borrow().get(&name).copied().unwrap_or_default()
    }

    /// Changes the visibility of a method defined here.
    pub fn set_vis(&self, name: Sym, vis: Vis) {
        methods_changed();
        self.store_vis(name, vis);
    }

    fn store_vis(&self, name: Sym, vis: Vis) {
        let vis = if vis == Vis::ModuleFunction { Vis::Private } else { vis };
        if vis == Vis::Public {
            self.vis.borrow_mut().remove(&name);
        } else {
            self.vis.borrow_mut().insert(name, vis);
        }
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
    /// `freeze`: no more methods, constants, class variables, ivars, modules.
    pub frozen: Cell<bool>,
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
            frozen: Default::default(),
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
    pub frozen: Cell<bool>,
}

impl RObject {
    pub fn new(class: ClassRef) -> RObject {
        RObject {
            class,
            ivars: Default::default(),
            singleton: Default::default(),
            frozen: Default::default(),
        }
    }
}

/// Captured registers of a frame (mruby: `struct REnv`). Blocks keep a
/// reference to their defining frame's env so they can read/write its locals.
pub struct Env {
    pub regs: RefCell<EnvRegs>,
    pub upper: Option<EnvRef>,
    /// The default visibility of the scope (method or class body) this env
    /// belongs to, when it is the scope's own env: blocks share it, so a
    /// `private` in a block reaches the scope.
    pub vis: Cell<Vis>,
    /// For a block's env, the env of the scope it is in (that has `vis`);
    /// None for a scope's own env.
    pub scope: Option<EnvRef>,
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
        /// The creating frame's env; None when the block doesn't need it
        /// (Irep::needs_env).
        env: Option<EnvRef>,
    },
    /// `&:sym` (Symbol#to_proc)
    SymCall(Sym),
    /// Internal: collects yielded values (used to run Enumerable over `each`).
    Collect(ArrayRef),
    /// `Method#to_proc`: calls the Method object.
    Method(Value),
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

/// A value as stored: in registers, arrays, hashes, ivars. Code reads it
/// through its view, `heap.kind(v)` (a [`V`]), and builds it with the
/// constructors below; `vm.int(n)` for an Integer that may be large.
///
/// NaN-boxed into 8 bytes. A Float is its own bits (every NaN is stored as
/// the one quiet NaN `0x7FF8…`), which leaves the bit patterns from
/// `0xFFF8 << 48` up for everything else, by the top 16 bits:
///
/// | top 16 bits | kind | low 48 bits |
/// |---|---|---|
/// | `FFF8` | nil, false, true | 0, 1, 2 |
/// | `FFF9` | Integer | the value, signed |
/// | `FFFA` | Symbol | its id |
/// | `FFFB` | heap object | type (3) · generation (13) · slot index (32) |
///
/// An Integer outside 48 bits is a heap object (an `i64` box) whose view
/// is still `V::Int`: that's why reading a value needs the heap.
#[derive(Clone, Copy)]
pub struct Value(u64);

const _: () = assert!(std::mem::size_of::<Value>() == 8);

const TAG_SPECIAL: u64 = 0xFFF8;
const TAG_INT: u64 = 0xFFF9;
const TAG_SYM: u64 = 0xFFFA;
const TAG_HEAP: u64 = 0xFFFB;
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;
const PAYLOAD: u64 = (1 << 48) - 1;
const INT_MIN: i64 = -(1 << 47);
const INT_MAX: i64 = (1 << 47) - 1;

/// Heap object types as packed in a [`Value`] (3 bits).
mod ty {
    pub const STR: u64 = 0;
    pub const ARRAY: u64 = 1;
    pub const HASH: u64 = 2;
    pub const RANGE: u64 = 3;
    pub const PROC: u64 = 4;
    pub const OBJECT: u64 = 5;
    pub const CLASS: u64 = 6;
    pub const INT: u64 = 7;
}

impl Default for Value {
    fn default() -> Self {
        Value::NIL
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Value({:#018x})", self.0)
    }
}

impl<T> Gc<T> {
    #[inline]
    fn pack(self, ty: u64) -> Value {
        Value(
            TAG_HEAP << 48
                | ty << 45
                | (self.generation() as u64) << 32
                | self.idx() as u64,
        )
    }
}

macro_rules! value_from_handles {
    ($($t:ty => $ty:ident),*) => {
        $(impl From<$t> for Value {
            #[inline]
            fn from(h: $t) -> Value {
                h.pack(ty::$ty)
            }
        })*
    };
}

value_from_handles!(StrRef => STR, ArrayRef => ARRAY, HashRef => HASH, RangeRef => RANGE,
    ProcRef => PROC, ClassRef => CLASS, ObjRef => OBJECT);

impl Value {
    pub const NIL: Value = Value(TAG_SPECIAL << 48);
    pub const FALSE: Value = Value(TAG_SPECIAL << 48 | 1);
    pub const TRUE: Value = Value(TAG_SPECIAL << 48 | 2);
    /// A deleted Hash entry's key (src/rhash.rs); never seen outside it.
    pub(crate) const DELETED: Value = Value(0xFFFC << 48);

    #[inline]
    pub(crate) fn is_deleted(self) -> bool {
        self.0 == Value::DELETED.0
    }

    #[inline]
    pub fn bool(b: bool) -> Value {
        if b { Value::TRUE } else { Value::FALSE }
    }

    #[inline]
    pub fn float(f: f64) -> Value {
        Value(if f.is_nan() { CANONICAL_NAN } else { f.to_bits() })
    }

    #[inline]
    pub fn sym(s: Sym) -> Value {
        Value(TAG_SYM << 48 | s.0 as u64)
    }

    /// An Integer known to be small (a length, an index, a character code):
    /// it never needs the heap. Anything else uses `vm.int(n)`.
    #[inline]
    pub fn small_int(n: i64) -> Value {
        Value::inline_int(n).expect("Value::small_int: Integer outside 48 bits")
    }

    /// The Integer `n` if it fits inline (48 bits).
    #[inline]
    pub fn inline_int(n: i64) -> Option<Value> {
        if (INT_MIN..=INT_MAX).contains(&n) {
            Some(Value(TAG_INT << 48 | (n as u64 & PAYLOAD)))
        } else {
            None
        }
    }

    /// The Integer if it is stored inline; the VM's fast paths use this
    /// and leave boxed ones to the slow path.
    #[inline]
    pub fn as_inline_int(self) -> Option<i64> {
        if self.0 >> 48 == TAG_INT {
            Some(((self.0 << 16) as i64) >> 16)
        } else {
            None
        }
    }

    /// The view, without the heap: `Err` is a boxed Integer's handle.
    #[inline]
    pub(crate) fn decode(self) -> Result<V, IntRef> {
        let b = self.0;
        Ok(match b >> 48 {
            TAG_SPECIAL => match b & 3 {
                0 => V::Nil,
                1 => V::Bool(false),
                _ => V::Bool(true),
            },
            TAG_INT => V::Int(((b << 16) as i64) >> 16),
            TAG_SYM => V::Sym(Sym(b as u32)),
            TAG_HEAP => {
                let (idx, generation) = (b as u32, (b >> 32) as u32 & ((1 << GEN_BITS) - 1));
                match (b >> 45) & 7 {
                    ty::STR => V::Str(Gc::from_raw(idx, generation)),
                    ty::ARRAY => V::Array(Gc::from_raw(idx, generation)),
                    ty::HASH => V::Hash(Gc::from_raw(idx, generation)),
                    ty::RANGE => V::Range(Gc::from_raw(idx, generation)),
                    ty::PROC => V::Proc(Gc::from_raw(idx, generation)),
                    ty::OBJECT => V::Object(Gc::from_raw(idx, generation)),
                    ty::CLASS => V::Class(Gc::from_raw(idx, generation)),
                    _ => return Err(Gc::from_raw(idx, generation)),
                }
            }
            _ => V::Float(f64::from_bits(b)),
        })
    }

    /// nil, true, false, an inline Integer or a Symbol: a value equal only
    /// to itself (a boxed Integer is never in inline range), so comparing
    /// bits is enough.
    #[inline]
    pub(crate) fn is_unique(self) -> bool {
        matches!(self.0 >> 48, TAG_SPECIAL | TAG_INT | TAG_SYM)
    }

    /// A Hash (`V::Hash`), by its bits.
    #[inline]
    pub fn is_hash(self) -> bool {
        self.0 >> 45 == (TAG_HEAP << 3 | ty::HASH)
    }

    /// A user object (`V::Object`), by its bits.
    #[inline]
    pub fn is_object(self) -> bool {
        self.0 >> 45 == (TAG_HEAP << 3 | ty::OBJECT)
    }

    #[inline]
    pub fn truthy(&self) -> bool {
        self.0 != Value::NIL.0 && self.0 != Value::FALSE.0
    }

    pub fn is_numeric(&self) -> bool {
        match self.0 >> 48 {
            TAG_INT => true,
            TAG_HEAP => (self.0 >> 45) & 7 == ty::INT,
            t => t < TAG_SPECIAL,
        }
    }

    #[inline]
    pub fn is_nil(&self) -> bool {
        self.0 == Value::NIL.0
    }

    /// Identity as a number (`object_id`, recursion guards); 0 for immediates
    /// (an Integer is one, boxed or not).
    pub fn heap_id(&self) -> u64 {
        match self.decode() {
            Ok(V::Str(h)) => h.id(),
            Ok(V::Array(h)) => h.id(),
            Ok(V::Hash(h)) => h.id(),
            Ok(V::Range(h)) => h.id(),
            Ok(V::Proc(h)) => h.id(),
            Ok(V::Object(h)) => h.id(),
            Ok(V::Class(h)) => h.id(),
            _ => 0,
        }
    }
}

impl Heap {
    /// The view of `v`.
    #[inline]
    pub fn kind(&self, v: Value) -> V {
        match v.decode() {
            Ok(v) => v,
            Err(boxed) => V::Int(self[boxed]),
        }
    }

    /// An Integer value: inline if it fits 48 bits, else boxed here.
    #[inline]
    pub fn int(&mut self, n: i64) -> Value {
        match Value::inline_int(n) {
            Some(v) => v,
            None => self.box_int(n),
        }
    }

    #[cold]
    #[inline(never)]
    fn box_int(&mut self, n: i64) -> Value {
        self.alloc(n).pack(ty::INT)
    }

    /// Identity (`equal?`): Integers and Floats by value.
    pub fn identical(&self, a: &Value, b: &Value) -> bool {
        if a.0 == b.0 {
            // the one NaN isn't identical to itself, as before boxing
            return !(a.0 >> 48 < TAG_SPECIAL && f64::from_bits(a.0).is_nan());
        }
        match (self.kind(*a), self.kind(*b)) {
            (V::Int(x), V::Int(y)) => x == y,
            (V::Float(x), V::Float(y)) => x == y,
            _ => false,
        }
    }
}

/// Structural `==` for built-in types (objects compare by identity here;
/// the VM dispatches user-defined `==` before getting here).
pub fn values_eq(heap: &Heap, a: &Value, b: &Value) -> bool {
    if a.is_unique() && b.is_unique() {
        return a.0 == b.0;
    }
    eq_rec(heap, a, b, &mut Vec::new(), false)
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

/// `strict`: `eql?` (an Integer is never eql? a Float, at any depth), else `==`.
fn eq_rec(heap: &Heap, a: &Value, b: &Value, in_progress: &mut InProgress, strict: bool) -> bool {
    use V::*;
    match (heap.kind(*a), heap.kind(*b)) {
        (Int(x), Float(y)) | (Float(y), Int(x)) => !strict && (x as f64) == y,
        (Str(x), Str(y)) => x == y || *heap[x].borrow() == *heap[y].borrow(),
        (Array(x), Array(y)) => {
            let (xv, yv) = (heap[x].borrow().clone(), heap[y].borrow().clone());
            guarded(x.id(), y.id(), in_progress, |ip| {
                xv.len() == yv.len() && xv.iter().zip(&yv).all(|(p, q)| eq_rec(heap, p, q, ip, strict))
            })
        }
        (Hash(x), Hash(y)) => {
            let (xh, yh) = (heap[x].borrow().clone(), heap[y].borrow().clone());
            guarded(x.id(), y.id(), in_progress, |ip| {
                xh.len() == yh.len()
                    && xh.iter().all(|(k, v)| {
                        yh.get(heap, &k)
                            .is_some_and(|w| eq_rec(heap, &v, &w, ip, strict))
                    })
            })
        }
        (Range(x), Range(y)) => {
            let (x, y) = (&heap[x], &heap[y]);
            eq_rec(heap, &x.0, &y.0, in_progress, strict) && eq_rec(heap, &x.1, &y.1, in_progress, strict) && x.2 == y.2
        }
        _ => heap.identical(a, b),
    }
}

/// `eql?` — hash-key equality (`1.eql?(1.0)` is false).
pub fn eql(heap: &Heap, a: &Value, b: &Value) -> bool {
    // also covers an Integer against a Float: never eql?
    if a.is_unique() || b.is_unique() {
        return a.0 == b.0;
    }
    eq_rec(heap, a, b, &mut Vec::new(), true)
}

/// The hash of a Hash key: values that are [`eql`] hash the same.
///
/// Numbers hash as their `f64` value, since inside containers an Integer
/// and a Float compare with `==` (`(x as f64) == y`). Containers hash their
/// elements down to a fixed depth, which keeps recursive ones finite and
/// is consistent for equal structures.
pub fn hash_key(heap: &Heap, v: &Value) -> u64 {
    let h = hash_rec(heap, v, 0);
    // fmix64 (MurmurHash3): the table indexes with the low bits
    let h = (h ^ (h >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let h = (h ^ (h >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

const HASH_DEPTH: u32 = 3;

fn mix(h: u64, x: u64) -> u64 {
    (h.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95)
}

fn hash_f64(f: f64) -> u64 {
    if f == 0.0 { 0 } else { f.to_bits() }
}

fn hash_rec(heap: &Heap, v: &Value, depth: u32) -> u64 {
    if let Some(n) = v.as_inline_int() {
        return hash_f64(n as f64);
    }
    if v.is_unique() {
        return v.0;
    }
    match heap.kind(*v) {
        V::Int(n) => hash_f64(n as f64),
        V::Float(f) => hash_f64(f),
        V::Str(s) => heap[s]
            .borrow()
            .as_bytes()
            .chunks(8)
            .fold(1, |h, c| {
                let mut w = [0u8; 8];
                w[..c.len()].copy_from_slice(c);
                mix(h, u64::from_le_bytes(w))
            }),
        V::Array(a) => {
            let a = heap[a].borrow();
            let h = mix(2, a.len() as u64);
            if depth == HASH_DEPTH {
                return h;
            }
            a.iter().fold(h, |h, x| mix(h, hash_rec(heap, x, depth + 1)))
        }
        // not by contents: a Hash may be a key of itself, and is borrowed
        // while its own key is hashed (Hash keys only collide with each other)
        V::Hash(_) => 3,
        V::Range(r) => {
            let (lo, hi, excl) = &heap[r];
            mix(mix(mix(4, *excl as u64), hash_rec(heap, lo, depth)), hash_rec(heap, hi, depth))
        }
        _ => v.heap_id(),
    }
}

pub fn num_cmp(heap: &Heap, x: &Value, y: &Value) -> Option<Ordering> {
    match (heap.kind(*x), heap.kind(*y)) {
        (V::Int(a), V::Int(b)) => Some(a.cmp(&b)),
        (V::Int(a), V::Float(b)) => (a as f64).partial_cmp(&b),
        (V::Float(a), V::Int(b)) => a.partial_cmp(&(b as f64)),
        (V::Float(a), V::Float(b)) => a.partial_cmp(&b),
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
