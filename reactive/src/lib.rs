//! Fine-grained reactivity with push-pull updates, for ruruby and for Rust.
//!
//! ```
//! use ruruby_reactive::Runtime;
//!
//! let rt = Runtime::new();
//! let count = rt.signal(1);
//! let doubled = rt.memo({ let count = count.clone(); move || count.get() * 2 });
//! let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
//! let _e = rt.effect({
//!     let (doubled, seen) = (doubled.clone(), seen.clone());
//!     move || seen.borrow_mut().push(doubled.get())
//! });
//! count.set(5);
//! rt.batch(|| { count.set(6); count.set(7); });
//! assert_eq!(*seen.borrow(), [2, 10, 14]);
//! ```
//!
//! # The graph
//!
//! *Sources* hold state: [`Signal`]s, and *keyed* sources an embedder names
//! with a `u128` (ruruby: an object's field). *Computations* read them:
//! [`Memo`]s (a cached value, recomputed lazily), *effects* (run for their
//! side effects) and *bindings* (an effect whose result is written to a keyed
//! source, so what reads that source depends on the binding).
//!
//! Reading a source or memo while a computation runs records an edge.
//! Writing a source marks what read it *Dirty* and everything further down
//! *Check* (push); effects and bindings marked are queued. A flush then
//! brings each queued computation up to date (pull): a *Check* one first
//! updates the memos and bindings it read, oldest first, and only runs if
//! one of them really changed. So each computation runs at most once per
//! change, after everything it depends on (no glitches), and memos nobody
//! reads aren't computed.
//!
//! Computations made while another runs belong to it: they're disposed when
//! their owner runs again or is disposed. [`Runtime::on_cleanup`] registers
//! code to run then.
//!
//! # Embedding
//!
//! An interpreter's computations are *external*: the runtime doesn't call
//! them, it asks the embedder to ([`Runtime::flush_with`] takes the function
//! that runs one, by node and tag). Writes made while a computation runs, or
//! inside [`Runtime::batch`], wait for the end of it. Single-threaded, no
//! dependencies, wasm-friendly.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// A node of the graph. Stale ids (of disposed nodes) are recognized.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    idx: u32,
    generation: u32,
}

/// How often one computation may run in one flush before it's a cycle.
pub const MAX_RUNS: usize = 100;

