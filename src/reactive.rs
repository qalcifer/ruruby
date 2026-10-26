//! Reactivity (lang-fork-ui): fine-grained, like SolidJS, Slint or QML.
//!
//! A *computation* — an `effect { }` or a `bind(obj, :prop) { }` — runs its
//! block while the VM records which instance variables it reads (the
//! storage of props, read through their readers or directly). Writing one of
//! them later marks the computations that read it; they run again, right
//! after the write or, inside `batch { }`, once at its end. Each run records
//! its reads afresh, so dependencies follow the code's branches.
//!
//! A computation created while another runs belongs to it: it stops when
//! its owner runs again or stops (so an effect made inside an effect
//! doesn't pile up with each run).
//!
//! The cost when nothing is reactive: a read checks one flag
//! ([`Reactive::tracking`]); a write checks the object's `observed` flag,
//! set once a computation has read one of its fields.
//!
//! This file holds the bookkeeping; running computations needs the VM and
//! lives in src/core/reactive.rs.

use crate::symbol::Sym;
use crate::value::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A field: the object's identity (`Gc::id`, unique among live objects)
/// and the instance variable.
pub type Key = (u64, Sym);

pub struct Computation {
    pub block: Value,
    /// A binding: the object and the writer its value goes to (`text=`).
    pub target: Option<(Value, Sym)>,
    pub deps: Vec<Key>,
    /// Computations created during its last run.
    pub children: Vec<usize>,
}

#[derive(Default)]
pub struct Reactive {
    /// Whether reads are recorded now: a computation is running, outside
    /// `untracked { }`.
    pub tracking: Cell<bool>,
    inner: RefCell<Inner>,
}

#[derive(Default)]
struct Inner {
    /// By id, in creation order; `None` once stopped.
    comps: Vec<Option<Computation>>,
    /// The computations that read each field.
    readers: HashMap<Key, Vec<usize>>,
    /// The running computations, innermost last; `None`: `untracked`.
    running: Vec<Option<usize>>,
    /// Computations to run again.
    queue: Vec<usize>,
    batch: u32,
    flushing: bool,
}

impl Reactive {
    /// A field was read: the running computation depends on it.
    pub fn track(&self, key: Key) {
        let mut x = self.inner.borrow_mut();
        let Some(Some(c)) = x.running.last().copied() else { return };
        let Some(Some(comp)) = x.comps.get_mut(c) else { return };
        if comp.deps.contains(&key) {
            return;
        }
        comp.deps.push(key);
        x.readers.entry(key).or_default().push(c);
    }

    /// A field changed: queue what read it. True if anything was queued.
    pub fn changed(&self, key: Key) -> bool {
        let x = &mut *self.inner.borrow_mut();
        let Some(readers) = x.readers.get(&key) else { return false };
        let mut any = false;
        for c in readers {
            if !x.queue.contains(c) {
                x.queue.push(*c);
            }
            any = true;
        }
        any
    }

    pub fn add(&self, block: Value, target: Option<(Value, Sym)>) -> usize {
        let mut x = self.inner.borrow_mut();
        x.comps.push(Some(Computation { block, target, deps: vec![], children: vec![] }));
        let c = x.comps.len() - 1;
        if let Some(Some(owner)) = x.running.last().copied()
            && let Some(Some(o)) = x.comps.get_mut(owner)
        {
            o.children.push(c);
        }
        c
    }

    pub fn stop(&self, c: usize) {
        let mut x = self.inner.borrow_mut();
        Self::stop_in(&mut x, c);
    }

    fn stop_in(x: &mut Inner, c: usize) {
        Self::stop_children(x, c);
        Self::forget_deps(x, c);
        if let Some(slot) = x.comps.get_mut(c) {
            *slot = None;
        }
        x.queue.retain(|q| *q != c);
    }

    fn stop_children(x: &mut Inner, c: usize) {
        let children = match x.comps.get_mut(c) {
            Some(Some(comp)) => std::mem::take(&mut comp.children),
            _ => return,
        };
        for k in children {
            Self::stop_in(x, k);
        }
    }

    pub fn active(&self, c: usize) -> bool {
        matches!(self.inner.borrow().comps.get(c), Some(Some(_)))
    }

    fn forget_deps(x: &mut Inner, c: usize) {
        let deps = match x.comps.get_mut(c) {
            Some(Some(comp)) => std::mem::take(&mut comp.deps),
            _ => return,
        };
        for k in deps {
            if let Some(list) = x.readers.get_mut(&k) {
                list.retain(|r| *r != c);
                if list.is_empty() {
                    x.readers.remove(&k);
                }
            }
        }
    }

    /// Before a run: its block and target, its old dependencies dropped,
    /// reads recorded for it.
    pub fn begin(&self, c: usize) -> Option<(Value, Option<(Value, Sym)>)> {
        let mut x = self.inner.borrow_mut();
        let (block, target) = match x.comps.get(c) {
            Some(Some(comp)) => (comp.block, comp.target),
            _ => return None,
        };
        Self::forget_deps(&mut x, c);
        Self::stop_children(&mut x, c);
        x.running.push(Some(c));
        self.tracking.set(true);
        Some((block, target))
    }

    /// `untracked { }`: reads aren't recorded until [`Reactive::end`].
    pub fn begin_untracked(&self) {
        self.inner.borrow_mut().running.push(None);
        self.tracking.set(false);
    }

    /// After a run (or an untracked section).
    pub fn end(&self) {
        let mut x = self.inner.borrow_mut();
        x.running.pop();
        self.tracking.set(matches!(x.running.last(), Some(Some(_))));
    }

    pub fn begin_batch(&self) {
        self.inner.borrow_mut().batch += 1;
    }

    pub fn end_batch(&self) {
        self.inner.borrow_mut().batch -= 1;
    }

    /// The next queued computation to run: the oldest, so a binding runs
    /// before what was created after it and reads its result; none while a
    /// batch is open.
    pub fn next(&self) -> Option<usize> {
        let mut x = self.inner.borrow_mut();
        if x.batch > 0 {
            return None;
        }
        let (i, _) = x.queue.iter().enumerate().min_by_key(|(_, c)| **c)?;
        Some(x.queue.swap_remove(i))
    }

    /// Starts a flush unless one runs, a batch is open or a computation is
    /// running (its writes wait until it's done): true if the caller should
    /// flush.
    pub fn start_flush(&self) -> bool {
        let mut x = self.inner.borrow_mut();
        if x.flushing || x.batch > 0 || x.running.iter().any(Option::is_some) {
            return false;
        }
        x.flushing = true;
        true
    }

    pub fn end_flush(&self) {
        self.inner.borrow_mut().flushing = false;
    }

    /// For the GC: the values computations hold.
    pub fn roots(&self) -> Vec<Value> {
        let x = self.inner.borrow();
        let mut v = vec![];
        for c in x.comps.iter().flatten() {
            v.push(c.block);
            if let Some((obj, _)) = c.target {
                v.push(obj);
            }
        }
        v
    }

    /// Live computations, and fields being watched (for tests and tools).
    pub fn stats(&self) -> (usize, usize) {
        let x = self.inner.borrow();
        (x.comps.iter().flatten().count(), x.readers.len())
    }
}
