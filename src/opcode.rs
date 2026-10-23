//! Bytecode: the counterpart of `include/mruby/ops.h` and `include/mruby/irep.h`.
//!
//! Like mruby, the VM is register based: `R[0]` is `self`, `R[1..=nlocals]`
//! hold the arguments and local variables, and temporaries live above them.
//! Instruction names and semantics follow mruby's opcodes (see the comments),
//! but operands are stored as a Rust enum rather than a packed byte stream.
//! Packing into mruby's variable-length encoding (and `.mrb` dump/load) is a
//! later, mechanical step.

use crate::symbol::{Sym, SymbolTable};
use crate::gc::ClassRef;
use crate::value::{Method, Vis};
use std::cell::RefCell;
use std::fmt::Write;
use std::rc::Rc;

pub type Reg = usize;

/// What the `defined?` check in `Op::Defined` looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DefinedKind {
    IVar,
    GVar,
    CVar,
    /// lexical constant lookup
    Const,
    /// `R[reg]::Name`
    ScopedConst(Reg),
    /// method on `R[reg]`; `true` = private calls allowed (no explicit receiver)
    Method(Reg, bool),
    Super,
}

#[derive(Debug, Clone, Copy)]
pub enum Op {
    Nop,
    /// R[a] = R[b]
    Move(Reg, Reg),
    /// R[a] = integer (mruby: LOADI8/LOADI16/LOADI32/LOADI_n)
    LoadI(Reg, i64),
    /// R[a] = Pool[b]
    LoadL(Reg, usize),
    LoadSym(Reg, Sym),
    LoadNil(Reg),
    LoadSelf(Reg),
    LoadTrue(Reg),
    LoadFalse(Reg),
    GetGV(Reg, Sym),
    SetGV(Reg, Sym),
    GetIV(Reg, Sym),
    SetIV(Reg, Sym),
    GetCV(Reg, Sym),
    SetCV(Reg, Sym),
    GetConst(Reg, Sym),
    SetConst(Reg, Sym),
    /// R[a] = R[a]::Sym
    GetMCnst(Reg, Sym),
    /// R[a+1]::Sym = R[a]
    SetMCnst(Reg, Sym),
    /// R[a] = Object (for `::Name`)
    OClass(Reg),
    /// R[a] = uvget(idx, depth)
    GetUpVar(Reg, usize, usize),
    /// uvset(idx, depth, R[a])
    SetUpVar(Reg, usize, usize),
    Jmp(usize),
    JmpIf(Reg, usize),
    JmpNot(Reg, usize),
    JmpNil(Reg, usize),
    /// Jump if optional argument #n was supplied (mruby encodes this in ENTER + a jump table).
    OptArg(usize, usize),
    /// Jump if keyword argument #n was supplied (mruby: KEY_P).
    KwArg(usize, usize),
    /// R[a] = current exception
    Except(Reg),
    /// R[b] = R[a].is_a?(R[b])  (mruby: RESCUE); an Array in R[b] means any of its classes
    Rescue(Reg, Reg),
    /// raise R[a]
    Raise(Reg),
    /// R[a] = R[a].send(sym, R[a+1]..R[a+n]) with block R[a+n+1] if flag
    Send(Reg, Sym, usize, bool),
    /// R[a] = R[a].send(sym, *R[a+1]) with block R[a+2] if flag
    SendV(Reg, Sym, bool),
    /// R[a] = super(R[a+1]..R[a+n]) with block R[a+n+1]
    Super(Reg, usize),
    /// R[a] = super(*R[a+1]) with block R[a+2]
    SuperV(Reg),
    /// R[a] = block of the current method (raise LocalJumpError if missing and flag set)
    BlkPush(Reg, bool),
    BlockGiven(Reg),
    Return(Reg),
    /// `return` inside a block: return from the enclosing method.
    ReturnBlk(Reg),
    /// `break` inside a block.
    Break(Reg),
    /// R[a] = R[a] + i (mruby: ADDI). Anything but an Integer without
    /// overflow takes the slow path (Float, or a call to `+`).
    AddI(Reg, i64),
    /// R[a] = R[a] - i (mruby: SUBI)
    SubI(Reg, i64),
    /// R[a] = R[b] + i: `x + 1` with x a local (no Move first)
    AddIR(Reg, Reg, i64),
    /// R[a] = R[b] - i
    SubIR(Reg, Reg, i64),
    /// R[a] = R[a] op R[a+1], with a fast path for numbers.
    Add(Reg),
    Sub(Reg),
    Mul(Reg),
    Div(Reg),
    Eq(Reg),
    Lt(Reg),
    Le(Reg),
    Gt(Reg),
    Ge(Reg),
    Not(Reg),
    /// R[a] = [R[b], ..., R[b+n-1]]  (mruby: ARRAY2)
    Array(Reg, Reg, usize),
    /// R[a].concat(splat(R[b]))  (mruby: ARYCAT)
    ArrayCat(Reg, Reg),
    /// R[a] << R[b]  (mruby: ARYPUSH)
    ArrayPush(Reg, Reg),
    /// R[a] = R[b] if it's an Array, else [R[b]] (or R[b].to_ary)
    ToAry(Reg, Reg),
    /// R[a] = R[b][c] (for multiple assignment)
    ARef(Reg, Reg, usize),
    /// R[a] = R[b][pre...len-post]  (the splat of `a, *b, c = ...`; mruby: APOST)
    ASplat(Reg, Reg, usize, usize),
    /// R[a] = the j-th of the `post` trailing elements of R[b] after `pre` leading ones
    APost(Reg, Reg, usize, usize, usize),
    /// R[a] = {R[b] => R[b+1], ...} with n pairs
    Hash(Reg, Reg, usize),
    /// R[a][R[b]] = R[b+1]
    HashAdd(Reg, Reg),
    /// R[a].merge!(R[b])  (`**h`)
    HashCat(Reg, Reg),
    /// marks the Hash in R[a] as keyword arguments
    KwHash(Reg),
    /// R[a] = str_dup(Pool[b])
    String(Reg, usize),
    /// R[a] << R[a+1]
    StrCat(Reg),
    /// R[a] = R[a].to_sym
    Intern(Reg),
    /// R[a] = R[a]..R[a+1]  (exclusive flag)
    Range(Reg, bool),
    /// R[a] = closure(Irep[b])
    Block(Reg, usize),
    /// R[a] = lambda(Irep[b])
    Lambda(Reg, usize),
    /// target_class.define_method(sym, Irep[b]), with the frame's default visibility
    TDef(Sym, usize),
    /// The default visibility of the methods `def` defines from here on in
    /// this frame (`private`, `public`, `protected`, `module_function` with
    /// no arguments).
    DefaultVis(Vis),
    /// R[a].freeze (what `defined?` answers is a frozen string)
    Freeze(Reg),
    /// R[a].singleton_class.define_method(sym, Irep[b])
    SDef(Reg, Sym, usize),
    /// R[a] = class/module `sym` under R[a] (nil = current), superclass R[a+1]
    Class(Reg, Sym, bool),
    /// R[a] = R[a].singleton_class
    SClass(Reg),
    /// R[a] = run Irep[b] with self = R[a] (class body)
    Exec(Reg, usize),
    /// alias_method(target_class, new, old)
    Alias(Sym, Sym),
    /// global variable alias
    GAlias(Sym, Sym),
    Undef(Sym),
    /// R[a] = description string if defined, else nil
    Defined(Reg, DefinedKind, Sym),
    /// R[a] = name of the current method (`__method__`)
    CurMethod(Reg),
}