#[derive(Debug)]
pub enum FlushError<E> {
    /// The embedder's computation failed.
    External(E),
    /// A computation kept being invalidated by its own effects.
    Cycle(NodeId),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum State {
    Clean,
    Check,
    Dirty,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Source,
    Memo,
    Effect,
    Binding,
}

enum Func {
    None,
    /// Runs the computation; a memo returns whether its value changed.
    Rust(Box<dyn FnMut() -> bool>),
    /// The embedder's, by tag.
    External(u64),
}

struct Node {
    generation: u32,
    alive: bool,
    kind: Kind,
    state: State,
    func: Option<Func>,
    /// What a computation read in its last run, in order.
    sources: Vec<NodeId>,
    /// The computations that read this source or memo.
    observers: Vec<NodeId>,
    /// A keyed source a binding writes: the binding.
    producer: Option<NodeId>,
    /// A binding's keyed source.
    output: Option<NodeId>,
    owner: Option<NodeId>,
    children: Vec<NodeId>,
    cleanups: Vec<Box<dyn FnOnce()>>,
    key: Option<u128>,
    runs: usize,
}

impl Node {
    fn new(kind: Kind, generation: u32) -> Node {
        Node {
            generation,
            alive: true,
            kind,
            state: State::Clean,
            func: Some(Func::None),
            sources: vec![],
            observers: vec![],
            producer: None,
            output: None,
            owner: None,
            children: vec![],
            cleanups: vec![],
            key: None,
            runs: 0,
        }
    }
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    free: Vec<u32>,
    keyed: HashMap<u128, NodeId>,
    /// The computations running, innermost last; `None`: untracked.
    running: Vec<Option<NodeId>>,
    queue: VecDeque<NodeId>,
    batch: u32,
    flushing: bool,
    /// Rust code wrote, but an external computation is waiting.
    external_pending: bool,
}

impl Graph {
    fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.idx as usize).filter(|n| n.alive && n.generation == id.generation)
    }

    fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.idx as usize).filter(|n| n.alive && n.generation == id.generation)
    }

    fn alloc(&mut self, kind: Kind) -> NodeId {
        let owner = self.running.last().copied().flatten();
        let id = match self.free.pop() {
            Some(idx) => {
                let generation = self.nodes[idx as usize].generation.wrapping_add(1);
                self.nodes[idx as usize] = Node::new(kind, generation);
                NodeId { idx, generation }
            }
            None => {
                self.nodes.push(Node::new(kind, 0));
                NodeId { idx: self.nodes.len() as u32 - 1, generation: 0 }
            }
        };
        if kind != Kind::Source
            && let Some(o) = owner
            && let Some(on) = self.get_mut(o)
        {
            on.children.push(id);
            self.nodes[id.idx as usize].owner = Some(o);
        }
        id
    }

    fn keyed_source(&mut self, key: u128) -> NodeId {
        if let Some(id) = self.keyed.get(&key) {
            return *id;
        }
        let id = self.alloc(Kind::Source);
        self.nodes[id.idx as usize].key = Some(key);
        self.keyed.insert(key, id);
        id
    }

    /// A keyed source nobody reads or writes any more goes away.
    fn maybe_drop_source(&mut self, s: NodeId) {
        let Some(n) = self.get(s) else { return };
        if n.kind == Kind::Source
            && n.observers.is_empty()
            && n.producer.is_none()
            && let Some(key) = n.key
        {
            self.keyed.remove(&key);
            self.free_node(s);
        }
    }

    fn free_node(&mut self, id: NodeId) {
        if let Some(n) = self.get_mut(id) {
            n.alive = false;
            n.func = None;
            n.sources.clear();
            n.observers.clear();
            n.children.clear();
            self.free.push(id.idx);
        }
    }

    fn unsubscribe(&mut self, c: NodeId) {
        let sources = match self.get_mut(c) {
            Some(n) => std::mem::take(&mut n.sources),
            None => return,
        };
        for s in sources {
            if let Some(sn) = self.get_mut(s) {
                sn.observers.retain(|o| *o != c);
            }
            self.maybe_drop_source(s);
        }
    }

    /// Where the value of source `s` comes from: itself if it's a memo, its
    /// binding if one writes it.
    fn upstream(&self, s: NodeId) -> Option<NodeId> {
        let n = self.get(s)?;
        match n.kind {
            Kind::Memo => Some(s),
            Kind::Source => n.producer,
            _ => None,
        }
    }

    /// What depends on computation `c`'s result.
    fn downstream(&self, c: NodeId) -> Vec<NodeId> {
        match self.get(c) {
            Some(n) if n.kind == Kind::Memo => n.observers.clone(),
            Some(n) if n.kind == Kind::Binding => n.output.and_then(|o| self.get(o)).map(|o| o.observers.clone()).unwrap_or_default(),
            _ => vec![],
        }
    }

    fn mark(&mut self, c: NodeId, state: State) {
        let Some(n) = self.get_mut(c) else { return };
        if n.state >= state {
            return;
        }
        let was_clean = n.state == State::Clean;
        n.state = state;
        if was_clean && matches!(n.kind, Kind::Effect | Kind::Binding) {
            self.queue.push_back(c);
        }
        for o in self.downstream(c) {
            self.mark(o, State::Check);
        }
    }

    fn write(&mut self, s: NodeId) -> bool {
        let observers = match self.get(s) {
            Some(n) => n.observers.clone(),
            None => return false,
        };
        for o in &observers {
            self.mark(*o, State::Dirty);
        }
        !observers.is_empty()
    }

    fn track(&mut self, s: NodeId) {
        let Some(Some(c)) = self.running.last().copied() else { return };
        if c == s || self.get(s).is_none() {
            return;
        }
        let Some(cn) = self.get_mut(c) else { return };
        if cn.sources.contains(&s) {
            return;
        }
        cn.sources.push(s);
        self.get_mut(s).unwrap().observers.push(c);
    }

    fn can_flush(&self) -> bool {
        self.batch == 0 && !self.flushing && !self.running.iter().any(Option::is_some)
    }
}

