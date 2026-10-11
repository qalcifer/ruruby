//! The virtual machine: `src/vm.c`, `src/state.c`, and the dispatch parts of
//! `src/class.c` / `src/error.c` in mruby.
//!
//! Differences from mruby worth knowing:
//! * mruby runs Ruby→Ruby calls inside one `mrb_vm_exec` loop with an
//!   explicit call-info stack (`mrb_callinfo`), which is what makes Fibers
//!   possible. Here each Ruby call recurses into `execute`. Moving to an
//!   explicit frame stack is a roadmap item.
//! * mruby unwinds with `setjmp/longjmp`; here unwinding is `Result<_, Unwind>`
//!   propagated with `?`, and each frame consults its irep's catch table.

use crate::opcode::{ArgSpec, DefinedKind, Irep, Literal, Op, Reg};
use crate::symbol::{Sym, SymbolTable};
use crate::value::*;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

/// Non-local control flow.
pub enum Unwind {
    /// A raised exception (an exception object).
    Error(Value),
    /// `break` out of a block; caught by the call site that passed the block (`tag`).
    Break { tag: usize, value: Value },
    /// `return` from inside a block; caught by the method frame `home`.
    Return { home: usize, value: Value },
}

pub type VmResult = Result<Value, Unwind>;

const MAX_DEPTH: usize = 10_000;

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
    pub syms: SymbolTable,
    pub c: Classes,
    pub s: CommonSyms,
    pub globals: HashMap<Sym, Value>,
    pub main_obj: Value,
    /// When set, output of `puts`/`print`/`p` goes here instead of stdout.
    pub capture: Option<String>,
    /// Blocks registered with `at_exit` / `END { }`.
    pub at_exit: Vec<Value>,
    next_id: usize,
    depth: usize,
    /// Arrays/hashes being inspected right now, to print recursion as `[...]`.
    inspecting: Vec<usize>,
    /// Blocks whose receiving call is still running: `break` targets.
    live_breaks: Vec<usize>,
    /// Method (and lambda) frames still running: `return` targets of their blocks.
    live_homes: Vec<usize>,
}

struct Frame {
    irep: Rc<Irep>,
    env: Rc<Env>,
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
    proc_tag: usize,
    exc: Option<Value>,
    /// A `break`/`return` paused while an `ensure` clause runs; `Raise` of nil resumes it.
    pending: Option<Unwind>,
}

impl Frame {
    fn get(&self, i: Reg) -> Value {
        self.env.regs.borrow()[i].clone()
    }

    fn set(&self, i: Reg, v: Value) {
        self.env.regs.borrow_mut()[i] = v;
    }
}

/// Bound arguments: the frame's registers plus what the prologue needs.
struct Bound {
    env: Rc<Env>,
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
        let main_obj = Value::Object(Rc::new(RObject::new(c.object.clone())));
        let mut vm = Vm {
            syms,
            c,
            s,
            globals: HashMap::new(),
            main_obj,
            capture: None,
            at_exit: Vec::new(),
            next_id: 1,
            depth: 0,
            inspecting: Vec::new(),
            live_breaks: Vec::new(),
            live_homes: Vec::new(),
        };
        crate::core::init(&mut vm);
        vm
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
        self.depth = 0;
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

    pub fn make_exception(&self, cls: &Rc<RClass>, msg: impl Into<String>) -> Value {
        let obj = RObject::new(cls.clone());
        obj.ivars.borrow_mut().set(self.s.message, Value::str(msg));
        Value::Object(Rc::new(obj))
    }

