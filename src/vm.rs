//! The virtual machine: `src/vm.c`, `src/state.c`, and the dispatch parts of
//! `src/class.c` / `src/error.c` in mruby.
//!
//! Differences from mruby worth knowing:
//! * Like mruby's `mrb_vm_exec`, one dispatch loop ([`Vm::run_frames`]) runs
//!   Ruby→Ruby calls on an explicit frame stack (`Vm::frames`, mruby's
//!   callinfo stack) without native recursion. A native method calling back
//!   into Ruby (`Array#each` yielding) starts a nested loop, as mruby's C
//!   functions do; that native recursion is bounded by `Vm::stack_budget`.
//! * As in mruby, the registers of all live frames are windows of one value
//!   stack (`Vm::stack`). A frame whose locals a block captures gets an
//!   `Env` that points into its window and copies it when the frame returns.
//! * mruby unwinds with `setjmp/longjmp`; here unwinding is `Result<_, Unwind>`
//!   propagated with `?`, and each frame consults its irep's catch table.

use crate::gc::{ArrayRef, ClassRef, EnvRef, HashRef, Heap, Marker};
use crate::rhash::stored_hash;
use crate::opcode::{ArgSpec, CallCache, DefinedKind, Irep, Literal, Op, Reg};
use crate::symbol::{Sym, SymMap, SymbolTable};
use crate::value::*;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

/// Non-local control flow.
pub enum Unwind {
    /// A raised exception (an exception object).
    Error(Value),
    /// An exception not yet made into an object: class and message. Errors
    /// raised from Rust are created this way so raising doesn't allocate
    /// ([`Vm::err`] only needs `&self`); [`Vm::materialize`] makes the object
    /// when Ruby code gets to see it.
    Raise(ClassRef, String),
    /// `break` out of a block; caught by the call site that passed the block (`tag`).
    Break { tag: u64, value: Value },
    /// `return` from inside a block; caught by the method frame `home`.
    Return { home: usize, value: Value },
}

pub type VmResult = Result<Value, Unwind>;

/// Arguments a native call can take without a heap-allocated argument list.
const MAX_INLINE_ARGS: usize = 4;

/// Deepest Ruby call nesting (frames on the VM stack) before SystemStackError.
const MAX_FRAMES: usize = 10_000;

/// Default for [`Vm::stack_budget`]. Ruby→Ruby calls don't use the native
/// stack, but natives calling back into Ruby do; wasm's default stack is 1 MiB.
pub const DEFAULT_STACK_BUDGET: usize = if cfg!(target_family = "wasm") {
    512 << 10
} else {
    1 << 20
};

pub struct Classes {
    pub basic_object: ClassRef,
    pub object: ClassRef,
    pub kernel: ClassRef,
    pub module: ClassRef,
    pub class: ClassRef,
    pub comparable: ClassRef,
    pub enumerable: ClassRef,
    pub numeric: ClassRef,
    pub integer: ClassRef,
    pub float: ClassRef,
    pub string: ClassRef,
    pub symbol: ClassRef,
    pub array: ClassRef,
    pub hash: ClassRef,
    pub range: ClassRef,
    pub proc_: ClassRef,
    pub nil: ClassRef,
    pub true_: ClassRef,
    pub false_: ClassRef,
    pub exception: ClassRef,
    pub script_error: ClassRef,
    pub syntax_error: ClassRef,
    pub not_implemented_error: ClassRef,
    pub standard_error: ClassRef,
    pub runtime_error: ClassRef,
    pub frozen_error: ClassRef,
    pub argument_error: ClassRef,
    pub type_error: ClassRef,
    pub name_error: ClassRef,
    pub no_method_error: ClassRef,
    pub zero_division_error: ClassRef,
    pub local_jump_error: ClassRef,
    pub index_error: ClassRef,
    pub key_error: ClassRef,
    pub stop_iteration: ClassRef,
    pub range_error: ClassRef,
    pub float_domain_error: ClassRef,
    pub no_matching_pattern_error: ClassRef,
    pub no_matching_pattern_key_error: ClassRef,
    pub stack_error: ClassRef,
}

impl Classes {
    /// Every builtin class and module (registered as constants of Object,
    /// and GC roots). Destructuring makes a new field a compile error here.
    pub fn all(&self) -> Vec<ClassRef> {
        let Classes {
            basic_object,
            object,
            kernel,
            module,
            class,
            comparable,
            enumerable,
            numeric,
            integer,
            float,
            string,
            symbol,
            array,
            hash,
            range,
            proc_,
            nil,
            true_,
            false_,
            exception,
            script_error,
            syntax_error,
            not_implemented_error,
            standard_error,
            runtime_error,
            frozen_error,
            argument_error,
            type_error,
            name_error,
            no_method_error,
            zero_division_error,
            local_jump_error,
            index_error,
            key_error,
            stop_iteration,
            range_error,
            float_domain_error,
            no_matching_pattern_error,
            no_matching_pattern_key_error,
            stack_error,
        } = self;
        vec![
            *basic_object, *object, *kernel, *module, *class, *comparable, *enumerable,
            *numeric, *integer, *float, *string, *symbol, *array, *hash, *range, *proc_,
            *nil, *true_, *false_, *exception, *script_error, *syntax_error,
            *not_implemented_error, *standard_error, *runtime_error, *frozen_error,
            *argument_error, *type_error, *name_error, *no_method_error,
            *zero_division_error, *local_jump_error, *index_error, *key_error,
            *stop_iteration, *range_error, *float_domain_error, *no_matching_pattern_error,
            *no_matching_pattern_key_error, *stack_error,
        ]
    }
}

/// Pre-interned symbols (mruby: "presym").
pub struct CommonSyms {
    pub initialize: Sym,
    pub message: Sym,
    pub call: Sym,
    pub to_s: Sym,
    pub inspect: Sym,
    pub each: Sym,
    pub eq: Sym,
    pub cmp: Sym,
    pub add: Sym,
    pub sub: Sym,
    pub mul: Sym,
    pub div: Sym,
    pub lt: Sym,
    pub le: Sym,
    pub gt: Sym,
    pub ge: Sym,
    pub method_missing: Sym,
    pub to_a: Sym,
    pub to_ary: Sym,
    pub to_proc: Sym,
    pub hash: Sym,
    pub eql: Sym,
    pub initialize_copy: Sym,
    pub respond_to_missing: Sym,
}

pub struct Vm {
    pub heap: Heap,
    /// Registers of all live frames, each frame a window at its `base`
    /// (mruby: the VM stack).
    stack: Vec<Value>,
    pub syms: SymbolTable,
    pub c: Classes,
    pub s: CommonSyms,
    pub globals: SymMap<Value>,
    pub main_obj: Value,
    /// When set, output of `puts`/`print`/`p` goes here instead of stdout.
    pub capture: Option<String>,
    /// Blocks registered with `at_exit` / `END { }`.
    pub at_exit: Vec<Value>,
    next_id: usize,
    /// Suspended frames: the callers of the running Ruby frame (mruby: the
    /// callinfo stack). Nested `run_frames` loops share it.
    frames: Vec<Frame>,
    /// Nesting of `run_frames`, i.e. natives that called back into Ruby.
    reentry: usize,
    /// Native stack address at the outermost `run_frames`.
    stack_base: usize,
    /// The `reentry` level of the loop where collection may run (the
    /// outermost loop of `run_main`); 0 when none is running.
    gc_level: usize,
    /// Native stack (bytes) the VM may use for natives calling back into Ruby
    /// before raising SystemStackError. Raise it when running on a big stack.
    pub stack_budget: usize,
    /// Arrays/hashes being inspected right now, to print recursion as `[...]`.
    inspecting: Vec<u64>,
    /// Blocks whose receiving call is still running: `break` targets.
    live_breaks: Vec<u64>,
    /// Method (and lambda) frames still running: `return` targets of their blocks.
    live_homes: Vec<usize>,
}

struct Frame {
    irep: Rc<Irep>,
    /// Stack index of the frame's register 0 (`self`).
    base: usize,
    /// The env of this frame's own locals, made when a block captures them.
    env: Option<EnvRef>,
    /// For a block: the env of the scope it was created in (upvars).
    upper: Option<EnvRef>,
    self_: Value,
    block: Option<Value>,
    /// Where `def` puts methods and where constants are looked up first.
    target: ClassRef,
    /// (owner, name) of the running method, for `super` and `__method__`.
    method: Option<(ClassRef, Sym)>,
    /// How many optional parameters received an argument.
    opt_filled: usize,
    /// Which keyword parameters received an argument.
    kw_mask: u64,
    home: usize,
    /// Identity of the running block's proc (0 for methods), the tag for `break`.
    proc_tag: u64,
    exc: Option<Value>,
    /// A `break`/`return` paused while an `ensure` clause runs; `Raise` of nil resumes it.
    pending: Option<Unwind>,
    /// Next instruction.
    pc: usize,
    /// The caller's register that receives this frame's value.
    ret: Reg,
    /// Tag of the block passed at the call site: a `break` from it ends this call.
    break_tag: u64,
    /// Catches `return` from its blocks: methods, lambdas and the main script.
    is_home: bool,
    /// A lambda: `break` in its own body returns from it.
    lambda: bool,
    /// Visibility `def` gives methods (`private` with no arguments).
    vis: Vis,
}

impl Frame {
    fn new(irep: Rc<Irep>, base: usize, self_: Value, target: ClassRef) -> Frame {
        Frame {
            irep,
            base,
            env: None,
            upper: None,
            self_,
            block: None,
            target,
            method: None,
            opt_filled: 0,
            kw_mask: 0,
            home: 0,
            proc_tag: 0,
            exc: None,
            pending: None,
            pc: 0,
            ret: 0,
            break_tag: 0,
            is_home: false,
            lambda: false,
            vis: Vis::Public,
        }
    }
}

/// A resolved call: Ruby code to run as a new frame, or the value of a call
/// that already ran (natives, attribute accessors).
#[allow(clippy::large_enum_variant)] // moved, never stored: cheaper than a Box per call
enum Call {
    Frame(Frame),
    Done(Value),
}

/// What the dispatch loop does after an instruction.
#[allow(clippy::large_enum_variant)]
enum Flow {
    Next,
    Return(Value),
    /// Run this frame; its value goes to its `ret` register of the current one.
    Call(Frame),
}

/// Arguments of a call: an owned list, or `n` registers of the caller on
/// the stack starting at `at` (Ruby→Ruby calls bind them without a copy
/// into a list first).
enum Args {
    Owned(Vec<Value>),
    Stack { at: usize, n: usize },
}