/// The reactive runtime: a cheap handle (clones share it).
#[derive(Clone, Default)]
pub struct Runtime(Rc<Inner>);

#[derive(Default)]
struct Inner {
    graph: RefCell<Graph>,
    /// A computation is running (outside `untracked`): reads are recorded.
    tracking: Cell<bool>,
}

/// State: `get` records a dependency, `set` notifies what depends on it.
pub struct Signal<T> {
    rt: Runtime,
    id: NodeId,
    value: Rc<RefCell<T>>,
}

/// A value computed from others, cached and recomputed when they change
/// (lazily: only when read, or needed by an effect).
pub struct Memo<T> {
    rt: Runtime,
    id: NodeId,
    value: Rc<RefCell<Option<T>>>,
}

/// A computation run for its side effects; [`Effect::dispose`] ends it.
#[derive(Clone)]
pub struct Effect {
    rt: Runtime,
    id: NodeId,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal { rt: self.rt.clone(), id: self.id, value: self.value.clone() }
    }
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Memo { rt: self.rt.clone(), id: self.id, value: self.value.clone() }
    }
}

impl<T: 'static> Signal<T> {
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.rt.track(self.id);
        self.value.borrow().clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.rt.track(self.id);
        f(&self.value.borrow())
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.value.borrow().clone()
    }

    /// Stores `v`; notifies if it differs from the value.
    pub fn set(&self, v: T)
    where
        T: PartialEq,
    {
        if *self.value.borrow() == v {
            return;
        }
        *self.value.borrow_mut() = v;
        self.rt.notify(self.id);
    }

    /// Changes the value in place; always notifies.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.value.borrow_mut());
        self.rt.notify(self.id);
    }

    pub fn id(&self) -> NodeId {
        self.id
    }
}

impl<T: Clone + 'static> Memo<T> {
    pub fn get(&self) -> T {
        self.rt.refresh(self.id);
        self.rt.track(self.id);
        self.value.borrow().clone().expect("memo computed")
    }

    pub fn get_untracked(&self) -> T {
        self.rt.refresh(self.id);
        self.value.borrow().clone().expect("memo computed")
    }

    pub fn id(&self) -> NodeId {
        self.id
    }
}

impl Effect {
    pub fn dispose(&self) {
        self.rt.dispose(self.id);
    }

    pub fn is_alive(&self) -> bool {
        self.rt.is_alive(self.id)
    }

    pub fn id(&self) -> NodeId {
        self.id
    }
}

/// Runs external computations in a Rust-only flush: none can run.
struct NoExternal;

impl Runtime {
    pub fn new() -> Runtime {
        Runtime::default()
    }