impl Op {
    fn sym(&self) -> Option<Sym> {
        match *self {
            Op::LoadSym(_, s)
            | Op::GetGV(_, s)
            | Op::SetGV(_, s)
            | Op::GetIV(_, s)
            | Op::SetIV(_, s)
            | Op::GetCV(_, s)
            | Op::SetCV(_, s)
            | Op::GetConst(_, s)
            | Op::SetConst(_, s)
            | Op::GetMCnst(_, s)
            | Op::SetMCnst(_, s)
            | Op::Send(_, s, _, _)
            | Op::SendV(_, s, _)
            | Op::TDef(s, _)
            | Op::SDef(_, s, _)
            | Op::Class(_, s, _)
            | Op::Undef(s)
            | Op::Defined(_, _, s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Literal {
    Str(Rc<str>),
    Float(f64),
}

/// An entry of the rescue/ensure table (mruby: `mrb_irep_catch_handler`).
#[derive(Debug, Clone, Copy)]
pub struct CatchHandler {
    pub start: usize,
    pub end: usize,
    pub target: usize,
    /// An `ensure` clause: also runs when `break`/`return` unwind through it.
    pub ensure: bool,
}

/// Parameter layout (mruby: the `aspec` operand of OP_ENTER). Arguments are
/// bound to registers 1.. in this order: required, optional, rest, post,
/// keywords, kwrest, block.
#[derive(Debug, Default, Clone)]
pub struct ArgSpec {
    pub req: usize,
    pub opt: usize,
    pub rest: bool,
    pub post: usize,
    /// (name, required?)
    pub kw: Vec<(Sym, bool)>,
    pub kwrest: bool,
    /// `**nil`: no keywords accepted
    pub nokw: bool,
    pub block: bool,
    /// `|a, |`: splat a single array argument even with one parameter
    pub auto_splat: bool,
}

impl ArgSpec {
    pub fn mandatory(&self) -> usize {
        self.req + self.post
    }

    pub fn takes_kw(&self) -> bool {
        !self.kw.is_empty() || self.kwrest
    }

    /// `Proc#arity` / `Method#arity`
    pub fn arity(&self) -> i64 {
        let required_kw = self.kw.iter().any(|(_, r)| *r);
        let m = self.mandatory() as i64 + required_kw as i64;
        let optional_kw = !required_kw && self.takes_kw();
        if self.rest || self.opt > 0 || optional_kw {
            -(m + 1)
        } else {
            m
        }
    }

    /// Registers used by parameters.
    pub fn nparams(&self) -> usize {
        self.req
            + self.opt
            + self.rest as usize
            + self.post
            + self.kw.len()
            + self.kwrest as usize
            + self.block as usize
    }
}

/// A compiled unit: method body, block, class body, or top-level script.
/// Inline cache of a `Send`: the method found for receivers of one class
/// (by heap id) in one method epoch (mruby: the method cache; CRuby: the
/// call cache of a call site).
#[derive(Default)]
pub struct CallCache {
    pub class: u64,
    pub epoch: u64,
    pub hit: Option<(Method, ClassRef, Vis)>,
}

#[derive(Debug, Default)]
pub struct Irep {
    pub name: String,
    pub code: Vec<Op>,
    /// One inline cache per instruction (used by `Send`).
    pub caches: Box<[RefCell<CallCache>]>,
    pub pool: Vec<Literal>,
    pub reps: Vec<Rc<Irep>>,
    pub catch: Vec<CatchHandler>,
    pub nlocals: usize,
    pub nregs: usize,
    pub spec: ArgSpec,
}

impl std::fmt::Debug for CallCache {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "CallCache")
    }
}

impl Irep {
    /// Disassembly, like `mruby -v` / `mrbc -v` (src/codedump.c).
    pub fn dump(&self, syms: &SymbolTable) -> String {
        let mut out = String::new();
        self.dump_into(syms, &mut out);
        out
    }

    fn dump_into(&self, syms: &SymbolTable, out: &mut String) {
        let s = &self.spec;
        let _ = writeln!(
            out,
            "irep {} nregs={} nlocals={} args=(req={} opt={} rest={} post={} kw={} kwrest={} block={}) pools={} reps={}",
            self.name,
            self.nregs,
            self.nlocals,
            s.req,
            s.opt,
            s.rest,
            s.post,
            s.kw.len(),
            s.kwrest,
            s.block,
            self.pool.len(),
            self.reps.len()
        );
        for h in &self.catch {
            let _ = writeln!(
                out,
                "  catch [{:03}, {:03}) -> {:03}",
                h.start, h.end, h.target
            );
        }
        for (i, op) in self.code.iter().enumerate() {
            let _ = write!(out, "  {i:03} {op:?}");
            if let Some(s) = op.sym() {
                let _ = write!(out, "\t; :{}", syms.name(s));
            }
            if let Op::LoadL(_, p) | Op::String(_, p) = op {
                let _ = write!(out, "\t; {:?}", self.pool[*p]);
            }
            out.push('\n');
        }
        out.push('\n');
        for r in &self.reps {
            r.dump_into(syms, out);
        }
    }
}
