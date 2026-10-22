//! The object heap (mruby: `src/gc.c`).
//!
//! Heap objects live in one arena per type, and a [`Value`] refers to them
//! with a [`Gc`] handle: a slot index plus the slot's generation. Handles are
//! `Copy` and need the [`Heap`] to be read (`vm.heap[h]`), so there is no
//! `unsafe` code: a handle to a slot that has been freed and reused has a
//! stale generation and panics on use instead of reading another object.
//!
//! Classes are not in the heap; they stay reference counted (`Rc<RClass>`).

use crate::value::{Env, RHash, RObject, RProc, Value};
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

impl<T: HeapObj> Gc<T> {
    /// Identity of the object, unique among live objects of all types and
    /// never 0. The low 3 bits hold the type's tag (1..=7), so ids never
    /// equal an 8-aligned address (classes use their `Rc` address).
    pub fn id(self) -> u64 {
        (self.generation as u64) << 40 | (self.idx as u64) << 3 | T::TAG
    }
}

pub type StrRef = Gc<RefCell<String>>;
pub type ArrayRef = Gc<RefCell<Vec<Value>>>;
pub type HashRef = Gc<RefCell<RHash>>;
pub type RangeRef = Gc<(Value, Value, bool)>;
pub type ProcRef = Gc<RProc>;
pub type ObjRef = Gc<RObject>;
pub type EnvRef = Gc<Env>;

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
}

macro_rules! heap_types {
    ($($field:ident: $t:ty = $tag:literal),* $(,)?) => {
        /// All heap objects (mruby: `mrb_gc` and its heap pages).
        #[derive(Default)]
        pub struct Heap {
            $(pub $field: Arena<$t>,)*
        }

        $(impl HeapObj for $t {
            const TAG: u64 = $tag;
            fn arena(h: &Heap) -> &Arena<Self> {
                &h.$field
            }
            fn arena_mut(h: &mut Heap) -> &mut Arena<Self> {
                &mut h.$field
            }
        })*

        impl Heap {
            /// Number of live objects of every type.
            pub fn live(&self) -> usize {
                0 $(+ self.$field.len())*
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
}

impl Heap {
    pub fn alloc<T: HeapObj>(&mut self, v: T) -> Gc<T> {
        T::arena_mut(self).alloc(v)
    }

    pub fn get<T: HeapObj>(&self, h: Gc<T>) -> &T {
        T::arena(self).get(h)
    }
}

impl<T: HeapObj> Index<Gc<T>> for Heap {
    type Output = T;

    fn index(&self, h: Gc<T>) -> &T {
        self.get(h)
    }
}

/// So `heap[h]` also works with the `&Gc` a `match` on `&Value` binds.
impl<T: HeapObj> Index<&Gc<T>> for Heap {
    type Output = T;

    fn index(&self, h: &Gc<T>) -> &T {
        self.get(*h)
    }
}
