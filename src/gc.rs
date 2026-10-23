//! The object heap (mruby: `src/gc.c`).
//!
//! Heap objects live in one arena per type, and a [`Value`] refers to them
//! with a [`Gc`] handle: a slot index plus the slot's generation. Handles are
//! `Copy` and need the [`Heap`] to be read (`vm.heap[h]`), so there is no
//! `unsafe` code: a handle to a slot that has been freed and reused has a
//! stale generation and panics on use instead of reading another object.
//!
//! Classes and modules are heap objects too, so a `Value` is `Copy`.
//!
//! Collection is a stop-the-world mark-sweep (mruby's is incremental). It
//! only runs at *safe points*: between instructions of the outermost
//! dispatch loop of [`Vm::run_main`](crate::vm::Vm::run_main), when no
//! native method or VM helper is on the Rust stack. So the roots are only
//! the VM's own state (frames, globals, classes), and native code never
//! needs to protect the values it holds (mruby's "GC arena").

use crate::value::{Env, EnvRegs, Method, ProcBody, RClass, RHash, RObject, RProc, V, Value};
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Index;

/// A handle to a heap object of type `T`.
pub struct Gc<T> {
    idx: u32,
    generation: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Gc<T> {}

impl<T> PartialEq for Gc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx && self.generation == other.generation
    }
}

impl<T> Eq for Gc<T> {}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Gc({}:{})", self.idx, self.generation)
    }
}

/// Generations wrap at 2^13: a handle in a [`Value`] has 13 bits for it
/// (next to 32 bits of index and 3 of type). A stale handle is still caught
/// unless its slot was reused a multiple of 8,192 times since.
pub const GEN_BITS: u32 = 13;
const GEN_MASK: u32 = (1 << GEN_BITS) - 1;

