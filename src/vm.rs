//! The virtual machine: `src/vm.c`, `src/state.c`, and the dispatch parts of
//! `src/class.c` / `src/error.c` in mruby.
//!
//! Differences from mruby worth knowing:
//! * Like mruby's `mrb_vm_exec`, one dispatch loop ([`Vm::run_frames`]) runs
//!   Ruby→Ruby calls on an explicit frame stack (`Vm::frames`, mruby's
//!   callinfo stack) without native recursion. A native method calling back
//!   into Ruby (`Array#each` yielding) starts a nested loop, as mruby's C
//!   functions do; that native recursion is bounded by `Vm::stack_budget`.
//! * Registers live in a heap `Env` per frame (blocks capture it), not in
//!   one contiguous value stack.
//! * mruby unwinds with `setjmp/longjmp`; here unwinding is `Result<_, Unwind>`
//!   propagated with `?`, and each frame consults its irep's catch table.

use crate::gc::{ArrayRef, EnvRef, HashRef, Heap, Marker};
use crate::opcode::{ArgSpec, DefinedKind, Irep, Literal, Op, Reg};
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
    Raise(Rc<RClass>, String),
    /// `break` out of a block; caught by the call site that passed the block (`tag`).
    Break { tag: u64, value: Value },
    /// `return` from inside a block; caught by the method frame `home`.
    Return { home: usize, value: Value },
}

pub type VmResult = Result<Value, Unwind>;

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
    pub basic_object: Rc<RClass>,
    pub object: Rc<RClass>,
    pub kernel: Rc<RClass>,
    pub module: Rc<RClass>,
    pub class: Rc<RClass>,
    pub comparable: Rc<RClass>,
    pub enumerable: Rc<RClass>,
    pub numeric: Rc<RClass>,
    pub integer: Rc<RClass>,
    pub float: Rc<RClass>,
    pub string: Rc<RClass>,
    pub symbol: Rc<RClass>,
    pub array: Rc<RClass>,
    pub hash: Rc<RClass>,
    pub range: Rc<RClass>,
    pub proc_: Rc<RClass>,
    pub nil: Rc<RClass>,
    pub true_: Rc<RClass>,
    pub false_: Rc<RClass>,
    pub exception: Rc<RClass>,
    pub script_error: Rc<RClass>,
    pub syntax_error: Rc<RClass>,
    pub not_implemented_error: Rc<RClass>,
    pub standard_error: Rc<RClass>,
    pub runtime_error: Rc<RClass>,
    pub frozen_error: Rc<RClass>,
    pub argument_error: Rc<RClass>,
    pub type_error: Rc<RClass>,
    pub name_error: Rc<RClass>,
    pub no_method_error: Rc<RClass>,
    pub zero_division_error: Rc<RClass>,
    pub local_jump_error: Rc<RClass>,
    pub index_error: Rc<RClass>,
    pub key_error: Rc<RClass>,
    pub stop_iteration: Rc<RClass>,
    pub range_error: Rc<RClass>,
    pub float_domain_error: Rc<RClass>,
    pub no_matching_pattern_error: Rc<RClass>,
    pub no_matching_pattern_key_error: Rc<RClass>,
    pub stack_error: Rc<RClass>,
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
}

pub struct Vm {
    pub heap: Heap,
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
    env: EnvRef,
    self_: Value,
    block: Option<Value>,
    /// Where `def` puts methods and where constants are looked up first.
    target: Rc<RClass>,
    /// (owner, name) of the running method, for `super` and `__method__`.
    method: Option<(Rc<RClass>, Sym)>,
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
}