impl Args {
    fn len(&self) -> usize {
        match self {
            Args::Owned(v) => v.len(),
            Args::Stack { n, .. } => *n,
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn into_vec(self, vm: &Vm) -> Vec<Value> {
        match self {
            Args::Owned(v) => v,
            Args::Stack { at, n } => vm.stack[at..at + n].to_vec(),
        }
    }
}

/// Bound arguments: the frame's registers plus what the prologue needs.
struct Bound {
    base: usize,
    opt_filled: usize,
    kw_mask: u64,
}

/// Overrides for running a proc as something else (`instance_eval`,
/// `class_eval`, `define_method`).
#[derive(Default)]
pub struct ProcCall {
    pub self_: Option<Value>,
    pub target: Option<ClassRef>,
    pub method: Option<(ClassRef, Sym)>,
    /// Treat as a lambda (strict arity, `return` returns from the proc).
    pub lambda: bool,
}

#[derive(Clone, Copy)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

fn float_arith(op: Arith, a: f64, b: f64) -> f64 {
    match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
        Arith::Div => a / b,
        Arith::Mod => {
            let r = a % b;
            if r != 0.0 && ((r < 0.0) != (b < 0.0)) {
                r + b
            } else {
                r
            }
        }
        Arith::Pow => a.powf(b),
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Vm {
        let mut syms = SymbolTable::new();
        let mut heap = Heap::default();
        let mut mk = |name: &str, sup: Option<ClassRef>, is_module: bool| {
            heap.alloc(RClass::new(name, sup, is_module))
        };
        let basic_object = mk("BasicObject", None, false);
        let object = mk("Object", Some(basic_object), false);
        let kernel = mk("Kernel", None, true);
        let mut sub = |name: &str, sup: ClassRef| mk(name, Some(sup), false);
        let module = sub("Module", object);
        let class = sub("Class", module);
        let numeric = sub("Numeric", object);
        let exception = sub("Exception", object);
        let script_error = sub("ScriptError", exception);
        let standard_error = sub("StandardError", exception);
        let runtime_error = sub("RuntimeError", standard_error);
        let name_error = sub("NameError", standard_error);
        let index_error = sub("IndexError", standard_error);
        let range_error = sub("RangeError", standard_error);
        let no_matching_pattern_error = sub("NoMatchingPatternError", standard_error);
        let c = Classes {
            integer: sub("Integer", numeric),
            float: sub("Float", numeric),
            string: sub("String", object),
            symbol: sub("Symbol", object),
            array: sub("Array", object),
            hash: sub("Hash", object),
            range: sub("Range", object),
            proc_: sub("Proc", object),
            nil: sub("NilClass", object),
            true_: sub("TrueClass", object),
            false_: sub("FalseClass", object),
            syntax_error: sub("SyntaxError", script_error),
            not_implemented_error: sub("NotImplementedError", script_error),
            frozen_error: sub("FrozenError", runtime_error),
            argument_error: sub("ArgumentError", standard_error),
            type_error: sub("TypeError", standard_error),
            no_method_error: sub("NoMethodError", name_error),
            zero_division_error: sub("ZeroDivisionError", standard_error),
            local_jump_error: sub("LocalJumpError", standard_error),
            key_error: sub("KeyError", index_error),
            stop_iteration: sub("StopIteration", index_error),
            float_domain_error: sub("FloatDomainError", range_error),
            no_matching_pattern_key_error: sub("NoMatchingPatternKeyError", no_matching_pattern_error),
            stack_error: sub("SystemStackError", exception),
            comparable: mk("Comparable", None, true),
            enumerable: mk("Enumerable", None, true),
            basic_object,
            object,
            kernel,
            module,
            class,
            numeric,
            exception,
            script_error,
            standard_error,
            runtime_error,
            name_error,
            index_error,
            range_error,
            no_matching_pattern_error,
        };
        heap[c.object].includes.push(c.kernel);
        for cls in c.all() {
            attach_meta(&mut heap, cls, c.class, c.module);
        }
        for cls in c.all() {
            let s = syms.intern(&heap[cls].name());
            heap[c.object].consts.borrow_mut().insert(s, Value::from(cls));
        }
        let s = CommonSyms {
            initialize: syms.intern("initialize"),
            message: syms.intern("@message"),
            call: syms.intern("call"),
            to_s: syms.intern("to_s"),
            inspect: syms.intern("inspect"),
            each: syms.intern("each"),
            eq: syms.intern("=="),
            cmp: syms.intern("<=>"),
            add: syms.intern("+"),
            sub: syms.intern("-"),
            mul: syms.intern("*"),
            div: syms.intern("/"),
            lt: syms.intern("<"),
            le: syms.intern("<="),
            gt: syms.intern(">"),
            ge: syms.intern(">="),
            method_missing: syms.intern("method_missing"),
            to_a: syms.intern("to_a"),
            to_ary: syms.intern("to_ary"),
            hash: syms.intern("hash"),
            eql: syms.intern("eql?"),
            initialize_copy: syms.intern("initialize_copy"),
            respond_to_missing: syms.intern("respond_to_missing?"),
            to_proc: syms.intern("to_proc"),
        };
        let main_obj = Value::from(heap.alloc(RObject::new(c.object)));
        let mut vm = Vm {
            heap,
            stack: Vec::new(),
            syms,
            c,
            s,
            globals: SymMap::default(),
            main_obj,
            capture: None,
            at_exit: Vec::new(),
            next_id: 1,
            frames: Vec::new(),
            reentry: 0,
            stack_base: 0,
            gc_level: 0,
            stack_budget: DEFAULT_STACK_BUDGET,
            inspecting: Vec::new(),
            live_breaks: Vec::new(),
            live_homes: Vec::new(),
        };
        crate::core::init(&mut vm);
        crate::core::load_prelude(&mut vm);
        vm
    }

    // ---- heap values ---------------------------------------------------

    /// The view of `v` (see [`Value`]).
    #[inline]
    pub fn kind(&self, v: Value) -> V {
        self.heap.kind(v)
    }

    /// An Integer value (may allocate a box for a large one).
    pub fn int(&mut self, n: i64) -> Value {
        self.heap.int(n)
    }

    pub fn str(&mut self, s: impl Into<String>) -> Value {
        Value::from(self.heap.alloc(RefCell::new(s.into())))
    }

    pub fn array(&mut self, v: Vec<Value>) -> Value {
        Value::from(self.heap.alloc(RefCell::new(v)))
    }

    pub fn hash(&mut self, h: RHash) -> Value {
        Value::from(self.heap.alloc(RefCell::new(h)))
    }

    pub fn range(&mut self, lo: Value, hi: Value, excl: bool) -> Value {
        Value::from(self.heap.alloc((lo, hi, excl)))
    }

    pub fn object(&mut self, o: RObject) -> Value {
        Value::from(self.heap.alloc(o))
    }

    /// `h[k] = v`. A new String key is copied, as Ruby dups (and freezes) them.
    pub fn hash_set(&mut self, h: HashRef, k: Value, v: Value) -> Result<(), Unwind> {
        if self.custom_key(&k) {
            return self.hash_set_custom(h, k, v);
        }
        let k = match self.kind(k) {
            V::Str(s) if !self.heap[h].borrow().contains(&self.heap, &k) => {
                let copy = self.heap[s].borrow().clone();
                self.str(copy)
            }
            _ => k,
        };
        self.heap[h].borrow_mut().set(&self.heap, k, v);
        Ok(())
    }

    #[inline(never)]
    fn hash_set_custom(&mut self, h: HashRef, k: Value, v: Value) -> Result<(), Unwind> {
        match self.hash_find_custom(h, &k)? {
            (Some(p), _) => self.heap[h].borrow_mut().set_value_at(p, v),
            (None, code) => self.heap[h].borrow_mut().push(&self.heap, k, v, code),
        }
        Ok(())
    }

    /// `h[k]` without the default.
    #[inline]
    pub fn hash_get(&mut self, h: HashRef, k: &Value) -> Result<Option<Value>, Unwind> {
        if !self.custom_key(k) {
            return Ok(self.heap[h].borrow().get(&self.heap, k));
        }
        Ok(self.hash_find_custom(h, k)?.0.map(|p| self.heap[h].borrow().value_at(p)))
    }

    pub fn hash_delete(&mut self, h: HashRef, k: &Value) -> Result<Option<Value>, Unwind> {
        Ok(self.hash_find(h, k)?.map(|p| self.heap[h].borrow_mut().remove_at(&self.heap, p)))
    }

    /// Position of `k` in `h`, through the key's own `hash` and `eql?`
    /// when its class defines them.
    #[inline]
    pub fn hash_find(&mut self, h: HashRef, k: &Value) -> Result<Option<usize>, Unwind> {
        if self.custom_key(k) {
            Ok(self.hash_find_custom(h, k)?.0)
        } else {
            Ok(self.heap[h].borrow().find(&self.heap, k))
        }
    }

    /// Ruby's `Hash#rehash`: hashes every key again (after keys mutated).
    pub fn hash_rehash(&mut self, h: HashRef) -> Result<(), Unwind> {
        let pairs = self.heap[h].borrow().to_vec();
        self.heap[h].borrow_mut().clear();
        for (k, v) in pairs {
            self.hash_set(h, k, v)?;
        }
        Ok(())
    }

    /// An object whose class defines its own `hash` or `eql?`: as a Hash
    /// key it is hashed and compared by calling them.
    #[inline]
    fn custom_key(&self, k: &Value) -> bool {
        k.is_object() && self.custom_object_key(k)
    }

    #[inline(never)]
    fn custom_object_key(&self, k: &Value) -> bool {
        let cls = self.dispatch_class(k);
        [self.s.hash, self.s.eql].into_iter().any(|m| match self.find_method(&cls, m) {
            Some((Method::Native(_), owner)) => owner != self.c.kernel,
            Some(_) => true,
            None => false,
        })
    }

    /// Looks up a custom key: its position, and the stored hash it has
    /// (to add it with). Ruby code runs between the steps, so the hash may
    /// change under the lookup; that raises rather than reading entries
    /// that moved.
    fn hash_find_custom(&mut self, h: HashRef, k: &Value) -> Result<(Option<usize>, u64), Unwind> {
        let hv = self.call_method(k, self.s.hash, vec![], None)?;
        if !matches!(self.kind(hv), V::Int(_)) {
            let c = self.class_name(&hv);
            return Err(self.type_err(format!("hash must return an Integer, not {c}")));
        }
        let code = stored_hash(hash_key(&self.heap, &hv));
        let (candidates, version) = {
            let r = self.heap[h].borrow();
            (r.candidates(code), r.version())
        };
        for p in candidates {
            let Some(stored) = self.heap[h].borrow().key_at(p) else {
                continue;
            };
            if self.heap.identical(k, &stored) {
                return Ok((Some(p), code));
            }
            let eq = self.call_method(k, self.s.eql, vec![stored], None)?.truthy();
            if self.heap[h].borrow().version() != version {
                return Err(self.err(&self.c.runtime_error, "hash modified during lookup"));
            }
            if eq {
                return Ok((Some(p), code));
            }
        }
        Ok((None, code))
    }

    /// Contents of a String value.
    pub fn string(&self, v: &Value) -> Option<String> {
        match self.kind(*v) {
            V::Str(s) => Some(self.heap[s].borrow().clone()),
            _ => None,
        }
    }

    /// Elements of an Array value.
    pub fn elements(&self, v: &Value) -> Option<Vec<Value>> {
        match self.kind(*v) {
            V::Array(a) => Some(self.heap[a].borrow().clone()),
            _ => None,
        }
    }

    /// The env `depth` levels out from a block's own (its outer scopes).
    fn upvar_env(&self, f: &Frame, depth: usize) -> EnvRef {
        let mut env = f.upper.expect("upvar outside of block");
        for _ in 0..depth {
            env = self.heap[env].upper.expect("upvar depth");
        }
        env
    }

    fn env_get(&self, e: EnvRef, i: usize) -> Value {
        match &*self.heap[e].regs.borrow() {
            EnvRegs::Stack { base, .. } => self.stack[base + i],
            EnvRegs::Heap(vals) => vals[i],
        }
    }

    fn env_set(&mut self, e: EnvRef, i: usize, v: Value) {
        let on_stack = match &*self.heap[e].regs.borrow() {
            EnvRegs::Stack { base, .. } => Some(base + i),
            EnvRegs::Heap(_) => None,
        };
        match on_stack {
            Some(at) => self.stack[at] = v,
            None => {
                if let EnvRegs::Heap(vals) = &mut *self.heap[e].regs.borrow_mut() {
                    vals[i] = v;
                }
            }
        }
    }

    /// The env of `f`'s locals, made on first use (a block captures them).
    fn frame_env(&mut self, f: &mut Frame) -> EnvRef {
        if let Some(e) = f.env {
            return e;
        }
        let len = f.irep.nregs.max(1 + f.irep.spec.nparams());
        let e = self.heap.alloc(Env {
            regs: RefCell::new(EnvRegs::Stack { base: f.base, len }),
            upper: f.upper,
        });
        f.env = Some(e);
        e
    }

    /// A finished frame gives back its stack window; an env its blocks
    /// captured keeps a copy of the registers.
    fn release_frame(&mut self, f: &Frame) {
        if let Some(e) = f.env {
            let mut regs = self.heap[e].regs.borrow_mut();
            if let EnvRegs::Stack { base, len } = *regs {
                *regs = EnvRegs::Heap(self.stack[base..base + len].to_vec());
            }
        }
        self.stack.truncate(f.base);
    }

    #[inline]
    fn reg(&self, f: &Frame, i: Reg) -> Value {
        self.stack[f.base + i]
    }

    #[inline]
    fn set_reg(&mut self, f: &Frame, i: Reg, v: Value) {
        self.stack[f.base + i] = v;
    }

    // ---- small helpers -------------------------------------------------

    pub fn intern(&mut self, s: &str) -> Sym {
        self.syms.intern(s)
    }

    pub fn sym_name(&self, s: Sym) -> &str {
        self.syms.name(s)
    }

    /// Resets per-run state after a Rust panic unwound through the VM.
    pub fn recover(&mut self) {
        self.frames.clear();
        self.stack.clear();
        self.reentry = 0;
        self.gc_level = 0;
        self.inspecting.clear();
        self.live_breaks.clear();
        self.live_homes.clear();
    }

    fn new_id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    pub fn def(&mut self, cls: &ClassRef, name: &str, f: NativeFn) {
        let s = self.intern(name);
        self.heap[cls].methods.insert(s, Method::Native(f));
    }

    pub fn def_singleton(&mut self, cls: &ClassRef, name: &str, f: NativeFn) {
        let s = self.intern(name);
        let meta = self.class_singleton(cls);
        self.heap[meta].methods.insert(s, Method::Native(f));
    }

    /// A new class or module together with its singleton class, whose
    /// superclass is the superclass's singleton class (so class methods are
    /// inherited; mruby: `prepare_singleton_class`). Making it eagerly keeps
    /// method lookup free of allocation.
    pub fn new_class(&mut self, name: &str, superclass: Option<ClassRef>, is_module: bool) -> ClassRef {
        let c = self.heap.alloc(RClass::new(name, superclass, is_module));
        attach_meta(&mut self.heap, c, self.c.class, self.c.module);
        c
    }

    pub fn set_const(&mut self, name: &str, v: Value) {
        let s = self.intern(name);
        self.heap[self.c.object].consts.borrow_mut().insert(s, v);
    }

    pub fn write(&mut self, s: &str) {
        match &mut self.capture {
            Some(buf) => buf.push_str(s),
            None => {
                use std::io::Write;
                let _ = std::io::stdout().lock().write_all(s.as_bytes());
            }
        }
    }

    // ---- errors (src/error.c) ------------------------------------------

    pub fn make_exception(&mut self, cls: &ClassRef, msg: impl Into<String>) -> Value {
        let msg = self.str(msg);
        let obj = RObject::new(*cls);
        obj.ivars.borrow_mut().set(self.s.message, msg);
        self.object(obj)
    }

    pub fn err(&self, cls: &ClassRef, msg: impl Into<String>) -> Unwind {
        Unwind::Raise(*cls, msg.into())
    }

    /// Turns a pending [`Unwind::Raise`] into an exception object.
    pub fn materialize(&mut self, e: Unwind) -> Unwind {
        match e {
            Unwind::Raise(cls, msg) => Unwind::Error(self.make_exception(&cls, msg)),
            e => e,
        }
    }

    pub fn arg_err(&self, msg: impl Into<String>) -> Unwind {
        self.err(&self.c.argument_error, msg)
    }

    pub fn type_err(&self, msg: impl Into<String>) -> Unwind {
        self.err(&self.c.type_error, msg)
    }

    pub fn name_err(&self, msg: impl Into<String>) -> Unwind {
        self.err(&self.c.name_error, msg)
    }

    pub fn exc_message(&self, e: &Value) -> String {
        if let V::Object(o) = self.kind(*e)
            && let Some(V::Str(s)) = self.heap[o].ivars.borrow().get(self.s.message).map(|v| self.kind(v))
        {
            return self.heap[s].borrow().clone();
        }
        self.heap[self.class_of(e)].name()
    }

    // ---- classes & method lookup (src/class.c) -------------------------

    /// The class `obj.class` reports (never a singleton class).
    pub fn class_of(&self, v: &Value) -> ClassRef {
        let c = &self.c;
        match self.kind(*v) {
            V::Nil => c.nil,
            V::Bool(true) => c.true_,
            V::Bool(false) => c.false_,
            V::Int(_) => c.integer,
            V::Float(_) => c.float,
            V::Sym(_) => c.symbol,
            V::Str(_) => c.string,
            V::Array(_) => c.array,
            V::Hash(_) => c.hash,
            V::Range(_) => c.range,
            V::Proc(_) => c.proc_,
            V::Class(k) => {
                if self.heap[k].is_module {
                    c.module
                } else {
                    c.class
                }
            }
            V::Object(o) => self.heap[o].class,
        }
    }

    pub fn class_name(&self, v: &Value) -> String {
        self.heap[self.class_of(v)].name()
    }

    /// The singleton class of a class or module (made with the class, see
    /// [`Vm::new_class`]). A singleton class has none until one is asked
    /// for with [`Vm::singleton_class`]; lookup then starts at `Class`.
    pub fn class_singleton(&self, c: &ClassRef) -> ClassRef {
        self.heap[c].meta.get().unwrap_or(self.c.class)
    }

    /// `obj.singleton_class`, created on demand.
    pub fn singleton_class(&mut self, v: &Value) -> Result<ClassRef, Unwind> {
        match self.kind(*v) {
            V::Class(c) => {
                if self.heap[c].meta.get().is_none() {
                    // a singleton class of a singleton class
                    attach_meta(&mut self.heap, c, self.c.class, self.c.module);
                }
                Ok(self.class_singleton(&c))
            }
            V::Object(o) => {
                if let Some(s) = self.heap[o].singleton.get() {
                    return Ok(s);
                }
                let class = self.heap[o].class;
                let name = format!("#<Class:#<{}>>", self.heap[class].name());
                let s = self.heap.alloc(RClass::new_singleton(name, class));
                self.heap[o].singleton.set(Some(s));
                Ok(s)
            }
            V::Nil => Ok(self.c.nil),
            V::Bool(true) => Ok(self.c.true_),
            V::Bool(false) => Ok(self.c.false_),
            _ => Err(self.type_err(format!("can't define singleton for {}", self.class_name(v)))),
        }
    }

    /// The class where method lookup for `v` starts.
    pub fn dispatch_class(&self, v: &Value) -> ClassRef {
        match self.kind(*v) {
            V::Class(c) => self.class_singleton(&c),
            V::Object(o) => {
                let o = &self.heap[o];
                o.singleton.get().unwrap_or(o.class)
            }
            _ => self.class_of(v),
        }
    }

    /// Method resolution order: prepended modules, the class, its included
    /// modules (latest first), then the superclass.
    pub fn ancestors(&self, cls: &ClassRef) -> Vec<ClassRef> {
        let mut out: Vec<ClassRef> = Vec::new();
        let add = |out: &mut Vec<ClassRef>, k: ClassRef| {
            if !out.contains(&k) {
                out.push(k);
            }
        };
        let mut c = Some(*cls);
        while let Some(k) = c {
            let kc = &self.heap[k];
            for p in kc.prepends.borrow().iter().rev() {
                for a in self.ancestors(p) {
                    add(&mut out, a);
                }
            }
            add(&mut out, k);
            for m in kc.includes.borrow().iter().rev() {
                for a in self.ancestors(m) {
                    add(&mut out, a);
                }
            }
            c = kc.superclass;
        }
        out
    }

    fn find_in_module(&self, k: ClassRef, mid: Sym) -> Option<Option<(Method, ClassRef, Vis)>> {
        let kc = &self.heap[k];
        for p in kc.prepends.borrow().iter().rev() {
            if let Some(found) = self.find_in_module(*p, mid) {
                return Some(found);
            }
        }
        if let Some(m) = kc.methods.borrow().get(&mid) {
            return Some(match m {
                Method::Undefined => None,
                m => Some((m.clone(), k, kc.methods.vis(mid))),
            });
        }
        for inc in kc.includes.borrow().iter().rev() {
            if let Some(found) = self.find_in_module(*inc, mid) {
                return Some(found);
            }
        }
        None
    }

    /// Finds `mid` starting at `cls`; returns the method and its owner.
    pub fn find_method(&self, cls: &ClassRef, mid: Sym) -> Option<(Method, ClassRef)> {
        self.find_method_vis(cls, mid).map(|(m, owner, _)| (m, owner))
    }

    /// [`Vm::find_method`] with the method's visibility.
    pub fn find_method_vis(&self, cls: &ClassRef, mid: Sym) -> Option<(Method, ClassRef, Vis)> {
        let mut c = Some(*cls);
        while let Some(k) = c {
            if let Some(found) = self.find_in_module(k, mid) {
                return found;
            }
            c = self.heap[k].superclass;
        }
        None
    }

    /// Identity of the class lookup for `v` starts at; 0 for classes, whose
    /// lookup isn't cached.
    fn dispatch_key(&self, v: &Value) -> u64 {
        match self.kind(*v) {
            V::Class(_) => 0,
            _ => self.dispatch_class(v).id(),
        }
    }

    /// `lookup` through the inline cache of the `Send` that just ran
    /// (`f.pc - 1`).
    fn cached_lookup(&self, f: &Frame, recv: &Value, mid: Sym) -> Option<(Method, ClassRef, Vis)> {
        let key = self.dispatch_key(recv);
        if key == 0 {
            return self.lookup_vis(recv, mid);
        }
        let epoch = method_epoch();
        let cache = &f.irep.caches[f.pc - 1];
        {
            let c = cache.borrow();
            if c.class == key && c.epoch == epoch {
                return c.hit.clone();
            }
        }
        let found = self.lookup_vis(recv, mid);
        *cache.borrow_mut() = CallCache {
            class: key,
            epoch,
            hit: found.clone(),
        };
        found
    }

    fn lookup(&self, recv: &Value, mid: Sym) -> Option<(Method, ClassRef)> {
        self.find_method(&self.dispatch_class(recv), mid)
    }

    fn lookup_vis(&self, recv: &Value, mid: Sym) -> Option<(Method, ClassRef, Vis)> {
        self.find_method_vis(&self.dispatch_class(recv), mid)
    }

    /// Whether code running as `caller` (its `self`) may call a method of
    /// `owner` with visibility `vis` on `recv`. A private method needs the
    /// receiver to be the caller's self (`foo`, `self.foo`); a protected
    /// one a caller that is an instance of the method's owner.
    #[inline]
    fn visible(&self, vis: Vis, owner: &ClassRef, recv: &Value, caller: &Value) -> bool {
        match vis {
            Vis::Public => true,
            Vis::Private | Vis::ModuleFunction => self.heap.identical(recv, caller),
            Vis::Protected => self.heap.identical(recv, caller) || self.is_a(caller, owner),
        }
    }

    #[cold]
    fn not_visible(&self, vis: Vis, recv: &Value, mid: Sym) -> Unwind {
        let kind = if vis == Vis::Protected { "protected" } else { "private" };
        let msg = format!(
            "{kind} method '{}' called for {}",
            self.sym_name(mid),
            self.recv_desc(recv)
        );
        self.err(&self.c.no_method_error, msg)
    }

    pub fn respond_to(&self, v: &Value, mid: Sym) -> bool {
        self.lookup(v, mid).is_some()
    }

    pub fn is_a(&self, v: &Value, cls: &ClassRef) -> bool {
        self.ancestors(&self.dispatch_class(v))
            .iter()
            .any(|c| c == cls)
    }

    fn find_super(
        &self,
        recv: &Value,
        owner: &ClassRef,
        mid: Sym,
    ) -> Option<(Method, ClassRef)> {
        let anc = self.ancestors(&self.dispatch_class(recv));
        let pos = anc.iter().position(|c| c == owner)?;
        for k in &anc[pos + 1..] {
            match self.heap[k].methods.borrow().get(&mid) {
                Some(Method::Undefined) => return None,
                Some(m) => return Some((m.clone(), *k)),
                None => {}
            }
        }
        None
    }

    /// The class used for constants and class variables: singleton classes
    /// of classes defer to the class itself.
    fn lexical_class(&self, target: &ClassRef) -> ClassRef {
        if self.heap[target].is_singleton
            && let Some(c) = self.heap[target].attached_class()
        {
            return c;
        }
        *target
    }

    pub fn get_const(&self, target: &ClassRef, name: Sym) -> Option<Value> {
        let start = self.lexical_class(target);
        // lexically enclosing classes first, then the ancestors
        let mut lex = self.heap[start].outer_class();
        if let Some(v) = self.heap[start].consts.borrow().get(&name) {
            return Some(*v);
        }
        while let Some(o) = lex {
            if let Some(v) = self.heap[o].consts.borrow().get(&name) {
                return Some(*v);
            }
            lex = self.heap[o].outer_class();
        }
        self.get_const_in(&start, name)
            .or_else(|| self.heap[self.c.object].consts.borrow().get(&name).cloned())
    }

    /// `Mod::Name`: the module and its ancestors (Object only if it's one of them).
    pub fn get_const_in(&self, cls: &ClassRef, name: Sym) -> Option<Value> {
        self.ancestors(cls)
            .into_iter()
            .find_map(|k| self.heap[k].consts.borrow().get(&name).cloned())
    }

    /// Stores a constant, naming an anonymous class after it.
    pub fn store_const(&self, owner: &ClassRef, name: Sym, v: Value) {
        if let V::Class(c) = self.kind(v)
            && self.heap[c].is_anonymous()
        {
            let n = self.sym_name(name);
            let full = if *owner == self.c.object {
                n.to_string()
            } else {
                format!("{}::{}", self.heap[owner].name(), n)
            };
            self.heap[c].set_name(full);
            if owner != &self.c.object {
                self.heap[c].outer.set(Some(*owner));
            }
        }
        self.heap[owner].consts.borrow_mut().insert(name, v);
    }

    fn cvar_holder(&self, cls: &ClassRef, name: Sym) -> Option<ClassRef> {
        self.ancestors(cls)
            .into_iter()
            .find(|k| self.heap[k].cvars.borrow().get(name).is_some())
    }

    fn cvar_get(&self, target: &ClassRef, name: Sym) -> VmResult {
        let base = self.lexical_class(target);
        match self.cvar_holder(&base, name) {
            Some(k) => Ok(self.heap[k].cvars.borrow().get(name).unwrap()),
            None => Err(self.name_err(format!(
                "uninitialized class variable {} in {}",
                self.sym_name(name),
                self.heap[base].name()
            ))),
        }
    }

    fn cvar_set(&self, target: &ClassRef, name: Sym, v: Value) {
        let base = self.lexical_class(target);
        let holder = self.cvar_holder(&base, name).unwrap_or(base);
        self.heap[holder].cvars.borrow_mut().set(name, v);
    }

    // ---- calling -------------------------------------------------------

    pub fn call_method(
        &mut self,
        recv: &Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> VmResult {
        let call = self.start_call(*recv, mid, Args::Owned(args), block)?;
        self.finish(call)
    }

    /// Resolves `recv.mid(*args)` (with `method_missing` as fallback).
    fn start_call(
        &mut self,
        recv: Value,
        mid: Sym,
        args: Args,
        block: Option<Value>,
    ) -> Result<Call, Unwind> {
        match self.lookup(&recv, mid) {
            Some((m, owner)) => self.start_invoke(m, owner, recv, mid, args, block),
            None => {
                if let Some((m, owner)) = self.lookup(&recv, self.s.method_missing) {
                    let mut margs = vec![Value::sym(mid)];
                    margs.extend(args.into_vec(self));
                    let mm = self.s.method_missing;
                    return self.start_invoke(m, owner, recv, mm, Args::Owned(margs), block);
                }
                Err(self.no_method(&recv, mid))
            }
        }
    }

    /// Runs a resolved call to completion.
    fn finish(&mut self, call: Call) -> VmResult {
        match call {
            Call::Done(v) => Ok(v),
            Call::Frame(f) => self.run_frames(f),
        }
    }

    pub fn no_method(&self, recv: &Value, mid: Sym) -> Unwind {
        let msg = format!("undefined method '{}' for {}", self.sym_name(mid), self.recv_desc(recv));
        self.err(&self.c.no_method_error, msg)
    }

    /// How error messages name a receiver: "nil", "class Foo", "an instance of Foo".
    fn recv_desc(&self, recv: &Value) -> String {
        match self.kind(*recv) {
            V::Nil => "nil".to_string(),
            V::Bool(b) => b.to_string(),
            V::Class(c) => format!(
                "{} {}",
                if self.heap[c].is_module { "module" } else { "class" },
                self.heap[c].name()
            ),
            _ if self.heap.identical(recv, &self.main_obj) => "main:Object".to_string(),
            _ => format!("an instance of {}", self.class_name(recv)),
        }
    }

    /// Convenience: `recv.name(*args)`.
    pub fn send(&mut self, recv: &Value, name: &str, args: Vec<Value>) -> VmResult {
        let mid = self.intern(name);
        self.call_method(recv, mid, args, None)
    }

    /// Calls with a block and turns a `break` from that block into the call's value.
    pub fn call_with_block(
        &mut self,
        recv: &Value,
        mid: Sym,
        args: Vec<Value>,
        blk: Option<Value>,
    ) -> VmResult {
        let tag = proc_tag(&blk);
        if tag == 0 {
            return self.call_method(recv, mid, args, blk);
        }
        self.live_breaks.push(tag);
        let r = self.call_method(recv, mid, args, blk);
        self.live_breaks.pop();
        match r {
            Err(Unwind::Break { tag: t, value }) if t == tag => Ok(value),
            r => r,
        }
    }

    pub fn invoke(
        &mut self,
        m: Method,
        owner: ClassRef,
        recv: Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> VmResult {
        let call = self.start_invoke(m, owner, recv, mid, Args::Owned(args), block)?;
        self.finish(call)
    }

    /// Starts calling method `m`: Ruby code becomes a frame for the caller
    /// to run, anything else runs now.
    fn start_invoke(
        &mut self,
        m: Method,
        owner: ClassRef,
        recv: Value,
        mid: Sym,
        args: Args,
        block: Option<Value>,
    ) -> Result<Call, Unwind> {
        let v = match m {
            Method::Native(f) => {
                let args = args.into_vec(self);
                f(self, &recv, &args, block.as_ref())?
            }
            Method::AttrReader(iv) => {
                if !args.is_empty() {
                    return Err(self.arity_err(args.len(), &ArgSpec::default()));
                }
                self.ivar_get(&recv, iv)
            }
            Method::AttrWriter(iv) => {
                if args.len() != 1 {
                    return Err(self.arity_err(
                        args.len(),
                        &ArgSpec {
                            req: 1,
                            ..Default::default()
                        },
                    ));
                }
                let v = args.into_vec(self).pop().unwrap();
                self.ivar_set(&recv, iv, v)?;
                v
            }
            Method::Proc(p) => {
                let call = ProcCall {
                    self_: Some(recv),
                    target: Some(owner),
                    method: Some((owner, mid)),
                    lambda: true,
                };
                return self.start_proc(&p, args, block, call);
            }
            Method::ProcCall => return self.start_proc(&recv, args, block, ProcCall::default()),
            Method::Undefined => return Err(self.no_method(&recv, mid)),
            Method::Ruby(irep) => {
                let bound = self.bind_args(&irep, &recv, args, &block, true)?;
                let mut f = Frame::new(irep, bound.base, recv, owner);
                f.block = block;
                f.method = Some((owner, mid));
                f.opt_filled = bound.opt_filled;
                f.kw_mask = bound.kw_mask;
                f.home = self.new_id();
                f.is_home = true;
                return Ok(Call::Frame(f));
            }
        };
        Ok(Call::Done(v))
    }

    fn arity_err(&self, given: usize, spec: &ArgSpec) -> Unwind {
        let expected = if spec.rest {
            format!("{}+", spec.mandatory())
        } else if spec.opt > 0 {
            format!("{}..{}", spec.mandatory(), spec.mandatory() + spec.opt)
        } else {
            spec.mandatory().to_string()
        };
        self.arg_err(format!(
            "wrong number of arguments (given {given}, expected {expected})"
        ))
    }

    fn sym_list(&self, syms: &[Sym]) -> String {
        syms.iter()
            .map(|s| format!(":{}", self.sym_name(*s)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Binds arguments to parameter registers (mruby: OP_ENTER).
    /// `strict` is method/lambda semantics; procs pad, truncate and auto-splat.
    fn bind_args(
        &mut self,
        irep: &Irep,
        self_: &Value,
        args: Args,
        block: &Option<Value>,
        strict: bool,
    ) -> Result<Bound, Unwind> {
        let spec = &irep.spec;
        // fast path: only required parameters, as many arguments, no keywords
        if let Args::Stack { at, n } = args
            && n == spec.req
            && spec.opt == 0
            && !spec.rest
            && spec.post == 0
            && !spec.takes_kw()
            && !spec.nokw
            && !spec.auto_splat
            && !(n > 0 && matches!(self.kind(self.stack[at + n - 1]), V::Hash(h) if self.heap[h].borrow().kw))
        {
            let base = self.stack.len();
            self.stack.resize(base + irep.nregs.max(1 + spec.nparams()), Value::NIL);
            self.stack[base] = *self_;
            for i in 0..n {
                self.stack[base + 1 + i] = self.stack[at + i];
            }
            if spec.block {
                self.stack[base + 1 + n] = (*block).unwrap_or(Value::NIL);
            }
            return Ok(Bound {
                base,
                opt_filled: 0,
                kw_mask: 0,
            });
        }
        let mut args = args.into_vec(self);
        // keyword arguments arrive as a trailing Hash marked `kw`
        let mut kwargs: Option<RHash> = None;
        if let Some(V::Hash(h)) = args.last().map(|v| self.kind(*v))
            && self.heap[h].borrow().kw
        {
            if spec.takes_kw() || spec.nokw {
                let mut kh = self.heap[h].borrow().clone();
                kh.kw = false;
                kwargs = Some(kh);
                args.pop();
            } else if self.heap[h].borrow().is_empty() {
                args.pop(); // `**{}` passes nothing
            } else {
                self.heap[h].borrow_mut().kw = false; // a plain positional Hash from here on
            }
        }
        // a proc taking several parameters splats a single Array argument
        if !strict && args.len() == 1 {
            let positional = spec.req + spec.opt + spec.post;
            if (positional > 1 || spec.auto_splat || (spec.rest && positional > 0))
                && let V::Array(a) = self.kind(args[0])
            {
                let items = self.heap[a].borrow().clone();
                args = items;
            }
        }
        let mand = spec.mandatory();
        if strict {
            if args.len() < mand || (!spec.rest && args.len() > mand + spec.opt) {
                return Err(self.arity_err(args.len(), spec));
            }
        } else {
            if args.len() < mand {
                args.resize(mand, Value::NIL);
            }
            if !spec.rest && args.len() > mand + spec.opt {
                args.truncate(mand + spec.opt);
            }
        }
        let n = args.len();
        let opt_filled = (n - mand).min(spec.opt);
        let rest_len = n - mand - opt_filled;
        // the frame's registers: a new window on top of the stack
        let base = self.stack.len();
        self.stack.resize(base + irep.nregs.max(1 + spec.nparams()), Value::NIL);
        self.stack[base] = *self_;
        let mut r = 1;
        let mut it = args.into_iter();
        for _ in 0..spec.req {
            self.stack[base + r] = it.next().unwrap();
            r += 1;
        }
        for i in 0..spec.opt {
            if i < opt_filled {
                self.stack[base + r] = it.next().unwrap();
            }
            r += 1;
        }
        if spec.rest {
            let rest = self.array(it.by_ref().take(rest_len).collect());
            self.stack[base + r] = rest;
            r += 1;
        }
        for _ in 0..spec.post {
            self.stack[base + r] = it.next().unwrap();
            r += 1;
        }
        let mut kw_mask = 0u64;
        if spec.nokw && kwargs.as_ref().is_some_and(|h| !h.is_empty()) {
            self.stack.truncate(base);
            return Err(self.arg_err("no keywords accepted"));
        }
        let mut kwh = kwargs.unwrap_or_default();
        let mut missing = Vec::new();
        for (i, (sym, required)) in spec.kw.iter().enumerate() {
            match kwh.remove(&self.heap, &Value::sym(*sym)) {
                Some(v) => {
                    self.stack[base + r] = v;
                    kw_mask |= 1 << i.min(63);
                }
                None if *required => missing.push(*sym),
                None => {}
            }
            r += 1;
        }
        if !missing.is_empty() {
            self.stack.truncate(base);
            let s = if missing.len() > 1 { "s" } else { "" };
            return Err(self.arg_err(format!("missing keyword{s}: {}", self.sym_list(&missing))));
        }
        if spec.kwrest {
            let kwrest = if spec.nokw {
                Value::NIL
            } else {
                self.hash(kwh)
            };
            self.stack[base + r] = kwrest;
            r += 1;
        } else if !kwh.is_empty() {
            let unknown: Vec<Sym> = kwh
                .iter()
                .filter_map(|(k, _)| {
                    if let V::Sym(s) = self.kind(k) {
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect();
            let s = if unknown.len() > 1 { "s" } else { "" };
            self.stack.truncate(base);
            return Err(self.arg_err(format!("unknown keyword{s}: {}", self.sym_list(&unknown))));
        }
        if spec.block {
            self.stack[base + r] = (*block).unwrap_or(Value::NIL);
        }
        Ok(Bound {
            base,
            opt_filled,
            kw_mask,
        })
    }

    /// Yields to a block (mruby: `mrb_yield`).
    pub fn call_block(&mut self, blk: &Value, args: Vec<Value>) -> VmResult {
        self.call_proc(blk, args, None, ProcCall::default())
    }

    /// Calls a proc, optionally as another self/class (`instance_eval`,
    /// `class_eval`, `define_method`). `block` binds a `&blk` parameter.
    pub fn call_proc(
        &mut self,
        blk: &Value,
        args: Vec<Value>,
        block: Option<Value>,
        call: ProcCall,
    ) -> VmResult {
        let call = self.start_proc(blk, Args::Owned(args), block, call)?;
        self.finish(call)
    }

    fn start_proc(
        &mut self,
        blk: &Value,
        args: Args,
        block: Option<Value>,
        call: ProcCall,
    ) -> Result<Call, Unwind> {
        let V::Proc(p) = self.kind(*blk) else {
            return Err(self.type_err("wrong argument type (expected Proc)"));
        };
        match self.heap[p].body.clone() {
            ProcBody::SymCall(sym) => {
                let mut it = args.into_vec(self).into_iter();
                let recv = it.next().ok_or_else(|| self.arg_err("no receiver given"))?;
                Ok(Call::Done(self.call_method(&recv, sym, it.collect(), block)?))
            }
            ProcBody::Collect(buf) => {
                let mut args = args.into_vec(self);
                let v = if args.len() == 1 {
                    args.pop().unwrap()
                } else {
                    self.array(args)
                };
                self.heap[buf].borrow_mut().push(v);
                Ok(Call::Done(Value::NIL))
            }
            ProcBody::Ruby { irep, env } => {
                let rp = &self.heap[p];
                let lambda = rp.lambda || call.lambda;
                let self_ = call.self_.unwrap_or(rp.self_);
                let target = call.target.unwrap_or(rp.target);
                let method = call.method.or(rp.method);
                let (outer_block, home) = (rp.block, rp.home);
                let bound = self.bind_args(&irep, &self_, args, &block, lambda)?;
                let mut f = Frame::new(irep, bound.base, self_, target);
                f.upper = Some(env);
                f.block = outer_block;
                f.method = method;
                f.opt_filled = bound.opt_filled;
                f.kw_mask = bound.kw_mask;
                f.home = if lambda { self.new_id() } else { home };
                f.proc_tag = p.id();
                f.is_home = lambda;
                f.lambda = lambda;
                Ok(Call::Frame(f))
            }
        }
    }

    pub fn proc_value(&mut self, body: ProcBody) -> Value {
        let target = self.c.object;
        Value::from(self.heap.alloc(RProc {
            body,
            self_: Value::NIL,
            block: None,
            home: 0,
            method: None,
            target,
            lambda: false,
        }))
    }

    /// The same proc with lambda semantics (`lambda { }`).
    pub fn to_lambda(&mut self, v: &Value) -> Value {
        match self.kind(*v) {
            V::Proc(p) if !self.heap[p].lambda => {
                let p = &self.heap[p];
                let copy = RProc {
                    body: p.body.clone(),
                    self_: p.self_,
                    block: p.block,
                    home: p.home,
                    method: p.method,
                    target: p.target,
                    lambda: true,
                };
                Value::from(self.heap.alloc(copy))
            }
            _ => *v,
        }
    }

    fn block_from(&mut self, v: Value) -> Result<Option<Value>, Unwind> {
        match self.kind(v) {
            V::Nil => Ok(None),
            V::Proc(_) => Ok(Some(v)),
            V::Sym(s) => Ok(Some(self.proc_value(ProcBody::SymCall(s)))),
            _ if self.respond_to(&v, self.s.to_proc) => {
                let p = self.call_method(&v, self.s.to_proc, vec![], None)?;
                match self.kind(p) {
                    V::Proc(_) => Ok(Some(p)),
                    _ => Err(self.type_err("can't convert to Proc")),
                }
            }
            _ => Err(self.type_err(format!(
                "wrong argument type {} (expected Proc)",
                self.class_name(&v)
            ))),
        }
    }

    /// Elements of `*v` (mruby: `mrb_ary_splat`).
    pub fn splat_to_vec(&mut self, v: &Value) -> Result<Vec<Value>, Unwind> {
        Ok(match self.kind(*v) {
            V::Array(a) => self.heap[a].borrow().clone(),
            V::Nil => vec![],
            V::Hash(h) => {
                let pairs = self.heap[h].borrow().to_vec();
                pairs.into_iter().map(|(k, v)| self.array(vec![k, v])).collect()
            }
            V::Range(_) => crate::core::to_vec(self, v)?,
            _ if self.respond_to(v, self.s.to_a) => {
                match self.call_method(v, self.s.to_a, vec![], None).map(|r| self.kind(r))? {
                    V::Array(a) => self.heap[a].borrow().clone(),
                    V::Nil => vec![*v],
                    _ => return Err(self.type_err("can't convert to Array (to_a gives non-Array)")),
                }
            }
            _ => vec![*v],
        })
    }

    /// Converts the right-hand side of a multiple assignment to an Array.
    fn masgn_array(&mut self, v: &Value) -> Result<ArrayRef, Unwind> {
        let wrap = |vm: &mut Self| vm.heap.alloc(RefCell::new(vec![*v]));
        match self.kind(*v) {
            V::Array(a) => Ok(a),
            _ if !matches!(self.kind(*v), V::Nil) && self.respond_to(v, self.s.to_ary) => {
                match self.call_method(v, self.s.to_ary, vec![], None).map(|r| self.kind(r))? {
                    V::Array(a) => Ok(a),
                    V::Nil => Ok(wrap(self)),
                    _ => Err(self.type_err("can't convert to Array (to_ary gives non-Array)")),
                }
            }
            _ => Ok(wrap(self)),
        }
    }

    pub fn ivar_get(&self, obj: &Value, iv: Sym) -> Value {
        self.ivar_lookup(obj, iv).unwrap_or(Value::NIL)
    }

    pub fn ivar_lookup(&self, obj: &Value, iv: Sym) -> Option<Value> {
        match self.kind(*obj) {
            V::Object(o) => self.heap[o].ivars.borrow().get(iv),
            V::Class(c) => self.heap[c].ivars.borrow().get(iv),
            _ => None,
        }
    }

    pub fn ivar_set(&self, obj: &Value, iv: Sym, v: Value) -> Result<(), Unwind> {
        match self.kind(*obj) {
            V::Object(o) => self.heap[o].ivars.borrow_mut().set(iv, v),
            V::Class(c) => self.heap[c].ivars.borrow_mut().set(iv, v),
            _ => {
                return Err(self.err(
                    &self.c.runtime_error,
                    "can't set instance variable on a builtin value",
                ));
            }
        }
        Ok(())
    }

    // ---- entry points --------------------------------------------------

    /// Runs a top-level irep with `self = main`.
    pub fn run_main(&mut self, irep: Rc<Irep>) -> Result<Value, Value> {
        let main = self.main_obj;
        let mut f = self.body_frame(irep, main, self.c.object);
        f.home = self.new_id();
        f.is_home = true;
        // the script's own loop is a safe point for collection; loops nested
        // in natives or entered from other APIs are not
        let outer_gc_level = self.gc_level;
        if self.reentry == 0 {
            self.gc_level = 1;
        }
        let r = self.run_frames(f);
        self.gc_level = outer_gc_level;
        let r = r.map_err(|e| self.materialize(e));
        match r {
            Ok(v) => Ok(v),
            Err(Unwind::Error(e)) => Err(e),
            Err(Unwind::Raise(..)) => unreachable!("materialized"),
            Err(Unwind::Return { .. }) => {
                let c = self.c.local_jump_error;
                Err(self.make_exception(&c, "unexpected return"))
            }
            Err(Unwind::Break { .. }) => {
                let c = self.c.local_jump_error;
                Err(self.make_exception(&c, "break from proc-closure"))
            }
        }
    }

    /// A frame for the script or a class body (`class X`, `class << obj`).
    fn body_frame(&mut self, irep: Rc<Irep>, self_: Value, target: ClassRef) -> Frame {
        let base = self.stack.len();
        self.stack.resize(base + irep.nregs.max(1), Value::NIL);
        self.stack[base] = self_;
        let mut f = Frame::new(irep, base, self_, target);
        f.home = self.new_id();
        f
    }

    /// Runs `frame` and every Ruby frame it calls in one loop, without
    /// native recursion (mruby: `mrb_vm_exec`). Returns when `frame` returns,
    /// or with the unwind that leaves it.
    fn run_frames(&mut self, frame: Frame) -> VmResult {
        // natives calling back into Ruby nest this loop: bound the native stack
        let marker = 0u8;
        let sp = std::hint::black_box(&marker) as *const u8 as usize;
        if self.reentry == 0 {
            self.stack_base = sp;
        } else if self.stack_base.saturating_sub(sp) > self.stack_budget
            || self.frames.len() >= MAX_FRAMES
        {
            self.release_frame(&frame); // never entered
            return Err(self.err(&self.c.stack_error, "stack level too deep"));
        }
        self.reentry += 1;
        let base = self.frames.len();
        self.enter_frame(&frame);
        let r = self.dispatch(frame, base);
        self.reentry -= 1;
        r
    }

    fn dispatch(&mut self, mut f: Frame, base: usize) -> VmResult {
        loop {
            if self.heap.gc.due() && self.reentry == self.gc_level {
                self.collect_garbage(&f);
            }
            let at = f.pc;
            let op = f.irep.code[at];
            f.pc += 1;
            let mut result = match self.step(&mut f, op) {
                Ok(Flow::Next) => continue,
                Ok(Flow::Call(callee)) if self.frames.len() < MAX_FRAMES => {
                    self.enter_frame(&callee);
                    let caller = std::mem::replace(&mut f, callee);
                    self.frames.push(caller);
                    continue;
                }
                Ok(Flow::Call(callee)) => {
                    self.release_frame(&callee); // never entered
                    Err(self.err(&self.c.stack_error, "stack level too deep"))
                }
                Ok(Flow::Return(v)) => Ok(v),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                // a break/return of this frame's own already ran its ensure
                // clauses (codegen inlines them)
                let own = matches!(op, Op::Break(_) | Op::ReturnBlk(_));
                match self.catch(&mut f, e, at, own) {
                    None => continue,
                    Some(e) => result = Err(e),
                }
            }
            // `f` is done: give its value to the caller, or keep unwinding
            loop {
                result = self.leave_frame(&f, result);
                if self.frames.len() == base {
                    return result;
                }
                let caller = self.frames.pop().unwrap();
                let done = std::mem::replace(&mut f, caller);
                match result {
                    Ok(v) => {
                        self.set_reg(&f, done.ret, v);
                        break;
                    }
                    Err(e) => {
                        let at = f.pc - 1; // the call instruction
                        match self.catch(&mut f, e, at, false) {
                            None => break,
                            Some(e) => result = Err(e),
                        }
                    }
                }
            }
        }
    }

    /// Mark-sweep collection (mruby: `mrb_full_gc`). Only called at a safe
    /// point, where `current` and the VM's fields hold every live value.
    #[cold]
    #[inline(never)]
    fn collect_garbage(&mut self, current: &Frame) {
        let mut m = Marker::new(&self.heap);
        for f in self.frames.iter().chain(std::iter::once(current)) {
            mark_frame(&mut m, f);
        }
        for v in &self.stack {
            m.value(v);
        }
        m.value(&self.main_obj);
        for v in self.globals.values().chain(&self.at_exit) {
            m.value(v);
        }
        for c in self.c.all() {
            m.class(c);
        }
        let marks = m.finish();
        self.heap.sweep(marks);
    }

    /// Looks for a handler in `f` covering instruction `at`; `None` when one
    /// took the unwind (and `f` continues there). Exceptions go to any
    /// handler, `break`/`return` passing through only run `ensure` clauses.
    #[cold]
    #[inline(never)]
    fn catch(&mut self, f: &mut Frame, e: Unwind, at: usize, own: bool) -> Option<Unwind> {
        let e = self.materialize(e);
        let covers = |h: &&crate::opcode::CatchHandler| h.start <= at && at < h.end;
        match e {
            Unwind::Error(exc) => match f.irep.catch.iter().find(covers).map(|h| h.target) {
                Some(target) => {
                    f.pc = target;
                    f.exc = Some(exc);
                    None
                }
                None => Some(Unwind::Error(exc)),
            },
            e if own => Some(e),
            e => {
                let ensure = f.irep.catch.iter().filter(|h| h.ensure).find(covers);
                match ensure.map(|h| h.target) {
                    Some(target) => {
                        f.pc = target;
                        f.pending = Some(e);
                        f.exc = None;
                        None
                    }
                    None => Some(e),
                }
            }
        }
    }

    /// Registers the `break`/`return` targets of a starting frame.
    fn enter_frame(&mut self, f: &Frame) {
        if f.break_tag != 0 {
            self.live_breaks.push(f.break_tag);
        }
        if f.lambda {
            self.live_breaks.push(f.proc_tag);
        }
        if f.is_home {
            self.live_homes.push(f.home);
        }
    }

    /// Unregisters a finished frame and catches the `break`/`return` aimed at it.
    fn leave_frame(&mut self, f: &Frame, r: VmResult) -> VmResult {
        self.release_frame(f);
        if f.is_home {
            self.live_homes.pop();
        }
        if f.lambda {
            self.live_breaks.pop();
        }
        if f.break_tag != 0 {
            self.live_breaks.pop();
        }
        match r {
            Err(Unwind::Return { home, value }) if f.is_home && home == f.home => Ok(value),
            Err(Unwind::Break { tag, value })
                if tag != 0 && (tag == f.break_tag || (f.lambda && tag == f.proc_tag)) =>
            {
                Ok(value)
            }
            r => r,
        }
    }

    /// A call made by the instruction that writes register `a`: Ruby code
    /// continues in the dispatch loop as a new frame, natives run now. A
    /// `break` from the passed block (`tag`) ends the call with its value.
    fn call_flow(
        &mut self,
        f: &Frame,
        a: Reg,
        tag: u64,
        start: impl FnOnce(&mut Self) -> Result<Call, Unwind>,
    ) -> Result<Flow, Unwind> {
        if tag != 0 {
            self.live_breaks.push(tag);
        }
        let r = start(self);
        if tag != 0 {
            self.live_breaks.pop();
        }
        match r {
            Ok(Call::Frame(mut callee)) => {
                callee.ret = a;
                callee.break_tag = tag;
                Ok(Flow::Call(callee))
            }
            Ok(Call::Done(v)) => {
                self.set_reg(f, a, v);
                Ok(Flow::Next)
            }
            Err(Unwind::Break { tag: t, value }) if tag != 0 && t == tag => {
                self.set_reg(f, a, value);
                Ok(Flow::Next)
            }
            Err(e) => Err(e),
        }
    }

    fn args_from_array(&self, v: Value) -> Vec<Value> {
        match self.kind(v) {
            V::Array(a) => self.heap[a].borrow().clone(),
            _ => vec![v],
        }
    }

    /// Executes one instruction: the simplest ones (and the Integer fast
    /// paths) right here in the dispatch loop, everything else out of line,
    /// so the loop stays small enough to keep its state in registers.
    #[inline(always)]
    fn step(&mut self, f: &mut Frame, op: Op) -> Result<Flow, Unwind> {
        match op {
            Op::Move(a, b) => {
                let v = self.reg(f, b);
                self.set_reg(f, a, v);
            }
            Op::LoadI(a, i) => { let v = self.int(i); self.set_reg(f, a, v) },
            Op::LoadSym(a, s) => self.set_reg(f, a, Value::sym(s)),
            Op::LoadNil(a) => self.set_reg(f, a, Value::NIL),
            Op::LoadSelf(a) => self.set_reg(f, a, f.self_),
            Op::LoadTrue(a) => self.set_reg(f, a, Value::TRUE),
            Op::LoadFalse(a) => self.set_reg(f, a, Value::FALSE),
            Op::Jmp(t) => f.pc = t,
            Op::JmpIf(a, t) => {
                if self.reg(f, a).truthy() {
                    f.pc = t;
                }
            }
            Op::JmpNot(a, t) => {
                if !self.reg(f, a).truthy() {
                    f.pc = t;
                }
            }
            Op::Return(a) => return Ok(Flow::Return(self.reg(f, a))),
            Op::GetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                let v = self.env_get(env, idx);
                self.set_reg(f, a, v);
            }
            Op::SetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                let v = self.reg(f, a);
                self.env_set(env, idx, v);
            }
            Op::BlkPush(a, _) if f.block.is_some() => self.set_reg(f, a, f.block.unwrap()),
            Op::Send(a, mid, argc, has_block) => return self.send_op(f, a, mid, argc, has_block),
            Op::AddI(a, i) | Op::SubI(a, i) => {
                let add = matches!(op, Op::AddI(..));
                if let V::Int(x) = self.kind(self.reg(f, a))
                    && let Some(v) = if add { x.checked_add(i) } else { x.checked_sub(i) }
                {
                    { let v = self.int(v); self.set_reg(f, a, v) };
                } else {
                    return self.step_slow(f);
                }
            }
            Op::Add(a) | Op::Sub(a) | Op::Lt(a) | Op::Le(a) | Op::Gt(a) | Op::Ge(a) => {
                // inline Integers only: the sum of two 48-bit ones can't
                // overflow i64, but may need boxing (the slow path does that)
                let (Some(x), Some(y)) = (self.reg(f, a).as_inline_int(), self.reg(f, a + 1).as_inline_int()) else {
                    return self.step_slow(f);
                };
                let v = match op {
                    Op::Add(_) => match Value::inline_int(x + y) {
                        Some(v) => v,
                        None => return self.step_slow(f),
                    },
                    Op::Sub(_) => match Value::inline_int(x - y) {
                        Some(v) => v,
                        None => return self.step_slow(f),
                    },
                    Op::Lt(_) => Value::bool(x < y),
                    Op::Le(_) => Value::bool(x <= y),
                    Op::Gt(_) => Value::bool(x > y),
                    _ => Value::bool(x >= y),
                };
                self.set_reg(f, a, v);
            }
            _ => return self.step_slow(f),
        }
        Ok(Flow::Next)
    }

    /// `Send`, out of line but separate from [`Vm::step_slow`]: calls and
    /// yields are frequent enough to deserve their own small function.
    #[inline(never)]
    fn send_op(
        &mut self,
        f: &mut Frame,
        a: Reg,
        mid: Sym,
        argc: usize,
        has_block: bool,
    ) -> Result<Flow, Unwind> {
        let recv = self.reg(f, a);
        let blk = if has_block {
            self.block_from(self.reg(f, a + argc + 1))?
        } else {
            None
        };
        let tag = proc_tag(&blk);
        let args = Args::Stack { at: f.base + a + 1, n: argc };
        let Some((m, owner, vis)) = self.cached_lookup(f, &recv, mid) else {
            return self.call_flow(f, a, tag, |vm| vm.start_call(recv, mid, args, blk));
        };
        if vis != Vis::Public && !self.visible(vis, &owner, &recv, &f.self_) {
            return Err(self.not_visible(vis, &recv, mid));
        }
        // natives take their arguments as a slice: no Vec for the common case
        if let Method::Native(native) = m
            && argc <= MAX_INLINE_ARGS
        {
            let mut buf: [Value; MAX_INLINE_ARGS] = Default::default();
            let at = f.base + a + 1;
            buf[..argc].clone_from_slice(&self.stack[at..at + argc]);
            let args = &buf[..argc];
            return self.call_flow(f, a, tag, |vm| {
                Ok(Call::Done(native(vm, &recv, args, blk.as_ref())?))
            });
        }
        self.call_flow(f, a, tag, |vm| vm.start_invoke(m, owner, recv, mid, args, blk))
    }

    /// Every instruction, out of line (see [`Vm::step`]). It reads its
    /// instruction (the one at `f.pc - 1`) itself, so the dispatch loop
    /// doesn't have to copy the 48-byte `Op` to pass it.
    #[inline(never)]
    fn step_slow(&mut self, f: &mut Frame) -> Result<Flow, Unwind> {
        let op = f.irep.code[f.pc - 1];
        match op {
            Op::Nop => {}
            Op::Move(a, b) => self.set_reg(f, a, self.reg(f, b)),
            Op::LoadI(a, i) => { let v = self.int(i); self.set_reg(f, a, v) },
            Op::LoadL(a, i) => {
                let v = match &f.irep.pool[i] {
                    Literal::Float(x) => Value::float(*x),
                    Literal::Str(s) => self.str(s.to_string()),
                };
                self.set_reg(f, a, v);
            }
            Op::LoadSym(a, s) => self.set_reg(f, a, Value::sym(s)),
            Op::LoadNil(a) => self.set_reg(f, a, Value::NIL),
            Op::LoadSelf(a) => self.set_reg(f, a, f.self_),
            Op::LoadTrue(a) => self.set_reg(f, a, Value::TRUE),
            Op::LoadFalse(a) => self.set_reg(f, a, Value::FALSE),
            Op::GetGV(a, s) => self.set_reg(f, a, self.globals.get(&s).cloned().unwrap_or(Value::NIL)),
            Op::SetGV(a, s) => _ = self.globals.insert(s, self.reg(f, a)),
            Op::GetIV(a, s) => self.set_reg(f, a, self.ivar_get(&f.self_, s)),
            Op::SetIV(a, s) => self.ivar_set(&f.self_, s, self.reg(f, a))?,
            Op::GetCV(a, s) => {
                let v = self.cvar_get(&f.target, s)?;
                self.set_reg(f, a, v);
            }
            Op::SetCV(a, s) => self.cvar_set(&f.target, s, self.reg(f, a)),
            Op::GetConst(a, s) => match self.get_const(&f.target, s) {
                Some(v) => self.set_reg(f, a, v),
                None => {
                    return Err(
                        self.name_err(format!("uninitialized constant {}", self.sym_name(s)))
                    );
                }
            },
            Op::SetConst(a, s) => {
                let owner = self.lexical_class(&f.target);
                self.store_const(&owner, s, self.reg(f, a));
            }
            Op::GetMCnst(a, s) => {
                let V::Class(outer) = self.kind(self.reg(f, a)) else {
                    let v = self.reg(f, a);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                match self.get_const_in(&outer, s) {
                    Some(v) => self.set_reg(f, a, v),
                    None => {
                        let msg = format!(
                            "uninitialized constant {}::{}",
                            self.heap[outer].name(),
                            self.sym_name(s)
                        );
                        return Err(self.name_err(msg));
                    }
                }
            }
            Op::SetMCnst(a, s) => {
                let V::Class(outer) = self.kind(self.reg(f, a + 1)) else {
                    let v = self.reg(f, a + 1);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                self.store_const(&outer, s, self.reg(f, a));
            }
            Op::OClass(a) => self.set_reg(f, a, Value::from(self.c.object)),
            Op::GetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                let v = self.env_get(env, idx);
                self.set_reg(f, a, v);
            }
            Op::SetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                let v = self.reg(f, a);
                self.env_set(env, idx, v);
            }
            Op::Jmp(t) => f.pc = t,
            Op::JmpIf(a, t) => {
                if self.reg(f, a).truthy() {
                    f.pc = t;
                }
            }
            Op::JmpNot(a, t) => {
                if !self.reg(f, a).truthy() {
                    f.pc = t;
                }
            }
            Op::JmpNil(a, t) => {
                if self.reg(f, a).is_nil() {
                    f.pc = t;
                }
            }
            Op::OptArg(n, t) => {
                if n < f.opt_filled {
                    f.pc = t;
                }
            }
            Op::KwArg(n, t) => {
                if f.kw_mask & (1 << n.min(63)) != 0 {
                    f.pc = t;
                }
            }
            Op::Except(a) => {
                let exc = f.exc.take().unwrap_or(Value::NIL);
                self.set_reg(f, a, exc);
            }
            Op::Rescue(a, b) => {
                let exc = self.reg(f, a);
                let cv = self.reg(f, b);
                let classes = match self.kind(cv) {
                    V::Array(list) => self.heap[list].borrow().clone(),
                    _ => vec![cv],
                };
                let mut hit = false;
                for c in classes {
                    let V::Class(cls) = self.kind(c) else {
                        return Err(self.type_err("class or module required for rescue clause"));
                    };
                    if self.is_a(&exc, &cls) {
                        hit = true;
                        break;
                    }
                }
                self.set_reg(f, b, Value::bool(hit));
            }
            Op::Raise(a) => {
                let v = self.reg(f, a);
                if v.is_nil()
                    && let Some(pending) = f.pending.take()
                {
                    return Err(pending); // end of an ensure run for a break/return
                }
                return Err(Unwind::Error(v));
            }
            Op::Send(a, mid, argc, has_block) => return self.send_op(f, a, mid, argc, has_block),
            Op::SendV(a, mid, has_block) => {
                let recv = self.reg(f, a);
                let args = self.args_from_array(self.reg(f, a + 1));
                let blk = if has_block {
                    self.block_from(self.reg(f, a + 2))?
                } else {
                    None
                };
                let tag = proc_tag(&blk);
                if let Some((_, owner, vis)) = self.lookup_vis(&recv, mid)
                    && !self.visible(vis, &owner, &recv, &f.self_)
                {
                    return Err(self.not_visible(vis, &recv, mid));
                }
                let args = Args::Owned(args);
                return self.call_flow(f, a, tag, |vm| vm.start_call(recv, mid, args, blk));
            }
            Op::Super(a, argc) => {
                let args: Vec<Value> = (0..argc).map(|i| self.reg(f, a + 1 + i)).collect();
                let blk = self.block_from(self.reg(f, a + argc + 1))?;
                return self.super_flow(f, a, args, blk);
            }
            Op::SuperV(a) => {
                let args = self.args_from_array(self.reg(f, a + 1));
                let blk = self.block_from(self.reg(f, a + 2))?;
                return self.super_flow(f, a, args, blk);
            }
            Op::BlkPush(a, required) => match &f.block {
                Some(b) => self.set_reg(f, a, *b),
                None if required => {
                    return Err(self.err(&self.c.local_jump_error, "no block given (yield)"));
                }
                None => self.set_reg(f, a, Value::NIL),
            },
            Op::BlockGiven(a) => self.set_reg(f, a, Value::bool(f.block.is_some())),
            Op::Return(a) => return Ok(Flow::Return(self.reg(f, a))),
            Op::ReturnBlk(a) => {
                if !self.live_homes.contains(&f.home) {
                    return Err(self.err(&self.c.local_jump_error, "unexpected return"));
                }
                return Err(Unwind::Return {
                    home: f.home,
                    value: self.reg(f, a),
                });
            }
            Op::Break(a) => {
                if !self.live_breaks.contains(&f.proc_tag) {
                    return Err(self.err(&self.c.local_jump_error, "break from proc-closure"));
                }
                return Err(Unwind::Break {
                    tag: f.proc_tag,
                    value: self.reg(f, a),
                });
            }
            Op::AddI(a, i) | Op::SubI(a, i) => {
                let add = matches!(op, Op::AddI(..));
                if let V::Int(x) = self.kind(self.reg(f, a))
                    && let Some(v) = if add { x.checked_add(i) } else { x.checked_sub(i) }
                {
                    { let v = self.int(v); self.set_reg(f, a, v) };
                } else {
                    // no register holds the operand (`a` may be a local's),
                    // so the slow path passes it as an owned argument
                    let (x, y) = (self.reg(f, a), self.int(i));
                    let (op, mid) = if add {
                        (Arith::Add, self.s.add)
                    } else {
                        (Arith::Sub, self.s.sub)
                    };
                    if x.is_numeric() {
                        let v = self.arith(op, &x, &y)?;
                        self.set_reg(f, a, v);
                    } else {
                        let args = Args::Owned(vec![y]);
                        return self.call_flow(f, a, 0, |vm| vm.start_call(x, mid, args, None));
                    }
                }
            }
            Op::Add(a) => return self.arith_flow(f, a, Arith::Add, self.s.add),
            Op::Sub(a) => return self.arith_flow(f, a, Arith::Sub, self.s.sub),
            Op::Mul(a) => return self.arith_flow(f, a, Arith::Mul, self.s.mul),
            Op::Div(a) => return self.arith_flow(f, a, Arith::Div, self.s.div),
            Op::Eq(a) => {
                let (x, y) = (self.reg(f, a), self.reg(f, a + 1));
                let v = self.equal(&x, &y)?;
                self.set_reg(f, a, Value::bool(v));
            }
            Op::Lt(a) => return self.cmp_flow(f, a, self.s.lt, Ordering::is_lt),
            Op::Le(a) => return self.cmp_flow(f, a, self.s.le, Ordering::is_le),
            Op::Gt(a) => return self.cmp_flow(f, a, self.s.gt, Ordering::is_gt),
            Op::Ge(a) => return self.cmp_flow(f, a, self.s.ge, Ordering::is_ge),
            Op::Not(a) => self.set_reg(f, a, Value::bool(!self.reg(f, a).truthy())),
            Op::Array(a, b, n) => {
                let v = (0..n).map(|i| self.reg(f, b + i)).collect();
                let v = self.array(v);
                self.set_reg(f, a, v);
            }
            Op::ArrayCat(a, b) => {
                let items = self.splat_to_vec(&self.reg(f, b))?;
                if let V::Array(arr) = self.kind(self.reg(f, a)) {
                    self.heap[arr].borrow_mut().extend(items);
                }
            }
            Op::ArrayPush(a, b) => {
                if let V::Array(arr) = self.kind(self.reg(f, a)) {
                    let v = self.reg(f, b);
                    self.heap[arr].borrow_mut().push(v);
                }
            }
            Op::ToAry(a, b) => {
                let v = self.reg(f, b);
                let arr = self.masgn_array(&v)?;
                self.set_reg(f, a, Value::from(arr));
            }
            Op::ARef(a, b, i) => {
                let v = match self.kind(self.reg(f, b)) {
                    V::Array(arr) => self.heap[arr].borrow().get(i).cloned().unwrap_or(Value::NIL),
                    _ if i == 0 => self.reg(f, b),
                    _ => Value::NIL,
                };
                self.set_reg(f, a, v);
            }
            Op::ASplat(a, b, pre, post) => {
                let items = self.args_from_array(self.reg(f, b));
                let start = pre.min(items.len());
                let end = items.len().saturating_sub(post).max(start);
                let v = self.array(items[start..end].to_vec());
                self.set_reg(f, a, v);
            }
            Op::APost(a, b, pre, post, j) => {
                let items = self.args_from_array(self.reg(f, b));
                let idx = pre.max(items.len().saturating_sub(post)) + j;
                self.set_reg(f, a, items.get(idx).cloned().unwrap_or(Value::NIL));
            }
            Op::Hash(a, b, n) => {
                let hv = self.hash(RHash::new());
                let V::Hash(h) = self.kind(hv) else {
                    unreachable!()
                };
                for i in 0..n {
                    let (k, v) = (self.reg(f, b + 2 * i), self.reg(f, b + 2 * i + 1));
                    self.hash_set(h, k, v)?;
                }
                self.set_reg(f, a, Value::from(h));
            }
            Op::HashAdd(a, b) => {
                if let V::Hash(h) = self.kind(self.reg(f, a)) {
                    let (k, v) = (self.reg(f, b), self.reg(f, b + 1));
                    self.hash_set(h, k, v)?;
                }
            }
            Op::HashCat(a, b) => {
                let pairs = match self.kind(self.reg(f, b)) {
                    V::Hash(other) => self.heap[other].borrow().to_vec(),
                    V::Nil => vec![],
                    _ => {
                        let what = self.class_name(&self.reg(f, b));
                        return Err(
                            self.type_err(format!("no implicit conversion of {what} into Hash"))
                        );
                    }
                };
                if let V::Hash(h) = self.kind(self.reg(f, a)) {
                    for (k, v) in pairs {
                        self.hash_set(h, k, v)?;
                    }
                }
            }
            Op::KwHash(a) => {
                if let V::Hash(h) = self.kind(self.reg(f, a)) {
                    self.heap[h].borrow_mut().kw = true;
                }
            }
            Op::String(a, i) => {
                let Literal::Str(s) = &f.irep.pool[i] else {
                    unreachable!()
                };
                let v = self.str(s.to_string());
                self.set_reg(f, a, v);
            }
            Op::StrCat(a) => {
                let piece = match self.kind(self.reg(f, a + 1)) {
                    V::Str(s) => self.heap[s].borrow().clone(),
                    _ => self.inspect(&self.reg(f, a + 1))?,
                };
                if let V::Str(s) = self.kind(self.reg(f, a)) {
                    self.heap[s].borrow_mut().push_str(&piece);
                }
            }
            Op::Intern(a) => {
                let v = match self.kind(self.reg(f, a)) {
                    V::Str(s) => {
                        let s = self.heap[s].borrow().clone();
                        Value::sym(self.intern(&s))
                    }
                    _ => self.reg(f, a),
                };
                self.set_reg(f, a, v);
            }
            Op::Range(a, excl) => {
                let (lo, hi) = (self.reg(f, a), self.reg(f, a + 1));
                let v = self.range(lo, hi, excl);
                self.set_reg(f, a, v);
            }
            Op::Block(a, idx) | Op::Lambda(a, idx) => {
                let env = self.frame_env(f);
                let p = RProc {
                    body: ProcBody::Ruby {
                        irep: f.irep.reps[idx].clone(),
                        env,
                    },
                    self_: f.self_,
                    block: f.block,
                    home: f.home,
                    method: f.method,
                    target: f.target,
                    lambda: matches!(op, Op::Lambda(..)),
                };
                let p = Value::from(self.heap.alloc(p));
                self.set_reg(f, a, p);
            }
            Op::TDef(s, idx) => {
                let irep = f.irep.reps[idx].clone();
                // like Ruby, these are private wherever they are defined
                let vis = if [self.s.initialize, self.s.initialize_copy, self.s.respond_to_missing]
                    .contains(&s)
                {
                    Vis::Private
                } else {
                    f.vis
                };
                if f.vis == Vis::ModuleFunction {
                    let meta = self.singleton_class(&Value::from(f.target))?;
                    self.heap[meta].methods.insert(s, Method::Ruby(irep.clone()));
                }
                self.heap[f.target].methods.insert_vis(s, Method::Ruby(irep), vis);
            }
            Op::DefaultVis(vis) => f.vis = vis,
            Op::SDef(a, s, idx) => {
                let cls = self.singleton_class(&self.reg(f, a))?;
                self.heap[cls].methods.insert(s, Method::Ruby(f.irep.reps[idx].clone()));
            }
            Op::Class(a, name, is_module) => {
                let cls = self.define_class(f, a, name, is_module)?;
                self.set_reg(f, a, Value::from(cls));
            }
            Op::SClass(a) => {
                let cls = self.singleton_class(&self.reg(f, a))?;
                self.set_reg(f, a, Value::from(cls));
            }
            Op::Exec(a, idx) => {
                let V::Class(cls) = self.kind(self.reg(f, a)) else {
                    unreachable!()
                };
                let target = cls;
                let mut body = self.body_frame(f.irep.reps[idx].clone(), Value::from(cls), target);
                body.ret = a;
                return Ok(Flow::Call(body));
            }
            Op::Alias(new, old) => self.alias_method(&f.target, new, old)?,
            Op::GAlias(new, old) => {
                let v = self.globals.get(&old).cloned().unwrap_or(Value::NIL);
                self.globals.insert(new, v);
            }
            Op::Undef(s) => self.undef_method(&f.target, s)?,
            Op::Defined(a, kind, s) => {
                let v = match self.defined(f, kind, s) {
                    Some(d) => self.str(d),
                    None => Value::NIL,
                };
                self.set_reg(f, a, v);
            }
            Op::CurMethod(a) => {
                let v = f
                    .method
                    .as_ref()
                    .map(|(_, m)| Value::sym(*m))
                    .unwrap_or(Value::NIL);
                self.set_reg(f, a, v);
            }
        }
        Ok(Flow::Next)
    }

    fn super_flow(
        &mut self,
        f: &Frame,
        a: Reg,
        args: Vec<Value>,
        blk: Option<Value>,
    ) -> Result<Flow, Unwind> {
        let Some((owner, mid)) = f.method else {
            return Err(self.err(&self.c.no_method_error, "super called outside of method"));
        };
        let self_ = f.self_;
        let tag = proc_tag(&blk);
        if let Some((m, o)) = self.find_super(&self_, &owner, mid) {
            let args = Args::Owned(args);
            return self.call_flow(f, a, tag, |vm| vm.start_invoke(m, o, self_, mid, args, blk));
        }
        if let Some((mm, mo)) = self.lookup(&self_, self.s.method_missing) {
            let mut margs = vec![Value::sym(mid)];
            margs.extend(args);
            let name = self.s.method_missing;
            let margs = Args::Owned(margs);
            return self.call_flow(f, a, tag, |vm| vm.start_invoke(mm, mo, self_, name, margs, blk));
        }
        let msg = format!("super: no superclass method '{}'", self.sym_name(mid));
        Err(self.err(&self.c.no_method_error, msg))
    }

    pub fn alias_method(&self, target: &ClassRef, new: Sym, old: Sym) -> Result<(), Unwind> {
        match self.find_method(target, old) {
            Some((m, _)) => {
                self.heap[target].methods.insert(new, m);
                Ok(())
            }
            None => Err(self.name_err(format!(
                "undefined method '{}' for class '{}'",
                self.sym_name(old),
                self.heap[target].name()
            ))),
        }
    }

    pub fn undef_method(&self, target: &ClassRef, s: Sym) -> Result<(), Unwind> {
        if self.find_method(target, s).is_none() {
            return Err(self.name_err(format!(
                "undefined method '{}' for class '{}'",
                self.sym_name(s),
                self.heap[target].name()
            )));
        }
        self.heap[target].methods.insert(s, Method::Undefined);
        Ok(())
    }

    fn defined(&mut self, f: &Frame, kind: DefinedKind, s: Sym) -> Option<&'static str> {
        let yes = match kind {
            DefinedKind::IVar => self.ivar_lookup(&f.self_, s).is_some(),
            DefinedKind::GVar => self.globals.contains_key(&s),
            DefinedKind::CVar => {
                let base = self.lexical_class(&f.target);
                self.cvar_holder(&base, s).is_some()
            }
            DefinedKind::Const => self.get_const(&f.target, s).is_some(),
            DefinedKind::ScopedConst(r) => match self.kind(self.reg(f, r)) {
                V::Class(c) => self.get_const_in(&c, s).is_some(),
                _ => false,
            },
            DefinedKind::Method(r, _) => self.respond_to(&self.reg(f, r), s),
            DefinedKind::Super => match &f.method {
                Some((owner, mid)) => self.find_super(&f.self_, owner, *mid).is_some(),
                None => false,
            },
        };
        if !yes {
            return None;
        }
        Some(match kind {
            DefinedKind::IVar => "instance-variable",
            DefinedKind::GVar => "global-variable",
            DefinedKind::CVar => "class variable",
            DefinedKind::Const | DefinedKind::ScopedConst(_) => "constant",
            DefinedKind::Method(..) => "method",
            DefinedKind::Super => "super",
        })
    }

    fn define_class(
        &mut self,
        f: &Frame,
        a: Reg,
        name: Sym,
        is_module: bool,
    ) -> Result<ClassRef, Unwind> {
        let outer = match self.kind(self.reg(f, a)) {
            V::Class(c) => c,
            V::Nil => self.lexical_class(&f.target),
            _ => {
                let what = self.inspect(&self.reg(f, a))?;
                return Err(self.type_err(format!("{what} is not a class/module")));
            }
        };
        let name_str = self.sym_name(name).to_string();
        let existing = self.heap[outer].consts.borrow().get(&name).cloned();
        match existing.map(|v| self.kind(v)) {
            Some(V::Class(c)) if self.heap[c].is_module == is_module => {
                if let V::Class(sup) = self.kind(self.reg(f, a + 1)) {
                    let same = self.heap[c].superclass == Some(sup);
                    if !is_module && !same {
                        return Err(
                            self.type_err(format!("superclass mismatch for class {name_str}"))
                        );
                    }
                }
                Ok(c)
            }
            Some(_) => Err(self.type_err(format!(
                "{name_str} is not a {}",
                if is_module { "module" } else { "class" }
            ))),
            None => {
                let superclass = if is_module {
                    None
                } else {
                    match self.kind(self.reg(f, a + 1)) {
                        V::Nil => Some(self.c.object),
                        V::Class(s) if !self.heap[s].is_module && !self.heap[s].is_singleton => Some(s),
                        _ => return Err(self.type_err("superclass must be a Class")),
                    }
                };
                let full = if outer == self.c.object {
                    name_str
                } else {
                    format!("{}::{}", self.heap[outer].name(), name_str)
                };
                let cls = self.new_class(&full, superclass, is_module);
                if outer != self.c.object {
                    self.heap[cls].outer.set(Some(outer));
                }
                self.heap[outer].consts.borrow_mut().insert(name, Value::from(cls));
                Ok(cls)
            }
        }
    }

    // ---- arithmetic & comparison (src/numeric.c, src/numops.c) ---------

    pub fn arith(&mut self, op: Arith, x: &Value, y: &Value) -> VmResult {
        use V::{Float, Int};
        Ok(match (self.kind(*x), self.kind(*y)) {
            (Int(a), Int(b)) => return self.int_arith(op, a, b),
            (Int(a), Float(b)) => Value::float(float_arith(op, a as f64, b)),
            (Float(a), Int(b)) => Value::float(float_arith(op, a, b as f64)),
            (Float(a), Float(b)) => Value::float(float_arith(op, a, b)),
            _ => {
                let what = match self.kind(*y) {
                    V::Nil => "nil".to_string(),
                    _ => self.class_name(y),
                };
                return Err(self.type_err(format!(
                    "{} can't be coerced into {}",
                    what,
                    self.class_name(x)
                )));
            }
        })
    }

    fn int_arith(&mut self, op: Arith, a: i64, b: i64) -> VmResult {
        // without Bignum (mruby-bigint), overflow falls back to Float
        let overflow = Value::float(float_arith(op, a as f64, b as f64));
        let zero_div = |vm: &Self| vm.err(&vm.c.zero_division_error, "divided by 0");
        let n = match op {
            Arith::Add => a.checked_add(b),
            Arith::Sub => a.checked_sub(b),
            Arith::Mul => a.checked_mul(b),
            Arith::Div => {
                if b == 0 {
                    return Err(zero_div(self));
                }
                match a.checked_div(b) {
                    Some(q) if a % b != 0 && ((a < 0) != (b < 0)) => Some(q - 1),
                    q => q,
                }
            }
            Arith::Mod => {
                if b == 0 {
                    return Err(zero_div(self));
                }
                let r = a.wrapping_rem(b);
                Some(if r != 0 && ((r < 0) != (b < 0)) { r + b } else { r })
            }
            Arith::Pow => match u32::try_from(b) {
                Ok(e) => a.checked_pow(e),
                Err(_) if b < 0 => return Ok(Value::float((a as f64).powf(b as f64))),
                Err(_) => None,
            },
        };
        Ok(match n {
            Some(n) => self.int(n),
            None => overflow,
        })
    }

    /// `R[a] op R[a+1]`: numbers directly, anything else as a method call.
    fn arith_flow(&mut self, f: &Frame, a: Reg, op: Arith, mid: Sym) -> Result<Flow, Unwind> {
        let (x, y) = (self.reg(f, a), self.reg(f, a + 1));
        if x.is_numeric() && y.is_numeric() {
            let v = self.arith(op, &x, &y)?;
            self.set_reg(f, a, v);
            return Ok(Flow::Next);
        }
        let args = Args::Stack { at: f.base + a + 1, n: 1 };
        self.call_flow(f, a, 0, |vm| vm.start_call(x, mid, args, None))
    }

    fn cmp_flow(
        &mut self,
        f: &Frame,
        a: Reg,
        mid: Sym,
        pred: fn(Ordering) -> bool,
    ) -> Result<Flow, Unwind> {
        let (x, y) = (self.reg(f, a), self.reg(f, a + 1));
        if x.is_numeric() && y.is_numeric() {
            self.set_reg(f, a, Value::bool(num_cmp(&self.heap, &x, &y).is_some_and(pred)));
            return Ok(Flow::Next);
        }
        let args = Args::Stack { at: f.base + a + 1, n: 1 };
        self.call_flow(f, a, 0, |vm| vm.start_call(x, mid, args, None))
    }

    /// `a == b`, dispatching to user-defined `==` for objects.
    pub fn equal(&mut self, a: &Value, b: &Value) -> Result<bool, Unwind> {
        if let V::Object(_) = self.kind(*a) {
            return Ok(self
                .call_method(a, self.s.eq, vec![*b], None)?
                .truthy());
        }
        Ok(values_eq(&self.heap, a, b))
    }

    // ---- conversions (src/print.c, object.c) ---------------------------

    pub fn to_s(&mut self, v: &Value) -> Result<String, Unwind> {
        Ok(match self.kind(*v) {
            V::Str(s) => self.heap[s].borrow().clone(),
            V::Nil => String::new(),
            V::Bool(b) => b.to_string(),
            V::Int(i) => i.to_string(),
            V::Float(x) => fmt_float(x),
            V::Sym(s) => self.sym_name(s).to_string(),
            _ => match self.call_method(v, self.s.to_s, vec![], None).map(|r| self.kind(r))? {
                V::Str(s) => self.heap[s].borrow().clone(),
                _ => format!("#<{}>", self.class_name(v)),
            },
        })
    }

    fn inspect_array(&mut self, a: ArrayRef) -> Result<String, Unwind> {
        let items = self.heap[a].borrow().clone();
        let mut parts = Vec::with_capacity(items.len());
        for x in &items {
            parts.push(self.inspect(x)?);
        }
        Ok(format!("[{}]", parts.join(", ")))
    }

    fn inspect_hash(&mut self, h: HashRef) -> Result<String, Unwind> {
        let pairs = self.heap[h].borrow().to_vec();
        if pairs.is_empty() {
            return Ok("{}".into());
        }
        let mut parts = Vec::with_capacity(pairs.len());
        for (k, val) in &pairs {
            let vs = self.inspect(val)?;
            parts.push(match self.kind(*k) {
                V::Sym(s) if is_simple_sym(self.sym_name(s)) => {
                    format!("{}: {}", self.sym_name(s), vs)
                }
                V::Sym(s) => format!("{}: {}", inspect_str(self.sym_name(s)), vs),
                _ => format!("{} => {}", self.inspect(k)?, vs),
            });
        }
        Ok(format!("{{{}}}", parts.join(", ")))
    }

    pub fn inspect(&mut self, v: &Value) -> Result<String, Unwind> {
        Ok(match self.kind(*v) {
            V::Nil => "nil".into(),
            V::Bool(b) => b.to_string(),
            V::Int(i) => i.to_string(),
            V::Float(x) => fmt_float(x),
            V::Sym(s) => inspect_sym(self.sym_name(s)),
            V::Str(s) => inspect_str(&self.heap[s].borrow()),
            V::Array(a) => {
                let id = a.id();
                if self.inspecting.contains(&id) {
                    return Ok("[...]".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_array(a);
                self.inspecting.pop();
                r?
            }
            V::Hash(h) => {
                let id = h.id();
                if self.inspecting.contains(&id) {
                    return Ok("{...}".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_hash(h);
                self.inspecting.pop();
                r?
            }
            V::Range(r) => {
                let (lo, hi, excl) = self.heap[r];
                let lo = if lo.is_nil() { String::new() } else { self.inspect(&lo)? };
                let hi = if hi.is_nil() { String::new() } else { self.inspect(&hi)? };
                format!("{lo}{}{hi}", if excl { "..." } else { ".." })
            }
            V::Class(_) | V::Proc(_) | V::Object(_) => {
                match self.call_method(v, self.s.inspect, vec![], None).map(|r| self.kind(r))? {
                    V::Str(s) => self.heap[s].borrow().clone(),
                    _ => format!("#<{}>", self.class_name(v)),
                }
            }
        })
    }
}

fn mark_frame(m: &mut Marker, f: &Frame) {
    for e in f.env.iter().chain(&f.upper) {
        m.env(*e);
    }
    m.value(&f.self_);
    for v in f.block.iter().chain(&f.exc) {
        m.value(v);
    }
    m.class(f.target);
    if let Some((owner, _)) = f.method {
        m.class(owner);
    }
    match &f.pending {
        Some(Unwind::Error(v) | Unwind::Break { value: v, .. } | Unwind::Return { value: v, .. }) => {
            m.value(v)
        }
        Some(Unwind::Raise(c, _)) => m.class(*c),
        None => {}
    }
}

/// Gives class `c` its singleton class.
fn attach_meta(heap: &mut Heap, c: ClassRef, class: ClassRef, module: ClassRef) {
    let cc = &heap[c];
    let sup = if cc.is_singleton {
        class
    } else if cc.is_module {
        module
    } else {
        match cc.superclass {
            Some(s) => heap[s].meta.get().unwrap_or(class),
            None => class,
        }
    };
    let name = format!("#<Class:{}>", cc.name());
    let meta = heap.alloc(RClass::new_singleton(name, sup));
    heap[meta].attached.set(Some(c));
    heap[c].meta.set(Some(meta));
}

fn proc_tag(blk: &Option<Value>) -> u64 {
    match blk.map(|v| v.decode()) {
        Some(Ok(V::Proc(p))) => p.id(),
        _ => 0,
    }
}

fn is_simple_sym(s: &str) -> bool {
    let mut chars = s.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || (c as u32) > 0x7f);
    let body = s.trim_end_matches(['?', '!', '=']);
    first_ok
        && body
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || (c as u32) > 0x7f)
}

pub fn inspect_sym(s: &str) -> String {
    const OPS: &[&str] = &[
        "+", "-", "*", "/", "%", "**", "==", "===", "=~", "!=", "!~", "<=>", "<", "<=", ">", ">=",
        "<<", ">>", "!", "[]", "[]=", "+@", "-@", "~", "&", "|", "^",
    ];
    let var = (s.starts_with("@@") && is_simple_sym(&s[2..]))
        || (s.starts_with('@') && is_simple_sym(&s[1..]))
        || (s.starts_with('$') && is_simple_sym(&s[1..]));
    if is_simple_sym(s) || OPS.contains(&s) || var {
        format!(":{s}")
    } else {
        format!(":{}", inspect_str(s))
    }
}

pub fn inspect_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x1b' => out.push_str("\\e"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0b' => out.push_str("\\v"),
            '\x0c' => out.push_str("\\f"),
            '\0' => out.push_str("\\0"),
            '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