impl<T> Gc<T> {
    /// The handle a [`Value`] packed as `(idx, generation)`.
    #[inline]
    pub(crate) fn from_raw(idx: u32, generation: u32) -> Self {
        Gc {
            idx,
            generation,
            _t: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn idx(self) -> u32 {
        self.idx
    }

    #[inline]
    pub(crate) fn generation(self) -> u32 {
        self.generation
    }
}

impl<T: HeapObj> Gc<T> {
    /// Identity of the object, unique among live objects of all types and
    /// never 0. The low 4 bits hold the type's tag (1..=15).
    pub fn id(self) -> u64 {
        (self.generation as u64) << 40 | (self.idx as u64) << 4 | T::TAG
    }
}

pub type StrRef = Gc<RefCell<String>>;
pub type ArrayRef = Gc<RefCell<Vec<Value>>>;
pub type HashRef = Gc<RefCell<RHash>>;
pub type RangeRef = Gc<(Value, Value, bool)>;
pub type ProcRef = Gc<RProc>;
pub type ObjRef = Gc<RObject>;
pub type EnvRef = Gc<Env>;
pub type ClassRef = Gc<RClass>;
/// An Integer that doesn't fit a [`Value`]'s 48 bits.
pub type IntRef = Gc<i64>;

struct Slot<T> {
    generation: u32,
    val: Option<T>,
}

/// The slots of one object type.
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Arena {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Arena<T> {
    fn alloc(&mut self, v: T) -> Gc<T> {
        let idx = match self.free.pop() {
            Some(i) => {
                let s = &mut self.slots[i as usize];
                s.val = Some(v);
                i
            }
            None => {
                self.slots.push(Slot { generation: 0, val: Some(v) });
                (self.slots.len() - 1) as u32
            }
        };
        Gc {
            idx,
            generation: self.slots[idx as usize].generation,
            _t: PhantomData,
        }
    }

    #[inline]
    fn get(&self, h: Gc<T>) -> &T {
        match self.slots.get(h.idx as usize) {
            Some(Slot { generation, val: Some(v) }) if *generation == h.generation => v,
            _ => panic!("GC: use of a freed object {h:?} (a missing root)"),
        }
    }

    /// Number of live objects.
    pub fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    /// Frees every live slot that isn't marked; returns how many.
    fn sweep(&mut self, marks: &[bool]) -> usize {
        let mut freed = 0;
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.val.is_some() && !marks.get(i).copied().unwrap_or(false) {
                slot.val = None;
                slot.generation = (slot.generation + 1) & GEN_MASK;
                self.free.push(i as u32);
                freed += 1;
            }
        }
        freed
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Types that live in the heap, each in its own arena.
pub trait HeapObj: Sized {
    /// Distinguishes ids of different types (see [`Gc::id`]).
    const TAG: u64;
    fn arena(h: &Heap) -> &Arena<Self>;
    fn arena_mut(h: &mut Heap) -> &mut Arena<Self>;
    fn marks(m: &mut Marks) -> &mut Vec<bool>;
}

macro_rules! heap_types {
    ($($field:ident: $t:ty = $tag:literal),* $(,)?) => {
        /// All heap objects (mruby: `mrb_gc` and its heap pages).
        #[derive(Default)]
        pub struct Heap {
            $(pub $field: Arena<$t>,)*
            pub gc: GcState,
        }

        /// Mark bits of one collection, per arena.
        pub struct Marks {
            $($field: Vec<bool>,)*
        }

        $(impl HeapObj for $t {
            const TAG: u64 = $tag;
            fn arena(h: &Heap) -> &Arena<Self> {
                &h.$field
            }
            fn arena_mut(h: &mut Heap) -> &mut Arena<Self> {
                &mut h.$field
            }
            fn marks(m: &mut Marks) -> &mut Vec<bool> {
                &mut m.$field
            }
        })*

        impl Heap {
            /// Number of live objects of every type.
            pub fn live(&self) -> usize {
                0 $(+ self.$field.len())*
            }

            fn new_marks(&self) -> Marks {
                Marks {
                    $($field: vec![false; self.$field.slots.len()],)*
                }
            }

            /// Frees every object that `marks` doesn't mark.
            pub fn sweep(&mut self, marks: Marks) {
                let freed = 0 $(+ self.$field.sweep(&marks.$field))*;
                let live = self.live();
                let gc = &mut self.gc;
                gc.count += 1;
                gc.freed += freed;
                gc.budget = if gc.stress { 0 } else { live.max(MIN_THRESHOLD) as isize };
            }
        }
    };
}

heap_types! {
    strings: RefCell<String> = 1,
    arrays: RefCell<Vec<Value>> = 2,
    hashes: RefCell<RHash> = 3,
    ranges: (Value, Value, bool) = 4,
    procs: RProc = 5,
    objects: RObject = 6,
    envs: Env = 7,
    classes: RClass = 8,
    ints: i64 = 9,
}

/// A collection runs once this many objects were allocated since the last
/// one, or as many as were live after it if that is more (so the heap may
/// grow to about twice its live size).
const MIN_THRESHOLD: usize = 10_000;

/// Collection policy and counters (mruby: the fields of `mrb_gc`).
pub struct GcState {
    /// Allocations left before the next collection is due; checked at
    /// every safe point, so it is a single counter.
    budget: isize,
    /// `GC.disable` turns collection off.
    pub enabled: bool,
    /// `GC.stress = true`: collect at every safe point (finds missing roots).
    stress: bool,
    /// Number of collections so far (`GC.count`).
    pub count: usize,
    /// Objects freed so far.
    pub freed: usize,
}

impl Default for GcState {
    fn default() -> Self {
        GcState {
            budget: MIN_THRESHOLD as isize,
            enabled: true,
            stress: false,
            count: 0,
            freed: 0,
        }
    }
}

impl GcState {
    #[inline]
    pub fn due(&self) -> bool {
        self.budget <= 0 && self.enabled
    }

    /// `GC.start`: collect at the next safe point.
    pub fn request(&mut self) {
        self.budget = 0;
    }

    pub fn stress(&self) -> bool {
        self.stress
    }

    pub fn set_stress(&mut self, on: bool) {
        self.stress = on;
        if on {
            self.budget = 0;
        }
    }
}

impl Heap {
    pub fn alloc<T: HeapObj>(&mut self, v: T) -> Gc<T> {
        self.gc.budget -= 1;
        T::arena_mut(self).alloc(v)
    }

    #[inline]
    pub fn get<T: HeapObj>(&self, h: Gc<T>) -> &T {
        T::arena(self).get(h)
    }
}

impl<T: HeapObj> Index<Gc<T>> for Heap {
    type Output = T;

    #[inline]
    fn index(&self, h: Gc<T>) -> &T {
        self.get(h)
    }
}

/// So `heap[h]` also works with the `&Gc` a `match` on `&Value` binds.
impl<T: HeapObj> Index<&Gc<T>> for Heap {
    type Output = T;

    #[inline]
    fn index(&self, h: &Gc<T>) -> &T {
        self.get(*h)
    }
}

enum Gray {
    Val(Value),
    Env(EnvRef),
    Class(ClassRef),
}

/// The mark phase: roots are added with [`Marker::value`] & co, then
/// [`Marker::finish`] traces everything reachable. Tracing uses an explicit
/// work list, not recursion, so deep structures don't need a deep stack.
pub struct Marker<'h> {
    heap: &'h Heap,
    marks: Marks,
    gray: Vec<Gray>,
}

impl<'h> Marker<'h> {
    pub fn new(heap: &'h Heap) -> Self {
        Marker {
            heap,
            marks: heap.new_marks(),
            gray: Vec::new(),
        }
    }

    pub fn value(&mut self, v: &Value) {
        match v.decode() {
            Ok(V::Nil | V::Bool(_) | V::Int(_) | V::Float(_) | V::Sym(_)) => {}
            Ok(V::Class(c)) => self.class(c),
            Ok(_) => self.gray.push(Gray::Val(*v)),
            Err(boxed) => {
                self.visit(boxed);
            }
        }
    }

    pub fn env(&mut self, e: EnvRef) {
        self.gray.push(Gray::Env(e));
    }

    pub fn class(&mut self, c: ClassRef) {
        self.gray.push(Gray::Class(c));
    }

    /// Marks everything reachable from the roots given so far.
    pub fn finish(mut self) -> Marks {
        while let Some(g) = self.gray.pop() {
            self.trace(g);
        }
        self.marks
    }

    /// Marks `h`; true the first time.
    fn visit<T: HeapObj>(&mut self, h: Gc<T>) -> bool {
        let bit = &mut T::marks(&mut self.marks)[h.idx as usize];
        !std::mem::replace(bit, true)
    }

    fn trace(&mut self, g: Gray) {
        let heap = self.heap;
        match g {
            Gray::Val(v) => match heap.kind(v) {
                V::Str(h) => {
                    self.visit(h);
                }
                V::Array(h) => {
                    if self.visit(h) {
                        for x in heap[h].borrow().iter() {
                            self.value(x);
                        }
                    }
                }
                V::Hash(h) => {
                    if self.visit(h) {
                        let h = heap[h].borrow();
                        for (k, v) in h.pairs() {
                            self.value(k);
                            self.value(v);
                        }
                        self.value(&h.default);
                        if let Some(p) = &h.default_proc {
                            self.value(p);
                        }
                    }
                }
                V::Range(h) => {
                    if self.visit(h) {
                        let (lo, hi, _) = &heap[h];
                        self.value(lo);
                        self.value(hi);
                    }
                }
                V::Proc(h) => {
                    if self.visit(h) {
                        self.proc(&heap[h]);
                    }
                }
                V::Object(h) => {
                    if self.visit(h) {
                        let o = &heap[h];
                        self.class(o.class);
                        for (_, v) in &o.ivars.borrow().items {
                            self.value(v);
                        }
                        if let Some(s) = o.singleton.get() {
                            self.class(s);
                        }
                    }
                }
                V::Nil
                | V::Bool(_)
                | V::Int(_)
                | V::Float(_)
                | V::Sym(_)
                | V::Class(_) => unreachable!("not queued"),
            },
            Gray::Env(e) => {
                if self.visit(e) {
                    let e = &heap[e];
                    // while on the stack the values are marked as stack roots
                    if let EnvRegs::Heap(vals) = &*e.regs.borrow() {
                        for v in vals {
                            self.value(v);
                        }
                    }
                    if let Some(up) = e.upper {
                        self.env(up);
                    }
                }
            }
            Gray::Class(h) => {
                if !self.visit(h) {
                    return;
                }
                let c = &heap[h];
                for k in c.superclass.iter().chain(c.meta.get().iter()) {
                    self.class(*k);
                }
                for m in c.includes.borrow().iter().chain(c.prepends.borrow().iter()) {
                    self.class(*m);
                }
                for k in c.attached.get().iter().chain(c.outer.get().iter()) {
                    self.class(*k);
                }
                for v in c.consts.borrow().values() {
                    self.value(v);
                }
                for (_, v) in c.ivars.borrow().items.iter().chain(&c.cvars.borrow().items) {
                    self.value(v);
                }
                for m in c.methods.borrow().values() {
                    if let Method::Proc(p) = m {
                        self.value(p);
                    }
                }
            }
        }
    }

    fn proc(&mut self, p: &RProc) {
        self.value(&p.self_);
        if let Some(b) = &p.block {
            self.value(b);
        }
        if let Some((owner, _)) = p.method {
            self.class(owner);
        }
        self.class(p.target);
        match &p.body {
            ProcBody::Ruby { env, .. } => self.env(*env),
            ProcBody::Collect(buf) => self.value(&Value::from(*buf)),
            ProcBody::SymCall(_) => {}
        }
    }
}