impl Frame {
    fn new(irep: Rc<Irep>, env: EnvRef, self_: Value, target: Rc<RClass>) -> Frame {
        Frame {
            irep,
            env,
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

/// Bound arguments: the frame's registers plus what the prologue needs.
struct Bound {
    env: EnvRef,
    opt_filled: usize,
    kw_mask: u64,
}

/// Overrides for running a proc as something else (`instance_eval`,
/// `class_eval`, `define_method`).
#[derive(Default)]
pub struct ProcCall {
    pub self_: Option<Value>,
    pub target: Option<Rc<RClass>>,
    pub method: Option<(Rc<RClass>, Sym)>,
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
        let basic_object = RClass::new("BasicObject", None, false);
        let object = RClass::new("Object", Some(basic_object.clone()), false);
        let kernel = RClass::new("Kernel", None, true);
        object.includes.borrow_mut().push(kernel.clone());
        let sub = |name: &str, sup: &Rc<RClass>| RClass::new(name, Some(sup.clone()), false);
        let module = sub("Module", &object);
        let class = sub("Class", &module);
        let numeric = sub("Numeric", &object);
        let exception = sub("Exception", &object);
        let script_error = sub("ScriptError", &exception);
        let standard_error = sub("StandardError", &exception);
        let runtime_error = sub("RuntimeError", &standard_error);
        let name_error = sub("NameError", &standard_error);
        let index_error = sub("IndexError", &standard_error);
        let range_error = sub("RangeError", &standard_error);
        let no_matching_pattern_error = sub("NoMatchingPatternError", &standard_error);
        let c = Classes {
            comparable: RClass::new("Comparable", None, true),
            enumerable: RClass::new("Enumerable", None, true),
            integer: sub("Integer", &numeric),
            float: sub("Float", &numeric),
            string: sub("String", &object),
            symbol: sub("Symbol", &object),
            array: sub("Array", &object),
            hash: sub("Hash", &object),
            range: sub("Range", &object),
            proc_: sub("Proc", &object),
            nil: sub("NilClass", &object),
            true_: sub("TrueClass", &object),
            false_: sub("FalseClass", &object),
            syntax_error: sub("SyntaxError", &script_error),
            not_implemented_error: sub("NotImplementedError", &script_error),
            frozen_error: sub("FrozenError", &runtime_error),
            argument_error: sub("ArgumentError", &standard_error),
            type_error: sub("TypeError", &standard_error),
            no_method_error: sub("NoMethodError", &name_error),
            zero_division_error: sub("ZeroDivisionError", &standard_error),
            local_jump_error: sub("LocalJumpError", &standard_error),
            key_error: sub("KeyError", &index_error),
            stop_iteration: sub("StopIteration", &index_error),
            float_domain_error: sub("FloatDomainError", &range_error),
            no_matching_pattern_key_error: sub(
                "NoMatchingPatternKeyError",
                &no_matching_pattern_error,
            ),
            stack_error: sub("SystemStackError", &exception),
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
        let all = [
            &c.basic_object,
            &c.object,
            &c.kernel,
            &c.module,
            &c.class,
            &c.comparable,
            &c.enumerable,
            &c.numeric,
            &c.integer,
            &c.float,
            &c.string,
            &c.symbol,
            &c.array,
            &c.hash,
            &c.range,
            &c.proc_,
            &c.nil,
            &c.true_,
            &c.false_,
            &c.exception,
            &c.script_error,
            &c.syntax_error,
            &c.not_implemented_error,
            &c.standard_error,
            &c.runtime_error,
            &c.frozen_error,
            &c.argument_error,
            &c.type_error,
            &c.name_error,
            &c.no_method_error,
            &c.zero_division_error,
            &c.local_jump_error,
            &c.index_error,
            &c.key_error,
            &c.stop_iteration,
            &c.range_error,
            &c.float_domain_error,
            &c.no_matching_pattern_error,
            &c.no_matching_pattern_key_error,
            &c.stack_error,
        ];
        for cls in all {
            let s = syms.intern(&cls.name());
            c.object
                .consts
                .borrow_mut()
                .insert(s, Value::Class(cls.clone()));
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
            to_proc: syms.intern("to_proc"),
        };
        let mut heap = Heap::default();
        let main_obj = Value::Object(heap.alloc(RObject::new(c.object.clone())));
        let mut vm = Vm {
            heap,
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
        vm
    }

    // ---- heap values ---------------------------------------------------

    pub fn str(&mut self, s: impl Into<String>) -> Value {
        Value::Str(self.heap.alloc(RefCell::new(s.into())))
    }

    pub fn array(&mut self, v: Vec<Value>) -> Value {
        Value::Array(self.heap.alloc(RefCell::new(v)))
    }

    pub fn hash(&mut self, h: RHash) -> Value {
        Value::Hash(self.heap.alloc(RefCell::new(h)))
    }

    pub fn range(&mut self, lo: Value, hi: Value, excl: bool) -> Value {
        Value::Range(self.heap.alloc((lo, hi, excl)))
    }

    pub fn object(&mut self, o: RObject) -> Value {
        Value::Object(self.heap.alloc(o))
    }

    /// `h[k] = v`. A new String key is copied, as Ruby dups (and freezes) them.
    pub fn hash_set(&mut self, h: HashRef, k: Value, v: Value) {
        let k = match &k {
            Value::Str(s) if !self.heap[h].borrow().contains(&self.heap, &k) => {
                let copy = self.heap[s].borrow().clone();
                self.str(copy)
            }
            _ => k,
        };
        self.heap[h].borrow_mut().set(&self.heap, k, v);
    }

    /// Contents of a String value.
    pub fn string(&self, v: &Value) -> Option<String> {
        match v {
            Value::Str(s) => Some(self.heap[s].borrow().clone()),
            _ => None,
        }
    }

    /// Elements of an Array value.
    pub fn elements(&self, v: &Value) -> Option<Vec<Value>> {
        match v {
            Value::Array(a) => Some(self.heap[a].borrow().clone()),
            _ => None,
        }
    }

    /// The env `depth + 1` levels out from the frame's (a block's outer scopes).
    fn upvar_env(&self, f: &Frame, depth: usize) -> EnvRef {
        let mut env = self.heap[f.env].upper.expect("upvar outside of block");
        for _ in 0..depth {
            env = self.heap[env].upper.expect("upvar depth");
        }
        env
    }

    fn reg(&self, f: &Frame, i: Reg) -> Value {
        self.heap[f.env].regs.borrow()[i].clone()
    }

    fn set_reg(&self, f: &Frame, i: Reg, v: Value) {
        self.heap[f.env].regs.borrow_mut()[i] = v;
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

    pub fn def(&mut self, cls: &Rc<RClass>, name: &str, f: NativeFn) {
        let s = self.intern(name);
        cls.methods.borrow_mut().insert(s, Method::Native(f));
    }

    pub fn def_singleton(&mut self, cls: &Rc<RClass>, name: &str, f: NativeFn) {
        let s = self.intern(name);
        self.class_singleton(cls)
            .methods
            .borrow_mut()
            .insert(s, Method::Native(f));
    }

    pub fn set_const(&mut self, name: &str, v: Value) {
        let s = self.intern(name);
        self.c.object.consts.borrow_mut().insert(s, v);
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

    pub fn make_exception(&mut self, cls: &Rc<RClass>, msg: impl Into<String>) -> Value {
        let msg = self.str(msg);
        let obj = RObject::new(cls.clone());
        obj.ivars.borrow_mut().set(self.s.message, msg);
        self.object(obj)
    }

    pub fn err(&self, cls: &Rc<RClass>, msg: impl Into<String>) -> Unwind {
        Unwind::Raise(cls.clone(), msg.into())
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
        if let Value::Object(o) = e
            && let Some(Value::Str(s)) = self.heap[o].ivars.borrow().get(self.s.message)
        {
            return self.heap[s].borrow().clone();
        }
        self.class_of(e).name()
    }

    // ---- classes & method lookup (src/class.c) -------------------------

    /// The class `obj.class` reports (never a singleton class).
    pub fn class_of(&self, v: &Value) -> Rc<RClass> {
        let c = &self.c;
        match v {
            Value::Nil => c.nil.clone(),
            Value::Bool(true) => c.true_.clone(),
            Value::Bool(false) => c.false_.clone(),
            Value::Int(_) => c.integer.clone(),
            Value::Float(_) => c.float.clone(),
            Value::Sym(_) => c.symbol.clone(),
            Value::Str(_) => c.string.clone(),
            Value::Array(_) => c.array.clone(),
            Value::Hash(_) => c.hash.clone(),
            Value::Range(_) => c.range.clone(),
            Value::Proc(_) => c.proc_.clone(),
            Value::Class(k) => {
                if k.is_module {
                    c.module.clone()
                } else {
                    c.class.clone()
                }
            }
            Value::Object(o) => self.heap[o].class.clone(),
        }
    }

    pub fn class_name(&self, v: &Value) -> String {
        self.class_of(v).name()
    }

    /// The singleton class of a class or module, created on demand. Its
    /// superclass is the superclass's singleton class, so class methods are
    /// inherited (mruby: `prepare_singleton_class`).
    pub fn class_singleton(&self, c: &Rc<RClass>) -> Rc<RClass> {
        if let Some(m) = c.meta.borrow().as_ref() {
            return m.clone();
        }
        let sup = if c.is_singleton {
            self.c.class.clone()
        } else if c.is_module {
            self.c.module.clone()
        } else {
            match &c.superclass {
                Some(s) => self.class_singleton(s),
                None => self.c.class.clone(),
            }
        };
        let m = RClass::new_singleton(format!("#<Class:{}>", c.name()), sup);
        *m.attached.borrow_mut() = Some(Rc::downgrade(c));
        *c.meta.borrow_mut() = Some(m.clone());
        m
    }

    /// `obj.singleton_class`
    pub fn singleton_class(&self, v: &Value) -> Result<Rc<RClass>, Unwind> {
        match v {
            Value::Class(c) => Ok(self.class_singleton(c)),
            Value::Object(o) => {
                if let Some(s) = self.heap[o].singleton.borrow().as_ref() {
                    return Ok(s.clone());
                }
                let s = RClass::new_singleton(
                    format!("#<Class:#<{}>>", self.heap[o].class.name()),
                    self.heap[o].class.clone(),
                );
                *self.heap[o].singleton.borrow_mut() = Some(s.clone());
                Ok(s)
            }
            Value::Nil => Ok(self.c.nil.clone()),
            Value::Bool(true) => Ok(self.c.true_.clone()),
            Value::Bool(false) => Ok(self.c.false_.clone()),
            _ => Err(self.type_err(format!("can't define singleton for {}", self.class_name(v)))),
        }
    }

    /// The class where method lookup for `v` starts.
    pub fn dispatch_class(&self, v: &Value) -> Rc<RClass> {
        match v {
            Value::Class(c) => self.class_singleton(c),
            Value::Object(o) => {
                let o = &self.heap[o];
                o.singleton.borrow().clone().unwrap_or_else(|| o.class.clone())
            }
            _ => self.class_of(v),
        }
    }

    /// Method resolution order: prepended modules, the class, its included
    /// modules (latest first), then the superclass.
    pub fn ancestors(cls: &Rc<RClass>) -> Vec<Rc<RClass>> {
        let mut out: Vec<Rc<RClass>> = Vec::new();
        let add = |out: &mut Vec<Rc<RClass>>, k: Rc<RClass>| {
            if !out.iter().any(|x| Rc::ptr_eq(x, &k)) {
                out.push(k);
            }
        };
        let mut c = Some(cls.clone());
        while let Some(k) = c {
            for p in k.prepends.borrow().iter().rev() {
                for a in Self::ancestors(p) {
                    add(&mut out, a);
                }
            }
            add(&mut out, k.clone());
            for m in k.includes.borrow().iter().rev() {
                for a in Self::ancestors(m) {
                    add(&mut out, a);
                }
            }
            c = k.superclass.clone();
        }
        out
    }

    fn find_in_module(k: &Rc<RClass>, mid: Sym) -> Option<Option<(Method, Rc<RClass>)>> {
        for p in k.prepends.borrow().iter().rev() {
            if let Some(found) = Self::find_in_module(p, mid) {
                return Some(found);
            }
        }
        if let Some(m) = k.methods.borrow().get(&mid) {
            return Some(match m {
                Method::Undefined => None,
                m => Some((m.clone(), k.clone())),
            });
        }
        for inc in k.includes.borrow().iter().rev() {
            if let Some(found) = Self::find_in_module(inc, mid) {
                return Some(found);
            }
        }
        None
    }

    /// Finds `mid` starting at `cls`; returns the method and its owner.
    pub fn find_method(cls: &Rc<RClass>, mid: Sym) -> Option<(Method, Rc<RClass>)> {
        let mut c = Some(cls.clone());
        while let Some(k) = c {
            if let Some(found) = Self::find_in_module(&k, mid) {
                return found;
            }
            c = k.superclass.clone();
        }
        None
    }

    fn lookup(&self, recv: &Value, mid: Sym) -> Option<(Method, Rc<RClass>)> {
        Self::find_method(&self.dispatch_class(recv), mid)
    }

    pub fn respond_to(&self, v: &Value, mid: Sym) -> bool {
        self.lookup(v, mid).is_some()
    }

    pub fn is_a(&self, v: &Value, cls: &Rc<RClass>) -> bool {
        Self::ancestors(&self.dispatch_class(v))
            .iter()
            .any(|c| Rc::ptr_eq(c, cls))
    }

    fn find_super(
        &self,
        recv: &Value,
        owner: &Rc<RClass>,
        mid: Sym,
    ) -> Option<(Method, Rc<RClass>)> {
        let anc = Self::ancestors(&self.dispatch_class(recv));
        let pos = anc.iter().position(|c| Rc::ptr_eq(c, owner))?;
        for k in &anc[pos + 1..] {
            match k.methods.borrow().get(&mid) {
                Some(Method::Undefined) => return None,
                Some(m) => return Some((m.clone(), k.clone())),
                None => {}
            }
        }
        None
    }

    /// The class used for constants and class variables: singleton classes
    /// of classes defer to the class itself.
    fn lexical_class(&self, target: &Rc<RClass>) -> Rc<RClass> {
        if target.is_singleton
            && let Some(c) = target.attached_class()
        {
            return c;
        }
        target.clone()
    }

    pub fn get_const(&self, target: &Rc<RClass>, name: Sym) -> Option<Value> {
        let start = self.lexical_class(target);
        // lexically enclosing classes first, then the ancestors
        let mut lex = start.outer_class();
        if let Some(v) = start.consts.borrow().get(&name) {
            return Some(v.clone());
        }
        while let Some(o) = lex {
            if let Some(v) = o.consts.borrow().get(&name) {
                return Some(v.clone());
            }
            lex = o.outer_class();
        }
        self.get_const_in(&start, name)
            .or_else(|| self.c.object.consts.borrow().get(&name).cloned())
    }

    /// `Mod::Name`: the module and its ancestors (Object only if it's one of them).
    pub fn get_const_in(&self, cls: &Rc<RClass>, name: Sym) -> Option<Value> {
        Self::ancestors(cls)
            .into_iter()
            .find_map(|k| k.consts.borrow().get(&name).cloned())
    }

    /// Stores a constant, naming an anonymous class after it.
    pub fn store_const(&self, owner: &Rc<RClass>, name: Sym, v: Value) {
        if let Value::Class(c) = &v
            && c.is_anonymous()
        {
            let n = self.sym_name(name);
            let full = if Rc::ptr_eq(owner, &self.c.object) {
                n.to_string()
            } else {
                format!("{}::{}", owner.name(), n)
            };
            c.set_name(full);
            if !Rc::ptr_eq(owner, &self.c.object) {
                *c.outer.borrow_mut() = Some(Rc::downgrade(owner));
            }
        }
        owner.consts.borrow_mut().insert(name, v);
    }

    fn cvar_holder(&self, cls: &Rc<RClass>, name: Sym) -> Option<Rc<RClass>> {
        Self::ancestors(cls)
            .into_iter()
            .find(|k| k.cvars.borrow().get(name).is_some())
    }

    fn cvar_get(&self, target: &Rc<RClass>, name: Sym) -> VmResult {
        let base = self.lexical_class(target);
        match self.cvar_holder(&base, name) {
            Some(k) => Ok(k.cvars.borrow().get(name).unwrap()),
            None => Err(self.name_err(format!(
                "uninitialized class variable {} in {}",
                self.sym_name(name),
                base.name()
            ))),
        }
    }

    fn cvar_set(&self, target: &Rc<RClass>, name: Sym, v: Value) {
        let base = self.lexical_class(target);
        let holder = self.cvar_holder(&base, name).unwrap_or(base);
        holder.cvars.borrow_mut().set(name, v);
    }

    // ---- calling -------------------------------------------------------

    pub fn call_method(
        &mut self,
        recv: &Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> VmResult {
        let call = self.start_call(recv.clone(), mid, args, block)?;
        self.finish(call)
    }

    /// Resolves `recv.mid(*args)` (with `method_missing` as fallback).
    fn start_call(
        &mut self,
        recv: Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> Result<Call, Unwind> {
        match self.lookup(&recv, mid) {
            Some((m, owner)) => self.start_invoke(m, owner, recv, mid, args, block),
            None => {
                if let Some((m, owner)) = self.lookup(&recv, self.s.method_missing) {
                    let mut margs = vec![Value::Sym(mid)];
                    margs.extend(args);
                    let mm = self.s.method_missing;
                    return self.start_invoke(m, owner, recv, mm, margs, block);
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
        let desc = match recv {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Class(c) => format!(
                "{} {}",
                if c.is_module { "module" } else { "class" },
                c.name()
            ),
            v if v.identical(&self.main_obj) => "main:Object".to_string(),
            _ => format!("an instance of {}", self.class_name(recv)),
        };
        let msg = format!("undefined method '{}' for {}", self.sym_name(mid), desc);
        self.err(&self.c.no_method_error, msg)
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
        owner: Rc<RClass>,
        recv: Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> VmResult {
        let call = self.start_invoke(m, owner, recv, mid, args, block)?;
        self.finish(call)
    }

    /// Starts calling method `m`: Ruby code becomes a frame for the caller
    /// to run, anything else runs now.
    fn start_invoke(
        &mut self,
        m: Method,
        owner: Rc<RClass>,
        recv: Value,
        mid: Sym,
        args: Vec<Value>,
        block: Option<Value>,
    ) -> Result<Call, Unwind> {
        let v = match m {
            Method::Native(f) => f(self, &recv, &args, block.as_ref())?,
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
                let v = args.into_iter().next().unwrap();
                self.ivar_set(&recv, iv, v.clone())?;
                v
            }
            Method::Proc(p) => {
                let call = ProcCall {
                    self_: Some(recv),
                    target: Some(owner.clone()),
                    method: Some((owner, mid)),
                    lambda: true,
                };
                return self.start_proc(&p, args, block, call);
            }
            Method::ProcCall => return self.start_proc(&recv, args, block, ProcCall::default()),
            Method::Undefined => return Err(self.no_method(&recv, mid)),
            Method::Ruby(irep) => {
                let bound = self.bind_args(&irep, &recv, args, &block, true, None)?;
                let mut f = Frame::new(irep, bound.env, recv, owner.clone());
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
        mut args: Vec<Value>,
        block: &Option<Value>,
        strict: bool,
        upper: Option<EnvRef>,
    ) -> Result<Bound, Unwind> {
        let spec = &irep.spec;
        // keyword arguments arrive as a trailing Hash marked `kw`
        let mut kwargs: Option<RHash> = None;
        if let Some(Value::Hash(h)) = args.last()
            && self.heap[h].borrow().kw
        {
            if spec.takes_kw() || spec.nokw {
                let mut kh = self.heap[h].borrow().clone();
                kh.kw = false;
                kwargs = Some(kh);
                args.pop();
            } else if self.heap[h].borrow().pairs.is_empty() {
                args.pop(); // `**{}` passes nothing
            } else {
                self.heap[h].borrow_mut().kw = false; // a plain positional Hash from here on
            }
        }
        // a proc taking several parameters splats a single Array argument
        if !strict && args.len() == 1 {
            let positional = spec.req + spec.opt + spec.post;
            if (positional > 1 || spec.auto_splat || (spec.rest && positional > 0))
                && let Value::Array(a) = &args[0]
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
                args.resize(mand, Value::Nil);
            }
            if !spec.rest && args.len() > mand + spec.opt {
                args.truncate(mand + spec.opt);
            }
        }
        let n = args.len();
        let opt_filled = (n - mand).min(spec.opt);
        let rest_len = n - mand - opt_filled;
        let mut regs = vec![Value::Nil; irep.nregs.max(1 + spec.nparams())];
        regs[0] = self_.clone();
        let mut r = 1;
        let mut it = args.into_iter();
        for _ in 0..spec.req {
            regs[r] = it.next().unwrap();
            r += 1;
        }
        for i in 0..spec.opt {
            if i < opt_filled {
                regs[r] = it.next().unwrap();
            }
            r += 1;
        }
        if spec.rest {
            regs[r] = self.array(it.by_ref().take(rest_len).collect());
            r += 1;
        }
        for _ in 0..spec.post {
            regs[r] = it.next().unwrap();
            r += 1;
        }
        let mut kw_mask = 0u64;
        if spec.nokw && kwargs.as_ref().is_some_and(|h| !h.pairs.is_empty()) {
            return Err(self.arg_err("no keywords accepted"));
        }
        let mut kwh = kwargs.unwrap_or_default();
        let mut missing = Vec::new();
        for (i, (sym, required)) in spec.kw.iter().enumerate() {
            match kwh.remove(&self.heap, &Value::Sym(*sym)) {
                Some(v) => {
                    regs[r] = v;
                    kw_mask |= 1 << i.min(63);
                }
                None if *required => missing.push(*sym),
                None => {}
            }
            r += 1;
        }
        if !missing.is_empty() {
            let s = if missing.len() > 1 { "s" } else { "" };
            return Err(self.arg_err(format!("missing keyword{s}: {}", self.sym_list(&missing))));
        }
        if spec.kwrest {
            regs[r] = if spec.nokw {
                Value::Nil
            } else {
                self.hash(kwh)
            };
            r += 1;
        } else if !kwh.pairs.is_empty() {
            let unknown: Vec<Sym> = kwh
                .pairs
                .iter()
                .filter_map(|(k, _)| {
                    if let Value::Sym(s) = k {
                        Some(*s)
                    } else {
                        None
                    }
                })
                .collect();
            let s = if unknown.len() > 1 { "s" } else { "" };
            return Err(self.arg_err(format!("unknown keyword{s}: {}", self.sym_list(&unknown))));
        }
        if spec.block {
            regs[r] = block.clone().unwrap_or(Value::Nil);
        }
        Ok(Bound {
            env: self.heap.alloc(Env {
                regs: RefCell::new(regs),
                upper,
            }),
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
        let call = self.start_proc(blk, args, block, call)?;
        self.finish(call)
    }

    fn start_proc(
        &mut self,
        blk: &Value,
        args: Vec<Value>,
        block: Option<Value>,
        call: ProcCall,
    ) -> Result<Call, Unwind> {
        let Value::Proc(p) = blk else {
            return Err(self.type_err("wrong argument type (expected Proc)"));
        };
        let p = *p;
        match self.heap[p].body.clone() {
            ProcBody::SymCall(sym) => {
                let mut it = args.into_iter();
                let recv = it.next().ok_or_else(|| self.arg_err("no receiver given"))?;
                Ok(Call::Done(self.call_method(&recv, sym, it.collect(), block)?))
            }
            ProcBody::Collect(buf) => {
                let v = if args.len() == 1 {
                    args.into_iter().next().unwrap()
                } else {
                    self.array(args)
                };
                self.heap[buf].borrow_mut().push(v);
                Ok(Call::Done(Value::Nil))
            }
            ProcBody::Ruby { irep, env } => {
                let rp = &self.heap[p];
                let lambda = rp.lambda || call.lambda;
                let self_ = call.self_.unwrap_or_else(|| rp.self_.clone());
                let target = call.target.unwrap_or_else(|| rp.target.clone());
                let method = call.method.or_else(|| rp.method.clone());
                let (outer_block, home) = (rp.block.clone(), rp.home);
                let bound = self.bind_args(&irep, &self_, args, &block, lambda, Some(env))?;
                let mut f = Frame::new(irep, bound.env, self_, target);
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
        let target = self.c.object.clone();
        Value::Proc(self.heap.alloc(RProc {
            body,
            self_: Value::Nil,
            block: None,
            home: 0,
            method: None,
            target,
            lambda: false,
        }))
    }

    /// The same proc with lambda semantics (`lambda { }`).
    pub fn to_lambda(&mut self, v: &Value) -> Value {
        match v {
            Value::Proc(p) if !self.heap[p].lambda => {
                let p = &self.heap[p];
                let copy = RProc {
                    body: p.body.clone(),
                    self_: p.self_.clone(),
                    block: p.block.clone(),
                    home: p.home,
                    method: p.method.clone(),
                    target: p.target.clone(),
                    lambda: true,
                };
                Value::Proc(self.heap.alloc(copy))
            }
            v => v.clone(),
        }
    }

    fn block_from(&mut self, v: Value) -> Result<Option<Value>, Unwind> {
        match v {
            Value::Nil => Ok(None),
            Value::Proc(_) => Ok(Some(v)),
            Value::Sym(s) => Ok(Some(self.proc_value(ProcBody::SymCall(s)))),
            _ if self.respond_to(&v, self.s.to_proc) => {
                let p = self.call_method(&v, self.s.to_proc, vec![], None)?;
                match p {
                    Value::Proc(_) => Ok(Some(p)),
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
        Ok(match v {
            Value::Array(a) => self.heap[a].borrow().clone(),
            Value::Nil => vec![],
            Value::Hash(h) => {
                let pairs = self.heap[h].borrow().pairs.clone();
                pairs.into_iter().map(|(k, v)| self.array(vec![k, v])).collect()
            }
            Value::Range(_) => crate::core::to_vec(self, v)?,
            _ if self.respond_to(v, self.s.to_a) => {
                match self.call_method(v, self.s.to_a, vec![], None)? {
                    Value::Array(a) => self.heap[a].borrow().clone(),
                    Value::Nil => vec![v.clone()],
                    _ => return Err(self.type_err("can't convert to Array (to_a gives non-Array)")),
                }
            }
            _ => vec![v.clone()],
        })
    }

    /// Converts the right-hand side of a multiple assignment to an Array.
    fn masgn_array(&mut self, v: &Value) -> Result<ArrayRef, Unwind> {
        let wrap = |vm: &mut Self| vm.heap.alloc(RefCell::new(vec![v.clone()]));
        match v {
            Value::Array(a) => Ok(*a),
            _ if !matches!(v, Value::Nil) && self.respond_to(v, self.s.to_ary) => {
                match self.call_method(v, self.s.to_ary, vec![], None)? {
                    Value::Array(a) => Ok(a),
                    Value::Nil => Ok(wrap(self)),
                    _ => Err(self.type_err("can't convert to Array (to_ary gives non-Array)")),
                }
            }
            _ => Ok(wrap(self)),
        }
    }

    pub fn ivar_get(&self, obj: &Value, iv: Sym) -> Value {
        self.ivar_lookup(obj, iv).unwrap_or(Value::Nil)
    }

    pub fn ivar_lookup(&self, obj: &Value, iv: Sym) -> Option<Value> {
        match obj {
            Value::Object(o) => self.heap[o].ivars.borrow().get(iv),
            Value::Class(c) => c.ivars.borrow().get(iv),
            _ => None,
        }
    }

    pub fn ivar_set(&self, obj: &Value, iv: Sym, v: Value) -> Result<(), Unwind> {
        match obj {
            Value::Object(o) => self.heap[o].ivars.borrow_mut().set(iv, v),
            Value::Class(c) => c.ivars.borrow_mut().set(iv, v),
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
        let main = self.main_obj.clone();
        let mut f = self.body_frame(irep, main, self.c.object.clone());
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
                let c = self.c.local_jump_error.clone();
                Err(self.make_exception(&c, "unexpected return"))
            }
            Err(Unwind::Break { .. }) => {
                let c = self.c.local_jump_error.clone();
                Err(self.make_exception(&c, "break from proc-closure"))
            }
        }
    }

    /// A frame for the script or a class body (`class X`, `class << obj`).
    fn body_frame(&mut self, irep: Rc<Irep>, self_: Value, target: Rc<RClass>) -> Frame {
        let mut regs = vec![Value::Nil; irep.nregs.max(1)];
        regs[0] = self_.clone();
        let env = self.heap.alloc(Env {
            regs: RefCell::new(regs),
            upper: None,
        });
        let mut f = Frame::new(irep, env, self_, target);
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
                Ok(Flow::Call(_)) => Err(self.err(&self.c.stack_error, "stack level too deep")),
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
    fn collect_garbage(&mut self, current: &Frame) {
        let mut m = Marker::new(&self.heap);
        for f in self.frames.iter().chain(std::iter::once(current)) {
            mark_frame(&mut m, f);
        }
        m.value(&self.main_obj);
        for v in self.globals.values().chain(&self.at_exit) {
            m.value(v);
        }
        // builtin classes are constants of Object, or reachable from those
        m.class(&self.c.basic_object);
        m.class(&self.c.object);
        let marks = m.finish();
        self.heap.sweep(marks);
    }

    /// Looks for a handler in `f` covering instruction `at`; `None` when one
    /// took the unwind (and `f` continues there). Exceptions go to any
    /// handler, `break`/`return` passing through only run `ensure` clauses.
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
        match v {
            Value::Array(a) => self.heap[a].borrow().clone(),
            other => vec![other],
        }
    }

    /// Executes one instruction.
    fn step(&mut self, f: &mut Frame, op: Op) -> Result<Flow, Unwind> {
        match op {
            Op::Nop => {}
            Op::Move(a, b) => self.set_reg(f, a, self.reg(f, b)),
            Op::LoadI(a, i) => self.set_reg(f, a, Value::Int(i)),
            Op::LoadL(a, i) => {
                let v = match &f.irep.pool[i] {
                    Literal::Float(x) => Value::Float(*x),
                    Literal::Str(s) => self.str(s.to_string()),
                };
                self.set_reg(f, a, v);
            }
            Op::LoadSym(a, s) => self.set_reg(f, a, Value::Sym(s)),
            Op::LoadNil(a) => self.set_reg(f, a, Value::Nil),
            Op::LoadSelf(a) => self.set_reg(f, a, f.self_.clone()),
            Op::LoadTrue(a) => self.set_reg(f, a, Value::Bool(true)),
            Op::LoadFalse(a) => self.set_reg(f, a, Value::Bool(false)),
            Op::GetGV(a, s) => self.set_reg(f, a, self.globals.get(&s).cloned().unwrap_or(Value::Nil)),
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
                let Value::Class(outer) = self.reg(f, a) else {
                    let v = self.reg(f, a);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                match self.get_const_in(&outer, s) {
                    Some(v) => self.set_reg(f, a, v),
                    None => {
                        let msg = format!(
                            "uninitialized constant {}::{}",
                            outer.name(),
                            self.sym_name(s)
                        );
                        return Err(self.name_err(msg));
                    }
                }
            }
            Op::SetMCnst(a, s) => {
                let Value::Class(outer) = self.reg(f, a + 1) else {
                    let v = self.reg(f, a + 1);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                self.store_const(&outer, s, self.reg(f, a));
            }
            Op::OClass(a) => self.set_reg(f, a, Value::Class(self.c.object.clone())),
            Op::GetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                self.set_reg(f, a, self.heap[env].regs.borrow()[idx].clone());
            }
            Op::SetUpVar(a, idx, depth) => {
                let env = self.upvar_env(f, depth);
                self.heap[env].regs.borrow_mut()[idx] = self.reg(f, a);
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
                let exc = f.exc.take().unwrap_or(Value::Nil);
                self.set_reg(f, a, exc);
            }
            Op::Rescue(a, b) => {
                let exc = self.reg(f, a);
                let classes = match self.reg(f, b) {
                    Value::Array(list) => self.heap[list].borrow().clone(),
                    v => vec![v],
                };
                let mut hit = false;
                for c in classes {
                    let Value::Class(cls) = c else {
                        return Err(self.type_err("class or module required for rescue clause"));
                    };
                    if self.is_a(&exc, &cls) {
                        hit = true;
                        break;
                    }
                }
                self.set_reg(f, b, Value::Bool(hit));
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
            Op::Send(a, mid, argc, has_block) => {
                let recv = self.reg(f, a);
                let args = (0..argc).map(|i| self.reg(f, a + 1 + i)).collect();
                let blk = if has_block {
                    self.block_from(self.reg(f, a + argc + 1))?
                } else {
                    None
                };
                let tag = proc_tag(&blk);
                return self.call_flow(f, a, tag, |vm| vm.start_call(recv, mid, args, blk));
            }
            Op::SendV(a, mid, has_block) => {
                let recv = self.reg(f, a);
                let args = self.args_from_array(self.reg(f, a + 1));
                let blk = if has_block {
                    self.block_from(self.reg(f, a + 2))?
                } else {
                    None
                };
                let tag = proc_tag(&blk);
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
                Some(b) => self.set_reg(f, a, b.clone()),
                None if required => {
                    return Err(self.err(&self.c.local_jump_error, "no block given (yield)"));
                }
                None => self.set_reg(f, a, Value::Nil),
            },
            Op::BlockGiven(a) => self.set_reg(f, a, Value::Bool(f.block.is_some())),
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
            Op::Add(a) => return self.arith_flow(f, a, Arith::Add, self.s.add),
            Op::Sub(a) => return self.arith_flow(f, a, Arith::Sub, self.s.sub),
            Op::Mul(a) => return self.arith_flow(f, a, Arith::Mul, self.s.mul),
            Op::Div(a) => return self.arith_flow(f, a, Arith::Div, self.s.div),
            Op::Eq(a) => {
                let (x, y) = (self.reg(f, a), self.reg(f, a + 1));
                let v = self.equal(&x, &y)?;
                self.set_reg(f, a, Value::Bool(v));
            }
            Op::Lt(a) => return self.cmp_flow(f, a, self.s.lt, Ordering::is_lt),
            Op::Le(a) => return self.cmp_flow(f, a, self.s.le, Ordering::is_le),
            Op::Gt(a) => return self.cmp_flow(f, a, self.s.gt, Ordering::is_gt),
            Op::Ge(a) => return self.cmp_flow(f, a, self.s.ge, Ordering::is_ge),
            Op::Not(a) => self.set_reg(f, a, Value::Bool(!self.reg(f, a).truthy())),
            Op::Array(a, b, n) => {
                let v = (0..n).map(|i| self.reg(f, b + i)).collect();
                let v = self.array(v);
                self.set_reg(f, a, v);
            }
            Op::ArrayCat(a, b) => {
                let items = self.splat_to_vec(&self.reg(f, b))?;
                if let Value::Array(arr) = self.reg(f, a) {
                    self.heap[arr].borrow_mut().extend(items);
                }
            }
            Op::ArrayPush(a, b) => {
                if let Value::Array(arr) = self.reg(f, a) {
                    let v = self.reg(f, b);
                    self.heap[arr].borrow_mut().push(v);
                }
            }
            Op::ToAry(a, b) => {
                let v = self.reg(f, b);
                let arr = self.masgn_array(&v)?;
                self.set_reg(f, a, Value::Array(arr));
            }
            Op::ARef(a, b, i) => {
                let v = match self.reg(f, b) {
                    Value::Array(arr) => self.heap[arr].borrow().get(i).cloned().unwrap_or(Value::Nil),
                    other if i == 0 => other,
                    _ => Value::Nil,
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
                self.set_reg(f, a, items.get(idx).cloned().unwrap_or(Value::Nil));
            }
            Op::Hash(a, b, n) => {
                let Value::Hash(h) = self.hash(RHash::new()) else {
                    unreachable!()
                };
                for i in 0..n {
                    let (k, v) = (self.reg(f, b + 2 * i), self.reg(f, b + 2 * i + 1));
                    self.hash_set(h, k, v);
                }
                self.set_reg(f, a, Value::Hash(h));
            }
            Op::HashAdd(a, b) => {
                if let Value::Hash(h) = self.reg(f, a) {
                    let (k, v) = (self.reg(f, b), self.reg(f, b + 1));
                    self.hash_set(h, k, v);
                }
            }
            Op::HashCat(a, b) => {
                let pairs = match self.reg(f, b) {
                    Value::Hash(other) => self.heap[other].borrow().pairs.clone(),
                    Value::Nil => vec![],
                    v => {
                        let what = self.class_name(&v);
                        return Err(
                            self.type_err(format!("no implicit conversion of {what} into Hash"))
                        );
                    }
                };
                if let Value::Hash(h) = self.reg(f, a) {
                    for (k, v) in pairs {
                        self.hash_set(h, k, v);
                    }
                }
            }
            Op::KwHash(a) => {
                if let Value::Hash(h) = self.reg(f, a) {
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
                let piece = match self.reg(f, a + 1) {
                    Value::Str(s) => self.heap[s].borrow().clone(),
                    other => self.inspect(&other)?,
                };
                if let Value::Str(s) = self.reg(f, a) {
                    self.heap[s].borrow_mut().push_str(&piece);
                }
            }
            Op::Intern(a) => {
                let v = match self.reg(f, a) {
                    Value::Str(s) => {
                        let s = self.heap[s].borrow().clone();
                        Value::Sym(self.intern(&s))
                    }
                    v => v,
                };
                self.set_reg(f, a, v);
            }
            Op::Range(a, excl) => {
                let (lo, hi) = (self.reg(f, a), self.reg(f, a + 1));
                let v = self.range(lo, hi, excl);
                self.set_reg(f, a, v);
            }
            Op::Block(a, idx) | Op::Lambda(a, idx) => {
                let p = RProc {
                    body: ProcBody::Ruby {
                        irep: f.irep.reps[idx].clone(),
                        env: f.env,
                    },
                    self_: f.self_.clone(),
                    block: f.block.clone(),
                    home: f.home,
                    method: f.method.clone(),
                    target: f.target.clone(),
                    lambda: matches!(op, Op::Lambda(..)),
                };
                let p = Value::Proc(self.heap.alloc(p));
                self.set_reg(f, a, p);
            }
            Op::TDef(s, idx) => {
                let irep = f.irep.reps[idx].clone();
                f.target.methods.borrow_mut().insert(s, Method::Ruby(irep));
            }
            Op::SDef(a, s, idx) => {
                let cls = self.singleton_class(&self.reg(f, a))?;
                cls.methods
                    .borrow_mut()
                    .insert(s, Method::Ruby(f.irep.reps[idx].clone()));
            }
            Op::Class(a, name, is_module) => {
                let cls = self.define_class(f, a, name, is_module)?;
                self.set_reg(f, a, Value::Class(cls));
            }
            Op::SClass(a) => {
                let cls = self.singleton_class(&self.reg(f, a))?;
                self.set_reg(f, a, Value::Class(cls));
            }
            Op::Exec(a, idx) => {
                let Value::Class(cls) = self.reg(f, a) else {
                    unreachable!()
                };
                let target = cls.clone();
                let mut body = self.body_frame(f.irep.reps[idx].clone(), Value::Class(cls), target);
                body.ret = a;
                return Ok(Flow::Call(body));
            }
            Op::Alias(new, old) => self.alias_method(&f.target, new, old)?,
            Op::GAlias(new, old) => {
                let v = self.globals.get(&old).cloned().unwrap_or(Value::Nil);
                self.globals.insert(new, v);
            }
            Op::Undef(s) => self.undef_method(&f.target, s)?,
            Op::Defined(a, kind, s) => {
                let v = match self.defined(f, kind, s) {
                    Some(d) => self.str(d),
                    None => Value::Nil,
                };
                self.set_reg(f, a, v);
            }
            Op::CurMethod(a) => {
                let v = f
                    .method
                    .as_ref()
                    .map(|(_, m)| Value::Sym(*m))
                    .unwrap_or(Value::Nil);
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
        let Some((owner, mid)) = f.method.clone() else {
            return Err(self.err(&self.c.no_method_error, "super called outside of method"));
        };
        let self_ = f.self_.clone();
        let tag = proc_tag(&blk);
        if let Some((m, o)) = self.find_super(&self_, &owner, mid) {
            return self.call_flow(f, a, tag, |vm| vm.start_invoke(m, o, self_, mid, args, blk));
        }
        if let Some((mm, mo)) = self.lookup(&self_, self.s.method_missing) {
            let mut margs = vec![Value::Sym(mid)];
            margs.extend(args);
            let name = self.s.method_missing;
            return self.call_flow(f, a, tag, |vm| vm.start_invoke(mm, mo, self_, name, margs, blk));
        }
        let msg = format!("super: no superclass method '{}'", self.sym_name(mid));
        Err(self.err(&self.c.no_method_error, msg))
    }

    pub fn alias_method(&self, target: &Rc<RClass>, new: Sym, old: Sym) -> Result<(), Unwind> {
        match Self::find_method(target, old) {
            Some((m, _)) => {
                target.methods.borrow_mut().insert(new, m);
                Ok(())
            }
            None => Err(self.name_err(format!(
                "undefined method '{}' for class '{}'",
                self.sym_name(old),
                target.name()
            ))),
        }
    }

    pub fn undef_method(&self, target: &Rc<RClass>, s: Sym) -> Result<(), Unwind> {
        if Self::find_method(target, s).is_none() {
            return Err(self.name_err(format!(
                "undefined method '{}' for class '{}'",
                self.sym_name(s),
                target.name()
            )));
        }
        target.methods.borrow_mut().insert(s, Method::Undefined);
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
            DefinedKind::ScopedConst(r) => match self.reg(f, r) {
                Value::Class(c) => self.get_const_in(&c, s).is_some(),
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
    ) -> Result<Rc<RClass>, Unwind> {
        let outer = match self.reg(f, a) {
            Value::Class(c) => c,
            Value::Nil => self.lexical_class(&f.target),
            v => {
                let what = self.inspect(&v)?;
                return Err(self.type_err(format!("{what} is not a class/module")));
            }
        };
        let name_str = self.sym_name(name).to_string();
        let existing = outer.consts.borrow().get(&name).cloned();
        match existing {
            Some(Value::Class(c)) if c.is_module == is_module => {
                if let Value::Class(sup) = self.reg(f, a + 1) {
                    let same = c.superclass.as_ref().is_some_and(|s| Rc::ptr_eq(s, &sup));
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
                    match self.reg(f, a + 1) {
                        Value::Nil => Some(self.c.object.clone()),
                        Value::Class(s) if !s.is_module && !s.is_singleton => Some(s),
                        _ => return Err(self.type_err("superclass must be a Class")),
                    }
                };
                let full = if Rc::ptr_eq(&outer, &self.c.object) {
                    name_str
                } else {
                    format!("{}::{}", outer.name(), name_str)
                };
                let cls = RClass::new(&full, superclass, is_module);
                if !Rc::ptr_eq(&outer, &self.c.object) {
                    *cls.outer.borrow_mut() = Some(Rc::downgrade(&outer));
                }
                outer
                    .consts
                    .borrow_mut()
                    .insert(name, Value::Class(cls.clone()));
                Ok(cls)
            }
        }
    }

    // ---- arithmetic & comparison (src/numeric.c, src/numops.c) ---------

    pub fn arith(&self, op: Arith, x: &Value, y: &Value) -> VmResult {
        use Value::{Float, Int};
        Ok(match (x, y) {
            (Int(a), Int(b)) => return self.int_arith(op, *a, *b),
            (Int(a), Float(b)) => Float(float_arith(op, *a as f64, *b)),
            (Float(a), Int(b)) => Float(float_arith(op, *a, *b as f64)),
            (Float(a), Float(b)) => Float(float_arith(op, *a, *b)),
            _ => {
                let what = match y {
                    Value::Nil => "nil".to_string(),
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

    fn int_arith(&self, op: Arith, a: i64, b: i64) -> VmResult {
        // without Bignum (mruby-bigint), overflow falls back to Float
        let overflow = || Value::Float(float_arith(op, a as f64, b as f64));
        let zero_div = || self.err(&self.c.zero_division_error, "divided by 0");
        Ok(match op {
            Arith::Add => a.checked_add(b).map(Value::Int).unwrap_or_else(overflow),
            Arith::Sub => a.checked_sub(b).map(Value::Int).unwrap_or_else(overflow),
            Arith::Mul => a.checked_mul(b).map(Value::Int).unwrap_or_else(overflow),
            Arith::Div => {
                if b == 0 {
                    return Err(zero_div());
                }
                match a.checked_div(b) {
                    Some(q) if a % b != 0 && ((a < 0) != (b < 0)) => Value::Int(q - 1),
                    Some(q) => Value::Int(q),
                    None => overflow(),
                }
            }
            Arith::Mod => {
                if b == 0 {
                    return Err(zero_div());
                }
                let r = a.wrapping_rem(b);
                Value::Int(if r != 0 && ((r < 0) != (b < 0)) {
                    r + b
                } else {
                    r
                })
            }
            Arith::Pow => match u32::try_from(b) {
                Ok(e) => a.checked_pow(e).map(Value::Int).unwrap_or_else(overflow),
                Err(_) if b < 0 => Value::Float((a as f64).powf(b as f64)),
                Err(_) => overflow(),
            },
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
        self.call_flow(f, a, 0, |vm| vm.start_call(x, mid, vec![y], None))
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
            self.set_reg(f, a, Value::Bool(num_cmp(&x, &y).is_some_and(pred)));
            return Ok(Flow::Next);
        }
        self.call_flow(f, a, 0, |vm| vm.start_call(x, mid, vec![y], None))
    }

    /// `a == b`, dispatching to user-defined `==` for objects.
    pub fn equal(&mut self, a: &Value, b: &Value) -> Result<bool, Unwind> {
        if let Value::Object(_) = a {
            return Ok(self
                .call_method(a, self.s.eq, vec![b.clone()], None)?
                .truthy());
        }
        Ok(values_eq(&self.heap, a, b))
    }

    // ---- conversions (src/print.c, object.c) ---------------------------

    pub fn to_s(&mut self, v: &Value) -> Result<String, Unwind> {
        Ok(match v {
            Value::Str(s) => self.heap[s].borrow().clone(),
            Value::Nil => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(x) => fmt_float(*x),
            Value::Sym(s) => self.sym_name(*s).to_string(),
            _ => match self.call_method(v, self.s.to_s, vec![], None)? {
                Value::Str(s) => self.heap[s].borrow().clone(),
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
        let pairs = self.heap[h].borrow().pairs.clone();
        if pairs.is_empty() {
            return Ok("{}".into());
        }
        let mut parts = Vec::with_capacity(pairs.len());
        for (k, val) in &pairs {
            let vs = self.inspect(val)?;
            parts.push(match k {
                Value::Sym(s) if is_simple_sym(self.sym_name(*s)) => {
                    format!("{}: {}", self.sym_name(*s), vs)
                }
                Value::Sym(s) => format!("{}: {}", inspect_str(self.sym_name(*s)), vs),
                _ => format!("{} => {}", self.inspect(k)?, vs),
            });
        }
        Ok(format!("{{{}}}", parts.join(", ")))
    }

    pub fn inspect(&mut self, v: &Value) -> Result<String, Unwind> {
        Ok(match v {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(x) => fmt_float(*x),
            Value::Sym(s) => inspect_sym(self.sym_name(*s)),
            Value::Str(s) => inspect_str(&self.heap[s].borrow()),
            Value::Array(a) => {
                let id = a.id();
                if self.inspecting.contains(&id) {
                    return Ok("[...]".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_array(*a);
                self.inspecting.pop();
                r?
            }
            Value::Hash(h) => {
                let id = h.id();
                if self.inspecting.contains(&id) {
                    return Ok("{...}".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_hash(*h);
                self.inspecting.pop();
                r?
            }
            Value::Range(r) => {
                let (lo, hi, excl) = self.heap[r].clone();
                let lo = if lo.is_nil() { String::new() } else { self.inspect(&lo)? };
                let hi = if hi.is_nil() { String::new() } else { self.inspect(&hi)? };
                format!("{lo}{}{hi}", if excl { "..." } else { ".." })
            }
            Value::Class(_) | Value::Proc(_) | Value::Object(_) => {
                match self.call_method(v, self.s.inspect, vec![], None)? {
                    Value::Str(s) => self.heap[s].borrow().clone(),
                    _ => format!("#<{}>", self.class_name(v)),
                }
            }
        })
    }
}

fn mark_frame(m: &mut Marker, f: &Frame) {
    m.env(f.env);
    m.value(&f.self_);
    for v in f.block.iter().chain(&f.exc) {
        m.value(v);
    }
    m.class(&f.target);
    if let Some((owner, _)) = &f.method {
        m.class(owner);
    }
    match &f.pending {
        Some(Unwind::Error(v) | Unwind::Break { value: v, .. } | Unwind::Return { value: v, .. }) => {
            m.value(v)
        }
        Some(Unwind::Raise(c, _)) => m.class(c),
        None => {}
    }
}

fn proc_tag(blk: &Option<Value>) -> u64 {
    match blk {
        Some(Value::Proc(p)) => p.id(),
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