    pub fn err(&self, cls: &Rc<RClass>, msg: impl Into<String>) -> Unwind {
        Unwind::Error(self.make_exception(cls, msg))
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
            && let Some(Value::Str(s)) = o.ivars.borrow().get(self.s.message)
        {
            return s.borrow().clone();
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
            Value::Object(o) => o.class.clone(),
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
                if let Some(s) = o.singleton.borrow().as_ref() {
                    return Ok(s.clone());
                }
                let s = RClass::new_singleton(
                    format!("#<Class:#<{}>>", o.class.name()),
                    o.class.clone(),
                );
                *o.singleton.borrow_mut() = Some(s.clone());
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
            Value::Object(o) => o
                .singleton
                .borrow()
                .clone()
                .unwrap_or_else(|| o.class.clone()),
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
        match self.lookup(recv, mid) {
            Some((m, owner)) => self.invoke(m, owner, recv.clone(), mid, args, block),
            None => {
                if let Some((m, owner)) = self.lookup(recv, self.s.method_missing) {
                    let mut margs = vec![Value::Sym(mid)];
                    margs.extend(args);
                    let mm = self.s.method_missing;
                    return self.invoke(m, owner, recv.clone(), mm, margs, block);
                }
                Err(self.no_method(recv, mid))
            }
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

    /// Runs a frame that blocks can `return` from.
    fn execute_home(&mut self, frame: Frame) -> VmResult {
        let id = frame.home;
        self.live_homes.push(id);
        let r = self.execute(frame);
        self.live_homes.pop();
        match r {
            Err(Unwind::Return { home, value }) if home == id => Ok(value),
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
        match m {
            Method::Native(f) => f(self, &recv, &args, block.as_ref()),
            Method::AttrReader(iv) => {
                if !args.is_empty() {
                    return Err(self.arity_err(args.len(), &ArgSpec::default()));
                }
                Ok(self.ivar_get(&recv, iv))
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
                Ok(v)
            }
            Method::Proc(p) => {
                let call = ProcCall {
                    self_: Some(recv),
                    target: Some(owner.clone()),
                    method: Some((owner, mid)),
                    lambda: true,
                };
                self.call_proc(&p, args, block, call)
            }
            Method::Undefined => Err(self.no_method(&recv, mid)),
            Method::Ruby(irep) => {
                let bound = self.bind_args(&irep, &recv, args, &block, true, None)?;
                let id = self.new_id();
                let frame = Frame {
                    env: bound.env,
                    irep,
                    self_: recv,
                    block,
                    target: owner.clone(),
                    method: Some((owner, mid)),
                    opt_filled: bound.opt_filled,
                    kw_mask: bound.kw_mask,
                    home: id,
                    proc_tag: 0,
                    exc: None,
                    pending: None,
                };
                self.execute_home(frame)
            }
        }
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
        &self,
        irep: &Irep,
        self_: &Value,
        mut args: Vec<Value>,
        block: &Option<Value>,
        strict: bool,
        upper: Option<Rc<Env>>,
    ) -> Result<Bound, Unwind> {
        let spec = &irep.spec;
        // keyword arguments arrive as a trailing Hash marked `kw`
        let mut kwargs: Option<RHash> = None;
        if let Some(Value::Hash(h)) = args.last()
            && h.borrow().kw
        {
            if spec.takes_kw() || spec.nokw {
                let mut kh = h.borrow().clone();
                kh.kw = false;
                kwargs = Some(kh);
                args.pop();
            } else if h.borrow().pairs.is_empty() {
                args.pop(); // `**{}` passes nothing
            } else {
                h.borrow_mut().kw = false; // a plain positional Hash from here on
            }
        }
        // a proc taking several parameters splats a single Array argument
        if !strict && args.len() == 1 {
            let positional = spec.req + spec.opt + spec.post;
            if (positional > 1 || spec.auto_splat || (spec.rest && positional > 0))
                && let Value::Array(a) = &args[0]
            {
                let items = a.borrow().clone();
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
            regs[r] = Value::array(it.by_ref().take(rest_len).collect());
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
            match kwh.remove(&Value::Sym(*sym)) {
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
                Value::hash(kwh)
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
            env: Rc::new(Env {
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
        let Value::Proc(p) = blk else {
            return Err(self.type_err("wrong argument type (expected Proc)"));
        };
        match &p.body {
            ProcBody::SymCall(sym) => {
                let mut it = args.into_iter();
                let recv = it.next().ok_or_else(|| self.arg_err("no receiver given"))?;
                self.call_method(&recv, *sym, it.collect(), block)
            }
            ProcBody::Collect(buf) => {
                let v = if args.len() == 1 {
                    args.into_iter().next().unwrap()
                } else {
                    Value::array(args)
                };
                buf.borrow_mut().push(v);
                Ok(Value::Nil)
            }
            ProcBody::Ruby { irep, env } => {
                let lambda = p.lambda || call.lambda;
                let self_ = call.self_.unwrap_or_else(|| p.self_.clone());
                let bound =
                    self.bind_args(irep, &self_, args, &block, lambda, Some(env.clone()))?;
                let id = if lambda { self.new_id() } else { p.home };
                let tag = Rc::as_ptr(p) as usize;
                let frame = Frame {
                    env: bound.env,
                    irep: irep.clone(),
                    self_,
                    block: p.block.clone(),
                    target: call.target.unwrap_or_else(|| p.target.clone()),
                    method: call.method.or_else(|| p.method.clone()),
                    opt_filled: bound.opt_filled,
                    kw_mask: bound.kw_mask,
                    home: id,
                    proc_tag: tag,
                    exc: None,
                    pending: None,
                };
                if !lambda {
                    return self.execute(frame);
                }
                self.live_breaks.push(tag);
                let r = self.execute_home(frame);
                self.live_breaks.pop();
                match r {
                    Err(Unwind::Break { tag: t, value }) if t == tag => Ok(value),
                    r => r,
                }
            }
        }
    }

    pub fn proc_value(&self, body: ProcBody) -> Value {
        Value::Proc(Rc::new(RProc {
            body,
            self_: Value::Nil,
            block: None,
            home: 0,
            method: None,
            target: self.c.object.clone(),
            lambda: false,
        }))
    }

    /// The same proc with lambda semantics (`lambda { }`).
    pub fn to_lambda(&self, v: &Value) -> Value {
        match v {
            Value::Proc(p) if !p.lambda => Value::Proc(Rc::new(RProc {
                body: p.body.clone(),
                self_: p.self_.clone(),
                block: p.block.clone(),
                home: p.home,
                method: p.method.clone(),
                target: p.target.clone(),
                lambda: true,
            })),
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
            Value::Array(a) => a.borrow().clone(),
            Value::Nil => vec![],
            Value::Hash(h) => h
                .borrow()
                .pairs
                .iter()
                .map(|(k, v)| Value::array(vec![k.clone(), v.clone()]))
                .collect(),
            Value::Range(_) => crate::core::to_vec(self, v)?,
            _ if self.respond_to(v, self.s.to_a) => {
                match self.call_method(v, self.s.to_a, vec![], None)? {
                    Value::Array(a) => a.borrow().clone(),
                    Value::Nil => vec![v.clone()],
                    _ => return Err(self.type_err("can't convert to Array (to_a gives non-Array)")),
                }
            }
            _ => vec![v.clone()],
        })
    }

    /// Converts the right-hand side of a multiple assignment to an Array.
    fn masgn_array(&mut self, v: &Value) -> Result<Rc<RefCell<Vec<Value>>>, Unwind> {
        match v {
            Value::Array(a) => Ok(a.clone()),
            _ if !matches!(v, Value::Nil) && self.respond_to(v, self.s.to_ary) => {
                match self.call_method(v, self.s.to_ary, vec![], None)? {
                    Value::Array(a) => Ok(a),
                    Value::Nil => Ok(Rc::new(RefCell::new(vec![v.clone()]))),
                    _ => Err(self.type_err("can't convert to Array (to_ary gives non-Array)")),
                }
            }
            _ => Ok(Rc::new(RefCell::new(vec![v.clone()]))),
        }
    }

    pub fn ivar_get(&self, obj: &Value, iv: Sym) -> Value {
        self.ivar_lookup(obj, iv).unwrap_or(Value::Nil)
    }

    pub fn ivar_lookup(&self, obj: &Value, iv: Sym) -> Option<Value> {
        match obj {
            Value::Object(o) => o.ivars.borrow().get(iv),
            Value::Class(c) => c.ivars.borrow().get(iv),
            _ => None,
        }
    }

    pub fn ivar_set(&self, obj: &Value, iv: Sym, v: Value) -> Result<(), Unwind> {
        match obj {
            Value::Object(o) => o.ivars.borrow_mut().set(iv, v),
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
        let id = self.new_id();
        let main = self.main_obj.clone();
        let mut regs = vec![Value::Nil; irep.nregs.max(1)];
        regs[0] = main.clone();
        let frame = Frame {
            env: Rc::new(Env {
                regs: RefCell::new(regs),
                upper: None,
            }),
            irep,
            self_: main,
            block: None,
            target: self.c.object.clone(),
            method: None,
            opt_filled: 0,
            kw_mask: 0,
            home: id,
            proc_tag: 0,
            exc: None,
            pending: None,
        };
        match self.execute_home(frame) {
            Ok(v) => Ok(v),
            Err(Unwind::Error(e)) => Err(e),
            Err(Unwind::Return { .. }) => {
                Err(self.make_exception(&self.c.local_jump_error, "unexpected return"))
            }
            Err(Unwind::Break { .. }) => {
                Err(self.make_exception(&self.c.local_jump_error, "break from proc-closure"))
            }
        }
    }

    /// Runs a class body (or `class << obj` body) with self = the class.
    fn exec_class_body(&mut self, cls: Rc<RClass>, irep: Rc<Irep>) -> VmResult {
        let id = self.new_id();
        let self_ = Value::Class(cls.clone());
        let mut regs = vec![Value::Nil; irep.nregs.max(1)];
        regs[0] = self_.clone();
        let frame = Frame {
            env: Rc::new(Env {
                regs: RefCell::new(regs),
                upper: None,
            }),
            irep,
            self_,
            block: None,
            target: cls,
            method: None,
            opt_filled: 0,
            kw_mask: 0,
            home: id,
            proc_tag: 0,
            exc: None,
            pending: None,
        };
        self.execute(frame)
    }

    fn execute(&mut self, mut f: Frame) -> VmResult {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(&self.c.stack_error, "stack level too deep"));
        }
        self.depth += 1;
        let r = self.run(&mut f);
        self.depth -= 1;
        r
    }

    fn run(&mut self, f: &mut Frame) -> VmResult {
        let mut pc = 0;
        loop {
            let cur = pc;
            let op = f.irep.code[pc];
            pc += 1;
            match self.step(f, op, &mut pc) {
                Ok(None) => {}
                Ok(Some(v)) => return Ok(v),
                Err(Unwind::Error(exc)) => {
                    // look for a rescue/ensure handler covering this pc
                    let handler = f
                        .irep
                        .catch
                        .iter()
                        .find(|h| h.start <= cur && cur < h.end)
                        .map(|h| h.target);
                    match handler {
                        Some(target) => {
                            f.exc = Some(exc);
                            pc = target;
                        }
                        None => return Err(Unwind::Error(exc)),
                    }
                }
                // a break/return of this frame's own already ran its ensure clauses (codegen inlines them)
                Err(e) if matches!(op, Op::Break(_) | Op::ReturnBlk(_)) => return Err(e),
                Err(e) => {
                    // break/return from a callee passing through: run the ensure clauses on the way
                    let handler = f
                        .irep
                        .catch
                        .iter()
                        .find(|h| h.ensure && h.start <= cur && cur < h.end)
                        .map(|h| h.target);
                    match handler {
                        Some(target) => {
                            f.pending = Some(e);
                            f.exc = None;
                            pc = target;
                        }
                        None => return Err(e),
                    }
                }
            }
        }
    }

    fn args_from_array(&self, v: Value) -> Vec<Value> {
        match v {
            Value::Array(a) => a.borrow().clone(),
            other => vec![other],
        }
    }

    /// Executes one instruction. `Ok(Some(v))` means the frame returns `v`.
    fn step(&mut self, f: &mut Frame, op: Op, pc: &mut usize) -> Result<Option<Value>, Unwind> {
        match op {
            Op::Nop => {}
            Op::Move(a, b) => f.set(a, f.get(b)),
            Op::LoadI(a, i) => f.set(a, Value::Int(i)),
            Op::LoadL(a, i) => {
                let v = match &f.irep.pool[i] {
                    Literal::Float(x) => Value::Float(*x),
                    Literal::Str(s) => Value::str(s.to_string()),
                };
                f.set(a, v);
            }
            Op::LoadSym(a, s) => f.set(a, Value::Sym(s)),
            Op::LoadNil(a) => f.set(a, Value::Nil),
            Op::LoadSelf(a) => f.set(a, f.self_.clone()),
            Op::LoadTrue(a) => f.set(a, Value::Bool(true)),
            Op::LoadFalse(a) => f.set(a, Value::Bool(false)),
            Op::GetGV(a, s) => f.set(a, self.globals.get(&s).cloned().unwrap_or(Value::Nil)),
            Op::SetGV(a, s) => _ = self.globals.insert(s, f.get(a)),
            Op::GetIV(a, s) => f.set(a, self.ivar_get(&f.self_, s)),
            Op::SetIV(a, s) => self.ivar_set(&f.self_, s, f.get(a))?,
            Op::GetCV(a, s) => {
                let v = self.cvar_get(&f.target, s)?;
                f.set(a, v);
            }
            Op::SetCV(a, s) => self.cvar_set(&f.target, s, f.get(a)),
            Op::GetConst(a, s) => match self.get_const(&f.target, s) {
                Some(v) => f.set(a, v),
                None => {
                    return Err(
                        self.name_err(format!("uninitialized constant {}", self.sym_name(s)))
                    );
                }
            },
            Op::SetConst(a, s) => {
                let owner = self.lexical_class(&f.target);
                self.store_const(&owner, s, f.get(a));
            }
            Op::GetMCnst(a, s) => {
                let Value::Class(outer) = f.get(a) else {
                    let v = f.get(a);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                match self.get_const_in(&outer, s) {
                    Some(v) => f.set(a, v),
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
                let Value::Class(outer) = f.get(a + 1) else {
                    let v = f.get(a + 1);
                    let what = self.inspect(&v)?;
                    return Err(self.type_err(format!("{what} is not a class/module")));
                };
                self.store_const(&outer, s, f.get(a));
            }
            Op::OClass(a) => f.set(a, Value::Class(self.c.object.clone())),
            Op::GetUpVar(a, idx, depth) => {
                let env = upvar_env(f, depth);
                f.set(a, env.regs.borrow()[idx].clone());
            }
            Op::SetUpVar(a, idx, depth) => {
                let env = upvar_env(f, depth);
                env.regs.borrow_mut()[idx] = f.get(a);
            }
            Op::Jmp(t) => *pc = t,
            Op::JmpIf(a, t) => {
                if f.get(a).truthy() {
                    *pc = t;
                }
            }
            Op::JmpNot(a, t) => {
                if !f.get(a).truthy() {
                    *pc = t;
                }
            }
            Op::JmpNil(a, t) => {
                if f.get(a).is_nil() {
                    *pc = t;
                }
            }
            Op::OptArg(n, t) => {
                if n < f.opt_filled {
                    *pc = t;
                }
            }
            Op::KwArg(n, t) => {
                if f.kw_mask & (1 << n.min(63)) != 0 {
                    *pc = t;
                }
            }
            Op::Except(a) => {
                let exc = f.exc.take().unwrap_or(Value::Nil);
                f.set(a, exc);
            }
            Op::Rescue(a, b) => {
                let exc = f.get(a);
                let classes = match f.get(b) {
                    Value::Array(list) => list.borrow().clone(),
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
                f.set(b, Value::Bool(hit));
            }
            Op::Raise(a) => {
                let v = f.get(a);
                if v.is_nil()
                    && let Some(pending) = f.pending.take()
                {
                    return Err(pending); // end of an ensure run for a break/return
                }
                return Err(Unwind::Error(v));
            }
            Op::Send(a, mid, argc, has_block) => {
                let recv = f.get(a);
                let args = (0..argc).map(|i| f.get(a + 1 + i)).collect();
                let blk = if has_block {
                    self.block_from(f.get(a + argc + 1))?
                } else {
                    None
                };
                let v = self.call_with_block(&recv, mid, args, blk)?;
                f.set(a, v);
            }
            Op::SendV(a, mid, has_block) => {
                let recv = f.get(a);
                let args = self.args_from_array(f.get(a + 1));
                let blk = if has_block {
                    self.block_from(f.get(a + 2))?
                } else {
                    None
                };
                let v = self.call_with_block(&recv, mid, args, blk)?;
                f.set(a, v);
            }
            Op::Super(a, argc) => {
                let args: Vec<Value> = (0..argc).map(|i| f.get(a + 1 + i)).collect();
                let blk = self.block_from(f.get(a + argc + 1))?;
                let v = self.super_call(f, args, blk)?;
                f.set(a, v);
            }
            Op::SuperV(a) => {
                let args = self.args_from_array(f.get(a + 1));
                let blk = self.block_from(f.get(a + 2))?;
                let v = self.super_call(f, args, blk)?;
                f.set(a, v);
            }
            Op::BlkPush(a, required) => match &f.block {
                Some(b) => f.set(a, b.clone()),
                None if required => {
                    return Err(self.err(&self.c.local_jump_error, "no block given (yield)"));
                }
                None => f.set(a, Value::Nil),
            },
            Op::BlockGiven(a) => f.set(a, Value::Bool(f.block.is_some())),
            Op::Return(a) => return Ok(Some(f.get(a))),
            Op::ReturnBlk(a) => {
                if !self.live_homes.contains(&f.home) {
                    return Err(self.err(&self.c.local_jump_error, "unexpected return"));
                }
                return Err(Unwind::Return {
                    home: f.home,
                    value: f.get(a),
                });
            }
            Op::Break(a) => {
                if !self.live_breaks.contains(&f.proc_tag) {
                    return Err(self.err(&self.c.local_jump_error, "break from proc-closure"));
                }
                return Err(Unwind::Break {
                    tag: f.proc_tag,
                    value: f.get(a),
                });
            }
            Op::Add(a) => self.arith_op(f, a, Arith::Add, self.s.add)?,
            Op::Sub(a) => self.arith_op(f, a, Arith::Sub, self.s.sub)?,
            Op::Mul(a) => self.arith_op(f, a, Arith::Mul, self.s.mul)?,
            Op::Div(a) => self.arith_op(f, a, Arith::Div, self.s.div)?,
            Op::Eq(a) => {
                let (x, y) = (f.get(a), f.get(a + 1));
                let v = self.equal(&x, &y)?;
                f.set(a, Value::Bool(v));
            }
            Op::Lt(a) => self.cmp_op(f, a, self.s.lt, Ordering::is_lt)?,
            Op::Le(a) => self.cmp_op(f, a, self.s.le, Ordering::is_le)?,
            Op::Gt(a) => self.cmp_op(f, a, self.s.gt, Ordering::is_gt)?,
            Op::Ge(a) => self.cmp_op(f, a, self.s.ge, Ordering::is_ge)?,
            Op::Not(a) => f.set(a, Value::Bool(!f.get(a).truthy())),
            Op::Array(a, b, n) => {
                let v = (0..n).map(|i| f.get(b + i)).collect();
                f.set(a, Value::array(v));
            }
            Op::ArrayCat(a, b) => {
                let items = self.splat_to_vec(&f.get(b))?;
                if let Value::Array(arr) = f.get(a) {
                    arr.borrow_mut().extend(items);
                }
            }
            Op::ArrayPush(a, b) => {
                if let Value::Array(arr) = f.get(a) {
                    arr.borrow_mut().push(f.get(b));
                }
            }
            Op::ToAry(a, b) => {
                let arr = self.masgn_array(&f.get(b))?;
                f.set(a, Value::Array(arr));
            }
            Op::ARef(a, b, i) => {
                let v = match f.get(b) {
                    Value::Array(arr) => arr.borrow().get(i).cloned().unwrap_or(Value::Nil),
                    other if i == 0 => other,
                    _ => Value::Nil,
                };
                f.set(a, v);
            }
            Op::ASplat(a, b, pre, post) => {
                let items = self.args_from_array(f.get(b));
                let start = pre.min(items.len());
                let end = items.len().saturating_sub(post).max(start);
                f.set(a, Value::array(items[start..end].to_vec()));
            }
            Op::APost(a, b, pre, post, j) => {
                let items = self.args_from_array(f.get(b));
                let idx = pre.max(items.len().saturating_sub(post)) + j;
                f.set(a, items.get(idx).cloned().unwrap_or(Value::Nil));
            }
            Op::Hash(a, b, n) => {
                let mut h = RHash::new();
                for i in 0..n {
                    h.set(f.get(b + 2 * i), f.get(b + 2 * i + 1));
                }
                f.set(a, Value::hash(h));
            }
            Op::HashAdd(a, b) => {
                if let Value::Hash(h) = f.get(a) {
                    h.borrow_mut().set(f.get(b), f.get(b + 1));
                }
            }
            Op::HashCat(a, b) => {
                let pairs = match f.get(b) {
                    Value::Hash(other) => other.borrow().pairs.clone(),
                    Value::Nil => vec![],
                    v => {
                        let what = self.class_name(&v);
                        return Err(
                            self.type_err(format!("no implicit conversion of {what} into Hash"))
                        );
                    }
                };
                if let Value::Hash(h) = f.get(a) {
                    let mut h = h.borrow_mut();
                    for (k, v) in pairs {
                        h.set(k, v);
                    }
                }
            }
            Op::KwHash(a) => {
                if let Value::Hash(h) = f.get(a) {
                    h.borrow_mut().kw = true;
                }
            }
            Op::String(a, i) => {
                let Literal::Str(s) = &f.irep.pool[i] else {
                    unreachable!()
                };
                f.set(a, Value::str(s.to_string()));
            }
            Op::StrCat(a) => {
                let piece = match f.get(a + 1) {
                    Value::Str(s) => s.borrow().clone(),
                    other => self.inspect(&other)?,
                };
                if let Value::Str(s) = f.get(a) {
                    s.borrow_mut().push_str(&piece);
                }
            }
            Op::Intern(a) => {
                let v = match f.get(a) {
                    Value::Str(s) => {
                        let s = s.borrow().clone();
                        Value::Sym(self.intern(&s))
                    }
                    v => v,
                };
                f.set(a, v);
            }
            Op::Range(a, excl) => f.set(a, Value::Range(Rc::new((f.get(a), f.get(a + 1), excl)))),
            Op::Block(a, idx) | Op::Lambda(a, idx) => {
                let p = RProc {
                    body: ProcBody::Ruby {
                        irep: f.irep.reps[idx].clone(),
                        env: f.env.clone(),
                    },
                    self_: f.self_.clone(),
                    block: f.block.clone(),
                    home: f.home,
                    method: f.method.clone(),
                    target: f.target.clone(),
                    lambda: matches!(op, Op::Lambda(..)),
                };
                f.set(a, Value::Proc(Rc::new(p)));
            }
            Op::TDef(s, idx) => {
                let irep = f.irep.reps[idx].clone();
                f.target.methods.borrow_mut().insert(s, Method::Ruby(irep));
            }
            Op::SDef(a, s, idx) => {
                let cls = self.singleton_class(&f.get(a))?;
                cls.methods
                    .borrow_mut()
                    .insert(s, Method::Ruby(f.irep.reps[idx].clone()));
            }
            Op::Class(a, name, is_module) => {
                let cls = self.define_class(f, a, name, is_module)?;
                f.set(a, Value::Class(cls));
            }
            Op::SClass(a) => {
                let cls = self.singleton_class(&f.get(a))?;
                f.set(a, Value::Class(cls));
            }
            Op::Exec(a, idx) => {
                let Value::Class(cls) = f.get(a) else {
                    unreachable!()
                };
                let v = self.exec_class_body(cls, f.irep.reps[idx].clone())?;
                f.set(a, v);
            }
            Op::Alias(new, old) => self.alias_method(&f.target, new, old)?,
            Op::GAlias(new, old) => {
                let v = self.globals.get(&old).cloned().unwrap_or(Value::Nil);
                self.globals.insert(new, v);
            }
            Op::Undef(s) => self.undef_method(&f.target, s)?,
            Op::Defined(a, kind, s) => {
                let v = self.defined(f, kind, s);
                f.set(a, v.map(Value::str).unwrap_or(Value::Nil));
            }
            Op::CurMethod(a) => {
                let v = f
                    .method
                    .as_ref()
                    .map(|(_, m)| Value::Sym(*m))
                    .unwrap_or(Value::Nil);
                f.set(a, v);
            }
        }
        Ok(None)
    }

    fn super_call(&mut self, f: &Frame, args: Vec<Value>, blk: Option<Value>) -> VmResult {
        let Some((owner, mid)) = f.method.clone() else {
            return Err(self.err(&self.c.no_method_error, "super called outside of method"));
        };
        let Some((m, o)) = self.find_super(&f.self_, &owner, mid) else {
            if let Some((mm, mo)) = self.lookup(&f.self_, self.s.method_missing) {
                let mut margs = vec![Value::Sym(mid)];
                margs.extend(args);
                let name = self.s.method_missing;
                return self.invoke(mm, mo, f.self_.clone(), name, margs, blk);
            }
            let msg = format!("super: no superclass method '{}'", self.sym_name(mid));
            return Err(self.err(&self.c.no_method_error, msg));
        };
        let tag = proc_tag(&blk);
        self.live_breaks.push(tag);
        let r = self.invoke(m, o, f.self_.clone(), mid, args, blk);
        self.live_breaks.pop();
        match r {
            Err(Unwind::Break { tag: t, value }) if tag != 0 && t == tag => Ok(value),
            r => r,
        }
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
            DefinedKind::ScopedConst(r) => match f.get(r) {
                Value::Class(c) => self.get_const_in(&c, s).is_some(),
                _ => false,
            },
            DefinedKind::Method(r, _) => self.respond_to(&f.get(r), s),
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
        let outer = match f.get(a) {
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
                if let Value::Class(sup) = f.get(a + 1) {
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
                    match f.get(a + 1) {
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

    fn arith_op(&mut self, f: &Frame, a: Reg, op: Arith, mid: Sym) -> Result<(), Unwind> {
        let (x, y) = (f.get(a), f.get(a + 1));
        let v = if x.is_numeric() && y.is_numeric() {
            self.arith(op, &x, &y)?
        } else {
            self.call_method(&x, mid, vec![y], None)?
        };
        f.set(a, v);
        Ok(())
    }

    fn cmp_op(
        &mut self,
        f: &Frame,
        a: Reg,
        mid: Sym,
        pred: fn(Ordering) -> bool,
    ) -> Result<(), Unwind> {
        let (x, y) = (f.get(a), f.get(a + 1));
        let v = if x.is_numeric() && y.is_numeric() {
            Value::Bool(num_cmp(&x, &y).is_some_and(pred))
        } else {
            self.call_method(&x, mid, vec![y], None)?
        };
        f.set(a, v);
        Ok(())
    }

    /// `a == b`, dispatching to user-defined `==` for objects.
    pub fn equal(&mut self, a: &Value, b: &Value) -> Result<bool, Unwind> {
        if let Value::Object(_) = a {
            return Ok(self
                .call_method(a, self.s.eq, vec![b.clone()], None)?
                .truthy());
        }
        Ok(values_eq(a, b))
    }

    // ---- conversions (src/print.c, object.c) ---------------------------

    pub fn to_s(&mut self, v: &Value) -> Result<String, Unwind> {
        Ok(match v {
            Value::Str(s) => s.borrow().clone(),
            Value::Nil => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(x) => fmt_float(*x),
            Value::Sym(s) => self.sym_name(*s).to_string(),
            _ => match self.call_method(v, self.s.to_s, vec![], None)? {
                Value::Str(s) => s.borrow().clone(),
                _ => format!("#<{}>", self.class_name(v)),
            },
        })
    }

    fn inspect_array(&mut self, a: &Rc<RefCell<Vec<Value>>>) -> Result<String, Unwind> {
        let items = a.borrow().clone();
        let mut parts = Vec::with_capacity(items.len());
        for x in &items {
            parts.push(self.inspect(x)?);
        }
        Ok(format!("[{}]", parts.join(", ")))
    }

    fn inspect_hash(&mut self, h: &Rc<RefCell<RHash>>) -> Result<String, Unwind> {
        let pairs = h.borrow().pairs.clone();
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
            Value::Str(s) => inspect_str(&s.borrow()),
            Value::Array(a) => {
                let id = Rc::as_ptr(a) as usize;
                if self.inspecting.contains(&id) {
                    return Ok("[...]".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_array(a);
                self.inspecting.pop();
                r?
            }
            Value::Hash(h) => {
                let id = Rc::as_ptr(h) as usize;
                if self.inspecting.contains(&id) {
                    return Ok("{...}".into());
                }
                self.inspecting.push(id);
                let r = self.inspect_hash(h);
                self.inspecting.pop();
                r?
            }
            Value::Range(r) => {
                let lo = if r.0.is_nil() {
                    String::new()
                } else {
                    self.inspect(&r.0)?
                };
                let hi = if r.1.is_nil() {
                    String::new()
                } else {
                    self.inspect(&r.1)?
                };
                format!("{lo}{}{hi}", if r.2 { "..." } else { ".." })
            }
            Value::Class(_) | Value::Proc(_) | Value::Object(_) => {
                match self.call_method(v, self.s.inspect, vec![], None)? {
                    Value::Str(s) => s.borrow().clone(),
                    _ => format!("#<{}>", self.class_name(v)),
                }
            }
        })
    }
}

fn proc_tag(blk: &Option<Value>) -> usize {
    match blk {
        Some(Value::Proc(p)) => Rc::as_ptr(p) as usize,
        _ => 0,
    }
}

fn upvar_env(f: &Frame, depth: usize) -> Rc<Env> {
    let mut env = f.env.upper.clone().expect("upvar outside of block");
    for _ in 0..depth {
        env = env.upper.clone().expect("upvar depth");
    }
    env
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