    fn g(&self) -> std::cell::RefMut<'_, Graph> {
        self.0.graph.borrow_mut()
    }

    /// Whether reads are recorded now (a fast check for embedders).
    #[inline]
    pub fn tracking(&self) -> bool {
        self.0.tracking.get()
    }

    fn set_tracking(&self, g: &Graph) {
        self.0.tracking.set(matches!(g.running.last(), Some(Some(_))));
    }

    // ---- the Rust API --------------------------------------------------

    pub fn signal<T: 'static>(&self, v: T) -> Signal<T> {
        let id = self.g().alloc(Kind::Source);
        Signal { rt: self.clone(), id, value: Rc::new(RefCell::new(v)) }
    }

    pub fn memo<T: Clone + PartialEq + 'static>(&self, mut f: impl FnMut() -> T + 'static) -> Memo<T> {
        let id = self.g().alloc(Kind::Memo);
        let value: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let slot = value.clone();
        let func = move || {
            let v = f();
            let changed = slot.borrow().as_ref() != Some(&v);
            if changed {
                *slot.borrow_mut() = Some(v);
            }
            changed
        };
        {
            let mut g = self.g();
            let n = g.get_mut(id).unwrap();
            n.func = Some(Func::Rust(Box::new(func)));
            n.state = State::Dirty;
        }
        Memo { rt: self.clone(), id, value }
    }

    /// Runs `f` now and again whenever what it read changes.
    pub fn effect(&self, mut f: impl FnMut() + 'static) -> Effect {
        let id = self.g().alloc(Kind::Effect);
        self.g().get_mut(id).unwrap().func = Some(Func::Rust(Box::new(move || {
            f();
            false
        })));
        let _ = self.run_now(id, &mut |_, _| Err(NoExternal));
        self.auto_flush();
        Effect { rt: self.clone(), id }
    }

    /// Runs `f` with updates held until it returns.
    pub fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.begin_batch();
        let r = f();
        self.end_batch();
        self.auto_flush();
        r
    }

    /// Runs `f` without recording its reads.
    pub fn untracked<R>(&self, f: impl FnOnce() -> R) -> R {
        self.begin_untracked();
        let r = f();
        self.end_untracked();
        r
    }

    /// Code to run when the running computation runs again or is disposed.
    pub fn on_cleanup(&self, f: impl FnOnce() + 'static) {
        let mut g = self.g();
        if let Some(Some(c)) = g.running.last().copied()
            && let Some(n) = g.get_mut(c)
        {
            n.cleanups.push(Box::new(f));
        }
    }

    /// Runs what's queued (Rust computations; external ones wait for the
    /// embedder's [`Runtime::flush_with`]).
    pub fn flush(&self) {
        let _ = self.flush_with(&mut |_, _| Err(NoExternal));
    }

    fn auto_flush(&self) {
        if self.g().can_flush() {
            self.flush();
        }
    }

    // ---- the embedding API ----------------------------------------------

    /// Records a read of `id` by the running computation.
    pub fn track(&self, id: NodeId) {
        if self.tracking() {
            self.g().track(id);
        }
    }

    /// Records a read of keyed source `key` (made on first read).
    pub fn track_key(&self, key: u128) {
        if !self.tracking() {
            return;
        }
        let mut g = self.g();
        let s = g.keyed_source(key);
        g.track(s);
    }

    fn notify(&self, id: NodeId) {
        self.g().write(id);
        self.auto_flush();
    }

    /// Keyed source `key` changed: marks what depends on it. True if
    /// something is queued (the embedder then flushes: [`Runtime::should_flush`]).
    pub fn write_key(&self, key: u128) -> bool {
        let mut g = self.g();
        match g.keyed.get(&key).copied() {
            Some(s) => g.write(s),
            None => false,
        }
    }

    /// Whether a flush would run now (no batch open, no flush or
    /// computation running) and anything is queued.
    pub fn should_flush(&self) -> bool {
        let g = self.g();
        g.can_flush() && !g.queue.is_empty()
    }

    /// An external effect (`output: None`) or binding (`Some(key)`: its
    /// result goes to that keyed source). Not run yet: see [`Runtime::run_now`].
    pub fn external(&self, tag: u64, output: Option<u128>) -> NodeId {
        let mut g = self.g();
        let kind = if output.is_some() { Kind::Binding } else { Kind::Effect };
        let id = g.alloc(kind);
        g.get_mut(id).unwrap().func = Some(Func::External(tag));
        if let Some(key) = output {
            let s = g.keyed_source(key);
            if let Some(old) = g.get(s).and_then(|n| n.producer) {
                // a new binding of the field replaces the old one
                drop(g);
                self.dispose(old);
                g = self.g();
            }
            g.get_mut(s).unwrap().producer = Some(id);
            g.get_mut(id).unwrap().output = Some(s);
        }
        id
    }

    /// Runs computation `id` now (its first run).
    pub fn run_now<E>(&self, id: NodeId, exec: &mut dyn FnMut(NodeId, u64) -> Result<(), E>) -> Result<(), E> {
        self.run(id, exec)
    }

    pub fn begin_batch(&self) {
        self.g().batch += 1;
    }

    pub fn end_batch(&self) {
        let mut g = self.g();
        g.batch = g.batch.saturating_sub(1);
    }

    pub fn begin_untracked(&self) {
        let mut g = self.g();
        g.running.push(None);
        self.set_tracking(&g);
    }

    pub fn end_untracked(&self) {
        let mut g = self.g();
        g.running.pop();
        self.set_tracking(&g);
    }

    pub fn is_alive(&self, id: NodeId) -> bool {
        self.0.graph.borrow().get(id).is_some()
    }

    /// The tag of a live external computation.
    pub fn tag(&self, id: NodeId) -> Option<u64> {
        match self.0.graph.borrow().get(id)?.func {
            Some(Func::External(t)) => Some(t),
            _ => None,
        }
    }

    /// The tags of the live external computations (for an embedder's GC).
    pub fn external_tags(&self) -> Vec<u64> {
        let g = self.0.graph.borrow();
        g.nodes
            .iter()
            .filter(|n| n.alive)
            .filter_map(|n| match n.func {
                Some(Func::External(t)) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// (live computations, keyed sources being read or written).
    pub fn stats(&self) -> (usize, usize) {
        let g = self.0.graph.borrow();
        let comps = g.nodes.iter().filter(|n| n.alive && n.kind != Kind::Source).count();
        (comps, g.keyed.len())
    }

    /// Ends a computation, its children first, running their cleanups.
    pub fn dispose(&self, id: NodeId) {
        let (children, cleanups, owner, output) = {
            let mut g = self.g();
            let Some(n) = g.get_mut(id) else { return };
            (std::mem::take(&mut n.children), std::mem::take(&mut n.cleanups), n.owner, n.output)
        };
        for c in children {
            self.dispose(c);
        }
        for f in cleanups {
            f();
        }
        let mut g = self.g();
        g.unsubscribe(id);
        if let Some(o) = owner
            && let Some(on) = g.get_mut(o)
        {
            on.children.retain(|c| *c != id);
        }
        if let Some(s) = output {
            if let Some(sn) = g.get_mut(s)
                && sn.producer == Some(id)
            {
                sn.producer = None;
            }
            g.maybe_drop_source(s);
        }
        // observers of a memo lose it as a source
        let observers = g.get(id).map(|n| n.observers.clone()).unwrap_or_default();
        for o in observers {
            if let Some(on) = g.get_mut(o) {
                on.sources.retain(|s| *s != id);
            }
        }
        g.queue.retain(|q| *q != id);
        g.free_node(id);
    }

    /// Runs one computation: its children disposed, cleanups run, reads
    /// recorded afresh.
    fn run<E>(&self, id: NodeId, exec: &mut dyn FnMut(NodeId, u64) -> Result<(), E>) -> Result<(), E> {
        let (children, cleanups) = {
            let mut g = self.g();
            let Some(n) = g.get_mut(id) else { return Ok(()) };
            (std::mem::take(&mut n.children), std::mem::take(&mut n.cleanups))
        };
        for c in children {
            self.dispose(c);
        }
        for f in cleanups {
            f();
        }
        let func = {
            let mut g = self.g();
            g.unsubscribe(id);
            let n = g.get_mut(id).unwrap();
            // clean now: writes during the run that affect it queue it again
            n.state = State::Clean;
            n.runs += 1;
            let f = n.func.take();
            g.running.push(Some(id));
            self.set_tracking(&g);
            f
        };
        let (func, result) = match func {
            Some(Func::Rust(mut f)) => {
                let changed = f();
                (Some(Func::Rust(f)), Ok(changed))
            }
            Some(Func::External(tag)) => (Some(Func::External(tag)), exec(id, tag).map(|_| false)),
            other => (other, Ok(false)),
        };
        let mut g = self.g();
        g.running.pop();
        self.set_tracking(&g);
        let kind = g.get(id).map(|n| n.kind);
        if let Some(n) = g.get_mut(id) {
            n.func = func;
        }
        match result {
            Ok(true) if kind == Some(Kind::Memo) => {
                for o in g.downstream(id) {
                    g.mark(o, State::Dirty);
                }
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => {
                // didn't finish: still to do
                if let Some(n) = g.get_mut(id) {
                    n.state = State::Dirty;
                }
                Err(e)
            }
        }
    }

    /// Brings `start` up to date: what it depends on first (pull).
    fn update<E>(&self, start: NodeId, exec: &mut dyn FnMut(NodeId, u64) -> Result<(), E>) -> Result<(), FlushError<E>> {
        let mut stack = vec![start];
        while let Some(&top) = stack.last() {
            let step = {
                let g = self.0.graph.borrow();
                match g.get(top) {
                    None => None,
                    Some(n) => match n.state {
                        State::Clean => None,
                        State::Dirty => Some(Err(())),
                        State::Check => {
                            // the first source whose producer isn't up to date
                            let stale = n
                                .sources
                                .iter()
                                .filter_map(|s| g.upstream(*s))
                                .find(|p| g.get(*p).is_some_and(|pn| pn.state != State::Clean) && !stack.contains(p));
                            Some(Ok(stale))
                        }
                    },
                }
            };
            match step {
                // clean (or gone)
                None => _ = stack.pop(),
                // check: an input to update first, or none: clean
                Some(Ok(Some(p))) => stack.push(p),
                Some(Ok(None)) => {
                    if let Some(n) = self.g().get_mut(top) {
                        n.state = State::Clean;
                    }
                    stack.pop();
                }
                // dirty: run it
                Some(Err(())) => {
                    let runs = self.0.graph.borrow().get(top).map_or(0, |n| n.runs);
                    if runs >= MAX_RUNS {
                        return Err(FlushError::Cycle(top));
                    }
                    self.run(top, exec).map_err(FlushError::External)?;
                    // a run that invalidated itself is queued again; move on
                    stack.pop();
                }
            }
        }
        Ok(())
    }

    /// A memo read outside a flush: updated if stale (Rust computations
    /// only; an external input can't run here).
    fn refresh(&self, id: NodeId) {
        let stale = self.0.graph.borrow().get(id).is_some_and(|n| n.state != State::Clean);
        if stale && self.update(id, &mut |_, _| Err(NoExternal)).is_err() {
            self.g().external_pending = true;
        }
    }

    /// Runs the queued computations (and what they need first), the
    /// embedder's through `exec`, until none are queued — unless a batch is
    /// open, a flush is running, or a computation is running (they flush
    /// when they end).
    pub fn flush_with<E>(&self, exec: &mut dyn FnMut(NodeId, u64) -> Result<(), E>) -> Result<(), FlushError<E>> {
        {
            let mut g = self.g();
            if !g.can_flush() {
                return Ok(());
            }
            g.flushing = true;
        }
        let r = self.flush_queue(exec);
        let mut g = self.g();
        g.flushing = false;
        for n in g.nodes.iter_mut() {
            n.runs = 0;
        }
        if r.is_err() {
            g.queue.clear();
        }
        r
    }

    fn flush_queue<E>(&self, exec: &mut dyn FnMut(NodeId, u64) -> Result<(), E>) -> Result<(), FlushError<E>> {
        loop {
            let next = self.g().queue.pop_front();
            let Some(c) = next else { return Ok(()) };
            match self.update(c, exec) {
                Ok(()) => {}
                // a Rust-only flush met an external computation: leave it for the embedder
                Err(FlushError::External(e)) => {
                    let mut g = self.g();
                    if g.get(c).is_some_and(|n| n.state != State::Clean) && !g.queue.contains(&c) {
                        g.queue.push_back(c);
                    }
                    g.external_pending = true;
                    return Err(FlushError::External(e));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Whether Rust code made changes an external computation still has to
    /// see (the embedder should flush).
    pub fn take_external_pending(&self) -> bool {
        std::mem::take(&mut self.g().external_pending)
    }
}
