//! AST → register bytecode. The counterpart of `mrbgems/mruby-compiler/src/codegen.c`.
//!
//! Register allocation follows mruby: `R0` = self, `R1..=nlocals` = locals
//! (parameters first), and a stack pointer `sp` hands out temporaries above
//! them. `expr(node, dst)` compiles `node` so that its value ends up in `dst`
//! and leaves `sp` where it found it. `dst` is always below `sp`.

use crate::ast::*;
use crate::opcode::{ArgSpec, CatchHandler, Cmp, DefinedKind, Irep, Literal, Op, Reg};
use crate::symbol::{Sym, SymbolTable};
use crate::value::Vis;
use std::cell::RefCell;
use std::rc::Rc;

type CResult<T> = Result<T, String>;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Top,
    Method,
    Block,
    Class,
}

struct LoopCtx {
    dst: Reg,
    /// start of the body, for `redo`
    body_start: usize,
    breaks: Vec<usize>,
    nexts: Vec<usize>,
    /// ensure clauses entered before the loop started
    ens_base: usize,
}

struct Scope {
    irep: Irep,
    locals: Vec<String>,
    kind: Kind,
    sp: Reg,
    /// The first temporary (registers below it are self and locals).
    first_temp: Reg,
    loops: Vec<LoopCtx>,
    /// method parameters, for `super` without arguments
    params: Option<Params>,
    /// `ensure` bodies of the begin blocks we're inside, inlined on return/break/next
    ensures: Vec<Node>,
    /// start of the begin bodies we're rescuing, for `retry`
    retry: Vec<usize>,
    /// start of a block body, for `redo`
    redo: Option<usize>,
}

pub struct Codegen<'a> {
    syms: &'a mut SymbolTable,
    scopes: Vec<Scope>,
}

pub fn compile(prog: &Program, syms: &mut SymbolTable) -> CResult<Rc<Irep>> {
    let mut g = Codegen {
        syms,
        scopes: Vec::new(),
    };
    g.push_scope(Kind::Top, "<main>", &prog.locals, None);
    let r = g.alloc();
    g.expr(&prog.body, r)?;
    g.emit(Op::Return(r));
    Ok(Rc::new(g.pop_scope()))
}

fn binop(name: &str) -> Option<fn(Reg) -> Op> {
    Some(match name {
        "+" => Op::Add,
        "-" => Op::Sub,
        "*" => Op::Mul,
        "/" => Op::Div,
        "==" => Op::Eq,
        "<" => Op::Lt,
        "<=" => Op::Le,
        ">" => Op::Gt,
        ">=" => Op::Ge,
        _ => return None,
    })
}

/// Whether compiling `node` into a register writes that register only at
/// the end, after every variable it reads. Then a local can be its own
/// destination (`x = x + 1`). Conditionals and sequences write their
/// destination early (the condition, the first statement), so they can't.
fn fills_dst_last(node: &Node) -> bool {
    matches!(
        node,
        Node::Nil
            | Node::True
            | Node::False
            | Node::SelfRef
            | Node::Int(_)
            | Node::Float(_)
            | Node::Str(_)
            | Node::DStr(_)
            | Node::Sym(_)
            | Node::DSym(_)
            | Node::Array(_)
            | Node::Hash(..)
            | Node::Range(..)
            | Node::LVar(_)
            | Node::IVar(_)
            | Node::GVar(_)
            | Node::CVar(_)
            | Node::Const(_)
            | Node::Colon3(_)
            | Node::Call { .. }
            | Node::Super { .. }
            | Node::Yield(_)
            | Node::Lambda(_)
    )
}

fn has_splat(args: &[Node]) -> bool {
    args.iter().any(|a| matches!(a, Node::Splat(_)))
}

impl Codegen<'_> {
    // ---- scope & emission helpers -------------------------------------

    fn push_scope(&mut self, kind: Kind, name: &str, locals: &[String], params: Option<Params>) {
        let nlocals = locals.len() + 1;
        let irep = Irep {
            name: name.to_string(),
            nlocals,
            nregs: nlocals,
            ..Default::default()
        };
        self.scopes.push(Scope {
            irep,
            locals: locals.to_vec(),
            kind,
            sp: nlocals,
            first_temp: nlocals,
            loops: vec![],
            params,
            ensures: vec![],
            retry: vec![],
            redo: None,
        });
    }

    fn pop_scope(&mut self) -> Irep {
        let mut irep = self.scopes.pop().unwrap().irep;
        if irep.catch.is_empty() {
            peephole(&mut irep.code);
        }
        irep.sets_vis = sets_vis(&irep);
        let spec = &irep.spec;
        irep.simple_args = spec.opt == 0
            && !spec.rest
            && spec.post == 0
            && !spec.takes_kw()
            && !spec.nokw
            && !spec.auto_splat;
        irep.frame_size = irep.nregs.max(1 + spec.nparams());
        irep.needs_env = reach(&irep) > 0 || irep.sets_vis;
        irep.caches = irep.code.iter().map(|_| RefCell::default()).collect();
        irep
    }

    fn add_child(&mut self, irep: Irep) -> usize {
        let reps = &mut self.cur().irep.reps;
        reps.push(Rc::new(irep));
        reps.len() - 1
    }

    fn cur(&mut self) -> &mut Scope {
        self.scopes.last_mut().unwrap()
    }

    fn kind(&self) -> Kind {
        self.scopes.last().unwrap().kind
    }

    fn sp(&self) -> Reg {
        self.scopes.last().unwrap().sp
    }

    fn alloc(&mut self) -> Reg {
        let s = self.cur();
        let r = s.sp;
        s.sp += 1;
        s.irep.nregs = s.irep.nregs.max(s.sp);
        r
    }

    fn free_to(&mut self, r: Reg) {
        self.cur().sp = r;
    }

    /// The register to compute an operation in: `dst` itself when it is the
    /// topmost temporary (so no `Move` is needed afterwards), else a new
    /// temporary. A local's register is never used: in `x = 1 + x` the `1`
    /// would overwrite `x` before it is read.
    fn op_reg(&mut self, dst: Reg) -> Reg {
        let s = self.scopes.last().unwrap();
        if dst + 1 == s.sp && dst >= s.first_temp {
            dst
        } else {
            self.alloc()
        }
    }

    /// Ends an operation computed in `a` (from [`Self::op_reg`]): its value
    /// goes to `dst` and the temporaries above are freed.
    fn op_done(&mut self, a: Reg, dst: Reg) {
        if a == dst {
            self.free_to(dst + 1);
        } else {
            self.emit(Op::Move(dst, a));
            self.free_to(a);
        }
    }

    fn emit(&mut self, op: Op) -> usize {
        let code = &mut self.cur().irep.code;
        code.push(op);
        code.len() - 1
    }

    fn pc(&self) -> usize {
        self.scopes.last().unwrap().irep.code.len()
    }

    /// Points the jump at `at` to the current pc.
    fn patch(&mut self, at: usize) {
        let target = self.pc();
        self.patch_to(at, target);
    }

    fn patch_to(&mut self, at: usize, target: usize) {
        match &mut self.cur().irep.code[at] {
            Op::Jmp(t)
            | Op::JmpIf(_, t)
            | Op::JmpNot(_, t)
            | Op::JmpNil(_, t)
            | Op::OptArg(_, t)
            | Op::KwArg(_, t) => *t = target,
            Op::JmpCmpI { target: t, .. } | Op::JmpCmpR { target: t, .. } => *t = target as u32,
            op => unreachable!("not a jump: {op:?}"),
        }
    }

    /// Evaluates `cond` into `r` and jumps when its truth is `jump_if`;
    /// returns the jumps to patch to the target. A comparison of a local
    /// with an Integer literal or another local is first tried as a fused
    /// JmpCmp, with the generic code after it for other operands.
    fn cond_jump(&mut self, cond: &Node, r: Reg, jump_if: bool) -> CResult<Vec<usize>> {
        let mut jumps = Vec::new();
        if let Some(fused) = self.fused_compare(cond, jump_if) {
            jumps.push(self.emit(fused));
        }
        self.expr(cond, r)?;
        jumps.push(self.emit(if jump_if { Op::JmpIf(r, 0) } else { Op::JmpNot(r, 0) }));
        let cont = self.pc() as u32;
        if jumps.len() == 2
            && let Op::JmpCmpI { cont: c, .. } | Op::JmpCmpR { cont: c, .. } = &mut self.cur().irep.code[jumps[0]]
        {
            *c = cont;
        }
        Ok(jumps)
    }

    /// `x < 2`, `i < n`, … with x, i, n locals of this scope.
    fn fused_compare(&mut self, cond: &Node, jump_if: bool) -> Option<Op> {
        let Node::Call { recv: Some(recv), name, args, block: None, block_arg: None, safe: false } = cond else {
            return None;
        };
        let cmp = match name.as_str() {
            "<" => Cmp::Lt,
            "<=" => Cmp::Le,
            ">" => Cmp::Gt,
            ">=" => Cmp::Ge,
            "==" => Cmp::Eq,
            _ => return None,
        };
        let local = |cg: &Self, n: &Node| match n {
            Node::LVar(v) => match cg.lookup(v) {
                Some((0, r)) => Some(r as u32),
                _ => None,
            },
            _ => None,
        };
        let a = local(self, recv)?;
        match args.as_slice() {
            [Node::Int(i)] => Some(Op::JmpCmpI { cmp, jump_if, a, imm: *i, target: 0, cont: 0 }),
            [other] => {
                let b = local(self, other)?;
                Some(Op::JmpCmpR { cmp, jump_if, a, b, target: 0, cont: 0 })
            }
            _ => None,
        }
    }

    fn sym(&mut self, s: &str) -> Sym {
        self.syms.intern(s)
    }

    fn pool(&mut self, lit: Literal) -> usize {
        let pool = &mut self.cur().irep.pool;
        pool.push(lit);
        pool.len() - 1
    }

    fn load_str(&mut self, dst: Reg, s: &str) {
        let i = self.pool(Literal::Str(Rc::from(s)));
        self.emit(Op::String(dst, i));
    }

    /// What `defined?` answers: a frozen string.
    fn defined_str(&mut self, dst: Reg, s: &str) {
        self.load_str(dst, s);
        self.emit(Op::Freeze(dst));
    }

    /// Finds a local: (scope depth, register).
    fn lookup(&self, name: &str) -> Option<(usize, Reg)> {
        for (depth, s) in self.scopes.iter().rev().enumerate() {
            if let Some(i) = s.locals.iter().position(|l| l == name) {
                return Some((depth, i + 1));
            }
            if s.kind != Kind::Block {
                break;
            }
        }
        None
    }

    fn set_local(&mut self, name: &str, src: Reg) -> CResult<()> {
        match self.lookup(name) {
            Some((0, reg)) => {
                if reg != src {
                    self.emit(Op::Move(reg, src));
                }
            }
            Some((depth, reg)) => _ = self.emit(Op::SetUpVar(src, reg, depth - 1)),
            None => return Err(format!("undeclared local variable '{name}'")),
        }
        Ok(())
    }

    // ---- expressions ---------------------------------------------------

    fn expr(&mut self, node: &Node, dst: Reg) -> CResult<()> {
        match node {
            Node::Nil => _ = self.emit(Op::LoadNil(dst)),
            Node::True => _ = self.emit(Op::LoadTrue(dst)),
            Node::False => _ = self.emit(Op::LoadFalse(dst)),
            Node::SelfRef => _ = self.emit(Op::LoadSelf(dst)),
            Node::Int(i) => _ = self.emit(Op::LoadI(dst, *i)),
            Node::Float(f) => {
                let i = self.pool(Literal::Float(*f));
                self.emit(Op::LoadL(dst, i));
            }
            Node::Str(s) => self.load_str(dst, s),
            Node::DStr(parts) => self.dstr(parts, dst)?,
            Node::DSym(parts) => {
                self.dstr(parts, dst)?;
                self.emit(Op::Intern(dst));
            }
            Node::Sym(s) => {
                let s = self.sym(s);
                self.emit(Op::LoadSym(dst, s));
            }
            Node::Array(elems) => self.array(elems, dst)?,
            Node::Splat(_) => self.array(std::slice::from_ref(node), dst)?,
            Node::Hash(items, kw) => self.hash(items, *kw, dst)?,
            Node::Range(lo, hi, excl) => {
                let a = self.alloc();
                self.expr(lo, a)?;
                let r = self.alloc();
                self.expr(hi, r)?;
                self.emit(Op::Range(a, *excl));
                self.emit(Op::Move(dst, a));
                self.free_to(a);
            }
            Node::LVar(name) => match self.lookup(name) {
                Some((0, reg)) => {
                    if reg != dst {
                        self.emit(Op::Move(dst, reg));
                    }
                }
                Some((depth, reg)) => _ = self.emit(Op::GetUpVar(dst, reg, depth - 1)),
                None => return Err(format!("undefined local variable '{name}'")),
            },
            Node::LAsgn(name, val) => match self.lookup(name) {
                Some((0, reg)) => {
                    self.assign_local(name, reg, val)?;
                    if reg != dst {
                        self.emit(Op::Move(dst, reg));
                    }
                }
                Some((depth, reg)) => {
                    self.expr(val, dst)?;
                    self.emit(Op::SetUpVar(dst, reg, depth - 1));
                }
                None => return Err(format!("undeclared local variable '{name}'")),
            },
            Node::IVar(n) => {
                let s = self.sym(n);
                self.emit(Op::GetIV(dst, s));
            }
            Node::GVar(n) => {
                let s = self.sym(n);
                self.emit(Op::GetGV(dst, s));
            }
            Node::CVar(n) => {
                let s = self.sym(n);
                self.emit(Op::GetCV(dst, s));
            }
            Node::Const(n) => {
                let s = self.sym(n);
                self.emit(Op::GetConst(dst, s));
            }
            Node::Colon2(base, n) => {
                self.expr(base, dst)?;
                let s = self.sym(n);
                self.emit(Op::GetMCnst(dst, s));
            }
            Node::Colon3(n) => {
                self.emit(Op::OClass(dst));
                let s = self.sym(n);
                self.emit(Op::GetMCnst(dst, s));
            }
            Node::IAsgn(n, v) => {
                self.expr(v, dst)?;
                let s = self.sym(n);
                self.emit(Op::SetIV(dst, s));
            }
            Node::GAsgn(n, v) => {
                self.expr(v, dst)?;
                let s = self.sym(n);
                self.emit(Op::SetGV(dst, s));
            }
            Node::CVAsgn(n, v) => {
                self.expr(v, dst)?;
                let s = self.sym(n);
                self.emit(Op::SetCV(dst, s));
            }
            Node::CAsgn(target, v) => self.const_assign(target, v, dst)?,
            Node::MAsgn(lhs, rhs) => self.masgn(lhs, rhs, dst)?,
            Node::Call {
                recv,
                name,
                args,
                block,
                block_arg,
                safe,
            } => self.call(
                recv.as_deref(),
                name,
                args,
                block.as_deref(),
                block_arg.as_deref(),
                *safe,
                dst,
            )?,
            Node::Super {
                args,
                block,
                block_arg,
            } => self.super_call(args.as_deref(), block.as_deref(), block_arg.as_deref(), dst)?,
            Node::Yield(args) => {
                let a = self.op_reg(dst);
                if has_splat(args) {
                    self.emit(Op::BlkPush(a, true));
                    let call = self.sym("call");
                    self.args(args)?;
                    self.emit(Op::SendV(a, call, false));
                } else {
                    let n = self.args(args)?.expect("no splat");
                    self.emit(Op::Yield(a, n));
                }
                self.op_done(a, dst);
            }
            Node::BlockGiven => _ = self.emit(Op::BlockGiven(dst)),
            Node::CurrentMethod => _ = self.emit(Op::CurMethod(dst)),
            Node::Not(e) => {
                self.expr(e, dst)?;
                self.emit(Op::Not(dst));
            }
            Node::And(l, r) => {
                self.expr(l, dst)?;
                let j = self.emit(Op::JmpNot(dst, 0));
                self.expr(r, dst)?;
                self.patch(j);
            }
            Node::Or(l, r) => {
                self.expr(l, dst)?;
                let j = self.emit(Op::JmpIf(dst, 0));
                self.expr(r, dst)?;
                self.patch(j);
            }
            Node::If(c, t, e) => {
                let jelse = self.cond_jump(c, dst, false)?;
                self.expr(t, dst)?;
                let jend = self.emit(Op::Jmp(0));
                for j in jelse {
                    self.patch(j);
                }
                self.expr(e, dst)?;
                self.patch(jend);
            }
            Node::While {
                cond,
                body,
                until,
                do_while,
            } => self.while_loop(cond, body, *until, *do_while, dst)?,
            Node::Seq(nodes) => match nodes.split_last() {
                None => _ = self.emit(Op::LoadNil(dst)),
                Some((last, init)) => {
                    for n in init {
                        self.stmt(n)?;
                    }
                    self.expr(last, dst)?;
                }
            },
            Node::Def {
                recv,
                name,
                params,
                body,
                locals,
            } => {
                let idx = self.method(name, params, body, locals)?;
                let s = self.sym(name);
                match recv {
                    Some(r) => {
                        self.expr(r, dst)?;
                        self.emit(Op::SDef(dst, s, idx));
                    }
                    None => _ = self.emit(Op::TDef(s, idx)),
                }
                self.emit(Op::LoadSym(dst, s));
            }
            Node::Class {
                path,
                superclass,
                body,
                locals,
                is_module,
            } => {
                let a = self.alloc();
                let sup = self.alloc();
                let name = match &**path {
                    Node::Const(n) => {
                        self.emit(Op::LoadNil(a));
                        n
                    }
                    Node::Colon2(base, n) => {
                        self.expr(base, a)?;
                        n
                    }
                    Node::Colon3(n) => {
                        self.emit(Op::OClass(a));
                        n
                    }
                    _ => return Err("bad class path".into()),
                };
                match superclass {
                    Some(s) => self.expr(s, sup)?,
                    None => _ = self.emit(Op::LoadNil(sup)),
                }
                let s = self.sym(name);
                self.emit(Op::Class(a, s, *is_module));
                let idx = self.class_body(name, body, locals)?;
                self.emit(Op::Exec(a, idx));
                self.emit(Op::Move(dst, a));
                self.free_to(a);
            }
            Node::SClass(target, body, locals) => {
                let a = self.alloc();
                self.expr(target, a)?;
                self.emit(Op::SClass(a));
                let idx = self.class_body("singleton class", body, locals)?;
                self.emit(Op::Exec(a, idx));
                self.emit(Op::Move(dst, a));
                self.free_to(a);
            }
            Node::Return(v) => {
                self.opt_expr(v.as_deref(), dst)?;
                self.inline_ensures(0, dst)?;
                let op = if self.kind() == Kind::Block {
                    Op::ReturnBlk(dst)
                } else {
                    Op::Return(dst)
                };
                self.emit(op);
            }
            Node::Break(v) => {
                if let Some((ldst, base)) = self.cur().loops.last().map(|l| (l.dst, l.ens_base)) {
                    self.opt_expr(v.as_deref(), ldst)?;
                    self.inline_ensures(base, dst)?;
                    let j = self.emit(Op::Jmp(0));
                    self.cur().loops.last_mut().unwrap().breaks.push(j);
                } else if self.kind() == Kind::Block {
                    self.opt_expr(v.as_deref(), dst)?;
                    self.inline_ensures(0, dst)?;
                    self.emit(Op::Break(dst));
                } else {
                    return Err("Invalid break".into());
                }
            }
            Node::Next(v) => {
                if let Some(base) = self.cur().loops.last().map(|l| l.ens_base) {
                    self.inline_ensures(base, dst)?;
                    let j = self.emit(Op::Jmp(0));
                    self.cur().loops.last_mut().unwrap().nexts.push(j);
                } else if self.kind() == Kind::Block {
                    self.opt_expr(v.as_deref(), dst)?;
                    self.inline_ensures(0, dst)?;
                    self.emit(Op::Return(dst));
                } else {
                    return Err("Invalid next".into());
                }
            }
            Node::Redo => {
                let in_loop = self.cur().loops.last().map(|l| (l.body_start, l.ens_base));
                let (target, base) = match (in_loop, self.cur().redo) {
                    (Some((t, base)), _) => (t, base),
                    (None, Some(t)) => (t, 0),
                    _ => return Err("Invalid redo".into()),
                };
                self.inline_ensures(base, dst)?;
                self.emit(Op::Jmp(target));
            }
            Node::Retry => match self.cur().retry.last() {
                Some(&target) => _ = self.emit(Op::Jmp(target)),
                None => return Err("Invalid retry".into()),
            },
            Node::Begin {
                body,
                rescues,
                else_body,
                ensure,
            } => self.begin(body, rescues, else_body.as_deref(), ensure.as_deref(), dst)?,
            Node::Lambda(blk) => {
                let idx = self.block(blk)?;
                self.emit(Op::Lambda(dst, idx));
            }
            Node::Defined(e) => self.defined(e, dst)?,
            Node::Alias(new, old) => {
                let (n, o) = (self.sym(new), self.sym(old));
                self.emit(Op::Alias(n, o));
                self.emit(Op::LoadNil(dst));
            }
            Node::GAlias(new, old) => {
                let (n, o) = (self.sym(new), self.sym(old));
                self.emit(Op::GAlias(n, o));
                self.emit(Op::LoadNil(dst));
            }
            Node::Undef(names) => {
                for n in names {
                    let s = self.sym(n);
                    self.emit(Op::Undef(s));
                }
                self.emit(Op::LoadNil(dst));
            }
        }
        Ok(())
    }

    fn opt_expr(&mut self, node: Option<&Node>, dst: Reg) -> CResult<()> {
        self.expr(node.unwrap_or(&Node::Nil), dst)
    }

    fn dstr(&mut self, parts: &[Node], dst: Reg) -> CResult<()> {
        let a = self.alloc();
        self.load_str(a, "");
        for part in parts {
            let r = self.alloc();
            self.expr(part, r)?;
            if !matches!(part, Node::Str(_)) {
                let to_s = self.sym("to_s");
                self.emit(Op::Send(r, to_s, 0, false));
            }
            self.emit(Op::StrCat(a));
            self.free_to(r);
        }
        self.emit(Op::Move(dst, a));
        self.free_to(a);
        Ok(())
    }

    /// Array literal, with `*splat` elements.
    fn array(&mut self, elems: &[Node], dst: Reg) -> CResult<()> {
        if !has_splat(elems) {
            let base = self.sp();
            for e in elems {
                let r = self.alloc();
                self.expr(e, r)?;
            }
            self.emit(Op::Array(dst, base, elems.len()));
            self.free_to(base);
            return Ok(());
        }
        // build in a temporary: elements may read the destination variable
        let t = self.alloc();
        let lead = elems
            .iter()
            .take_while(|e| !matches!(e, Node::Splat(_)))
            .count();
        let base = self.sp();
        for e in &elems[..lead] {
            let r = self.alloc();
            self.expr(e, r)?;
        }
        self.emit(Op::Array(t, base, lead));
        self.free_to(base);
        for e in &elems[lead..] {
            let r = self.alloc();
            match e {
                Node::Splat(inner) => {
                    self.expr(inner, r)?;
                    self.emit(Op::ArrayCat(t, r));
                }
                _ => {
                    self.expr(e, r)?;
                    self.emit(Op::ArrayPush(t, r));
                }
            }
            self.free_to(r);
        }
        self.emit(Op::Move(dst, t));
        self.free_to(t);
        Ok(())
    }

    fn hash(&mut self, items: &[HashItem], kw: bool, dst: Reg) -> CResult<()> {
        let t = self.alloc();
        let lead = items
            .iter()
            .take_while(|i| matches!(i, HashItem::Pair(..)))
            .count();
        let base = self.sp();
        for item in &items[..lead] {
            if let HashItem::Pair(k, v) = item {
                let r = self.alloc();
                self.expr(k, r)?;
                let r = self.alloc();
                self.expr(v, r)?;
            }
        }
        self.emit(Op::Hash(t, base, lead));
        self.free_to(base);
        for item in &items[lead..] {
            let r = self.alloc();
            match item {
                HashItem::Pair(k, v) => {
                    self.expr(k, r)?;
                    let r2 = self.alloc();
                    self.expr(v, r2)?;
                    self.emit(Op::HashAdd(t, r));
                }
                HashItem::Splat(h) => {
                    self.expr(h, r)?;
                    self.emit(Op::HashCat(t, r));
                }
            }
            self.free_to(r);
        }
        if kw {
            self.emit(Op::KwHash(t));
        }
        self.emit(Op::Move(dst, t));
        self.free_to(t);
        Ok(())
    }

    /// Compiles `node` for its effects only: its value is unused.
    fn stmt(&mut self, node: &Node) -> CResult<()> {
        match node {
            Node::LAsgn(name, val) if matches!(self.lookup(name), Some((0, _))) => {
                let Some((_, reg)) = self.lookup(name) else {
                    unreachable!()
                };
                self.assign_local(name, reg, val)
            }
            Node::Seq(nodes) => nodes.iter().try_for_each(|n| self.stmt(n)),
            _ => {
                let t = self.alloc();
                self.expr(node, t)?;
                self.free_to(t);
                Ok(())
            }
        }
    }

    /// `name = val` for a local of this scope (register `reg`).
    fn assign_local(&mut self, name: &str, reg: Reg, val: &Node) -> CResult<()> {
        // `i += 1`: one instruction on the local's register
        if let Node::Call {
            recv: Some(recv),
            name: op,
            args,
            block: None,
            block_arg: None,
            safe: false,
        } = val
            && matches!(&**recv, Node::LVar(n) if n == name)
            && let [Node::Int(i)] = args.as_slice()
            && (op == "+" || op == "-")
        {
            self.emit(if op == "+" {
                Op::AddI(reg, *i)
            } else {
                Op::SubI(reg, *i)
            });
            return Ok(());
        }
        if fills_dst_last(val) {
            return self.expr(val, reg);
        }
        // `x = (c ? x * 2 : x)` writes its destination before it reads `x`:
        // compute it elsewhere first
        let t = self.alloc();
        self.expr(val, t)?;
        self.emit(Op::Move(reg, t));
        self.free_to(t);
        Ok(())
    }

    /// Evaluates call arguments into `sp, sp+1, ...`. Returns the count, or
    /// None if they were splatted into a single Array (for `SendV`).
    fn args(&mut self, args: &[Node]) -> CResult<Option<usize>> {
        if has_splat(args) {
            let r = self.alloc();
            self.array(args, r)?;
            return Ok(None);
        }
        for arg in args {
            let r = self.alloc();
            self.expr(arg, r)?;
        }
        Ok(Some(args.len()))
    }

    /// Puts the block (literal or `&arg`) in the next register; returns whether there is one.
    fn block_arg(&mut self, block: Option<&Block>, block_arg: Option<&Node>) -> CResult<bool> {
        if let Some(blk) = block {
            let r = self.alloc();
            let idx = self.block(blk)?;
            self.emit(Op::Block(r, idx));
            Ok(true)
        } else if let Some(ba) = block_arg {
            let r = self.alloc();
            self.expr(ba, r)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        recv: Option<&Node>,
        name: &str,
        args: &[Node],
        block: Option<&Block>,
        block_arg: Option<&Node>,
        safe: bool,
        dst: Reg,
    ) -> CResult<()> {
        if let (None, [], None, None) = (recv, args, block, block_arg) {
            let vis = match name {
                "private" => Some(Vis::Private),
                "public" => Some(Vis::Public),
                "protected" => Some(Vis::Protected),
                "module_function" => Some(Vis::ModuleFunction),
                _ => None,
            };
            if let Some(vis) = vis {
                self.emit(Op::DefaultVis(vis));
                self.emit(Op::LoadNil(dst));
                return Ok(());
            }
        }
        if let (Some(recv), [arg], None, None, false, Some(op)) =
            (recv, args, block, block_arg, safe, binop(name))
            && !matches!(arg, Node::Splat(_))
        {
            let a = self.op_reg(dst);
            // `x + 1` with x a local: one instruction reading the local
            if let (Node::LVar(v), Node::Int(i), "+" | "-") = (recv, arg, name)
                && let Some((0, r)) = self.lookup(v)
            {
                self.emit(if name == "+" { Op::AddIR(a, r, *i) } else { Op::SubIR(a, r, *i) });
                self.op_done(a, dst);
                return Ok(());
            }
            self.expr(recv, a)?;
            // `x + 1`, `n - 2`: the operand is an immediate
            match (name, arg) {
                ("+", Node::Int(i)) => _ = self.emit(Op::AddI(a, *i)),
                ("-", Node::Int(i)) => _ = self.emit(Op::SubI(a, *i)),
                _ => {
                    let r = self.alloc();
                    self.expr(arg, r)?;
                    self.emit(op(a));
                }
            }
            self.op_done(a, dst);
            return Ok(());
        }
        let a = self.op_reg(dst);
        // `foo(x)`: SendSelf puts self in R[a] itself (a splat needs it
        // there before the arguments, which SendV doesn't do)
        let self_send = recv.is_none() && !has_splat(args);
        match recv {
            Some(r) => self.expr(r, a)?,
            None if self_send => {}
            None => _ = self.emit(Op::LoadSelf(a)),
        }
        let skip = if safe {
            Some(self.emit(Op::JmpNil(a, 0)))
        } else {
            None
        };
        let argc = self.args(args)?;
        let has_block = self.block_arg(block, block_arg)?;
        let mid = self.sym(name);
        match argc {
            Some(n) if self_send => self.emit(Op::SendSelf(a, mid, n, has_block)),
            Some(n) => self.emit(Op::Send(a, mid, n, has_block)),
            None => self.emit(Op::SendV(a, mid, has_block)),
        };
        if let Some(j) = skip {
            self.patch(j);
        }
        self.op_done(a, dst);
        Ok(())
    }

    fn super_call(
        &mut self,
        args: Option<&[Node]>,
        block: Option<&Block>,
        block_arg: Option<&Node>,
        dst: Reg,
    ) -> CResult<()> {
        let args: Vec<Node> = match args {
            Some(a) => a.to_vec(),
            // outside a method this fails at run time, as in Ruby
            None => self.zsuper_args().unwrap_or_default(),
        };
        let a = self.op_reg(dst);
        self.emit(Op::LoadSelf(a));
        let argc = self.args(&args)?;
        if !self.block_arg(block, block_arg)? {
            let r = self.alloc();
            self.emit(Op::BlkPush(r, false));
        }
        match argc {
            Some(n) => self.emit(Op::Super(a, n)),
            None => self.emit(Op::SuperV(a)),
        };
        self.op_done(a, dst);
        Ok(())
    }

    /// `super` without arguments passes the current method's parameters on.
    fn zsuper_args(&self) -> CResult<Vec<Node>> {
        let params = self
            .scopes
            .iter()
            .rev()
            .find(|s| s.kind != Kind::Block)
            .filter(|s| s.kind == Kind::Method)
            .and_then(|s| s.params.clone())
            .ok_or("super called outside of method")?;
        let mut args: Vec<Node> = params
            .required
            .iter()
            .map(|p| Node::LVar(p.name().to_string()))
            .collect();
        args.extend(params.optional.iter().map(|(n, _)| Node::LVar(n.clone())));
        if let Some(r) = &params.rest {
            args.push(Node::Splat(Box::new(Node::LVar(r.clone()))));
        }
        args.extend(params.post.iter().map(|p| Node::LVar(p.name().to_string())));
        let mut kw: Vec<HashItem> = params
            .kw
            .iter()
            .map(|(n, _)| HashItem::Pair(Node::Sym(n.clone()), Node::LVar(n.clone())))
            .collect();
        if let Some(r) = params.kwrest.as_ref().filter(|r| *r != "%nokw") {
            kw.push(HashItem::Splat(Node::LVar(r.clone())));
        }
        if !kw.is_empty() {
            args.push(Node::Hash(kw, true));
        }
        Ok(args)
    }

    // ---- assignment ----------------------------------------------------

    fn const_assign(&mut self, target: &Node, v: &Node, dst: Reg) -> CResult<()> {
        match target {
            Node::Const(n) => {
                self.expr(v, dst)?;
                let s = self.sym(n);
                self.emit(Op::SetConst(dst, s));
            }
            Node::Colon2(..) | Node::Colon3(_) => {
                let a = self.alloc();
                self.expr(v, a)?;
                self.scoped_const_store(target, a)?;
                self.emit(Op::Move(dst, a));
                self.free_to(a);
            }
            _ => return Err("bad constant assignment".into()),
        }
        Ok(())
    }

    /// Stores R[src] into `Base::Name` / `::Name`.
    fn scoped_const_store(&mut self, target: &Node, src: Reg) -> CResult<()> {
        let a = self.alloc();
        self.emit(Op::Move(a, src));
        let b = self.alloc();
        let name = match target {
            Node::Colon2(base, n) => {
                self.expr(base, b)?;
                n
            }
            Node::Colon3(n) => {
                self.emit(Op::OClass(b));
                n
            }
            _ => unreachable!(),
        };
        let s = self.sym(name);
        self.emit(Op::SetMCnst(a, s));
        self.free_to(a);
        Ok(())
    }

    fn masgn(&mut self, lhs: &MLhs, rhs: &Node, dst: Reg) -> CResult<()> {
        let v = self.alloc();
        let t = self.alloc();
        match rhs {
            Node::Array(elems) => {
                // several values: the expression's value is the array itself
                self.array(elems, v)?;
                self.emit(Op::Move(t, v));
            }
            _ => {
                self.expr(rhs, v)?;
                self.emit(Op::ToAry(t, v));
            }
        }
        self.destructure(lhs, t)?;
        self.emit(Op::Move(dst, v));
        self.free_to(v);
        Ok(())
    }

    /// Assigns the elements of the Array in R[src] to the targets of `m`.
    fn destructure(&mut self, m: &MLhs, src: Reg) -> CResult<()> {
        let r = self.alloc();
        for (i, target) in m.pre.iter().enumerate() {
            self.emit(Op::ARef(r, src, i));
            self.assign_target(target, r)?;
        }
        if let Some(splat) = &m.splat {
            self.emit(Op::ASplat(r, src, m.pre.len(), m.post.len()));
            if let Some(target) = splat {
                self.assign_target(target, r)?;
            }
        }
        for (j, target) in m.post.iter().enumerate() {
            self.emit(Op::APost(r, src, m.pre.len(), m.post.len(), j));
            self.assign_target(target, r)?;
        }
        self.free_to(r);
        Ok(())
    }

    fn assign_target(&mut self, target: &MTarget, src: Reg) -> CResult<()> {
        let node = match target {
            MTarget::Nested(m) => {
                let t = self.alloc();
                self.emit(Op::ToAry(t, src));
                self.destructure(m, t)?;
                self.free_to(t);
                return Ok(());
            }
            MTarget::Var(n) => n,
        };
        match node {
            Node::LVar(n) => self.set_local(n, src)?,
            Node::IVar(n) => {
                let s = self.sym(n);
                self.emit(Op::SetIV(src, s));
            }
            Node::GVar(n) => {
                let s = self.sym(n);
                self.emit(Op::SetGV(src, s));
            }
            Node::CVar(n) => {
                let s = self.sym(n);
                self.emit(Op::SetCV(src, s));
            }
            Node::Const(n) => {
                let s = self.sym(n);
                self.emit(Op::SetConst(src, s));
            }
            Node::Colon2(..) | Node::Colon3(_) => self.scoped_const_store(node, src)?,
            Node::Call {
                recv: Some(recv),
                name,
                args,
                safe,
                ..
            } => {
                // recv.attr = v  /  recv[args] = v
                let a = self.alloc();
                self.expr(recv, a)?;
                let skip = if *safe {
                    Some(self.emit(Op::JmpNil(a, 0)))
                } else {
                    None
                };
                for arg in args {
                    let r = self.alloc();
                    self.expr(arg, r)?;
                }
                let r = self.alloc();
                self.emit(Op::Move(r, src));
                let setter = if name == "[]" {
                    "[]=".to_string()
                } else {
                    format!("{name}=")
                };
                let mid = self.sym(&setter);
                self.emit(Op::Send(a, mid, args.len() + 1, false));
                if let Some(j) = skip {
                    self.patch(j);
                }
                self.free_to(a);
            }
            _ => return Err("unsupported assignment target".into()),
        }
        Ok(())
    }

    // ---- control flow --------------------------------------------------

    fn while_loop(
        &mut self,
        cond: &Node,
        body: &Node,
        until: bool,
        do_while: bool,
        dst: Reg,
    ) -> CResult<()> {
        let t = self.alloc();
        let ens_base = self.cur().ensures.len();
        self.cur().loops.push(LoopCtx {
            dst,
            body_start: 0,
            breaks: vec![],
            nexts: vec![],
            ens_base,
        });
        let (start, cond_pc, jexit);
        if do_while {
            start = self.pc();
            self.cur().loops.last_mut().unwrap().body_start = start;
            self.stmt(body)?;
            cond_pc = self.pc();
            self.expr(cond, t)?;
            self.emit(if until {
                Op::JmpNot(t, start)
            } else {
                Op::JmpIf(t, start)
            });
            jexit = None;
        } else {
            start = self.pc();
            cond_pc = start;
            jexit = Some(self.cond_jump(cond, t, until)?);
            let body_start = self.pc();
            self.cur().loops.last_mut().unwrap().body_start = body_start;
            self.stmt(body)?;
            self.emit(Op::Jmp(start));
        }
        let lp = self.cur().loops.pop().unwrap();
        for j in jexit.into_iter().flatten() {
            self.patch(j);
        }
        self.emit(Op::LoadNil(dst));
        for j in lp.breaks {
            self.patch(j);
        }
        for j in lp.nexts {
            self.patch_to(j, cond_pc);
        }
        self.free_to(t);
        Ok(())
    }

    /// Compiles the ensure clauses entered after `base` (innermost first),
    /// for a `return`/`break`/`next` that leaves them.
    fn inline_ensures(&mut self, base: usize, _dst: Reg) -> CResult<()> {
        let pending: Vec<Node> = self.cur().ensures[base..].to_vec();
        if pending.is_empty() {
            return Ok(());
        }
        let saved = std::mem::take(&mut self.cur().ensures);
        let mut result = Ok(());
        for (i, ens) in pending.iter().enumerate().rev() {
            // an ensure body runs outside its own ensure
            self.cur().ensures = saved[..base + i].to_vec();
            let t = self.alloc();
            result = self.expr(ens, t);
            self.free_to(t);
            if result.is_err() {
                break;
            }
        }
        self.cur().ensures = saved;
        result
    }

    // ---- nested ireps --------------------------------------------------

    fn arg_spec(&mut self, params: &Params) -> ArgSpec {
        ArgSpec {
            req: params.required.len(),
            opt: params.optional.len(),
            rest: params.rest.is_some(),
            post: params.post.len(),
            kw: params
                .kw
                .iter()
                .map(|(n, d)| (self.syms.intern(n), d.is_none()))
                .collect(),
            kwrest: params.kwrest.is_some(),
            nokw: params.kwrest.as_deref() == Some("%nokw"),
            block: params.block.is_some(),
            auto_splat: params.trailing_comma,
        }
    }

    /// Default values and destructuring for parameters, at the start of a body.
    fn prologue(&mut self, params: &Params) -> CResult<()> {
        for (i, (pname, default)) in params.optional.iter().enumerate() {
            let reg = self.lookup(pname).unwrap().1;
            let skip = self.emit(Op::OptArg(i, 0));
            self.expr(default, reg)?;
            self.patch(skip);
        }
        for (i, (pname, default)) in params.kw.iter().enumerate() {
            if let Some(default) = default {
                let reg = self.lookup(pname).unwrap().1;
                let skip = self.emit(Op::KwArg(i, 0));
                self.expr(default, reg)?;
                self.patch(skip);
            }
        }
        // after the defaults: `def m((a, b), c = b)` gives c nil, as in Ruby
        for p in params.required.iter().chain(params.post.iter()) {
            if let ParamTarget::Destructure(hidden, m) = p {
                let reg = self.lookup(hidden).unwrap().1;
                let t = self.alloc();
                self.emit(Op::ToAry(t, reg));
                self.destructure(m, t)?;
                self.free_to(t);
            }
        }
        Ok(())
    }

    fn method(
        &mut self,
        name: &str,
        params: &Params,
        body: &Node,
        locals: &[String],
    ) -> CResult<usize> {
        self.push_scope(Kind::Method, name, locals, Some(params.clone()));
        let spec = self.arg_spec(params);
        self.cur().irep.spec = spec;
        self.prologue(params)?;
        let r = self.alloc();
        self.expr(body, r)?;
        self.emit(Op::Return(r));
        let irep = self.pop_scope();
        Ok(self.add_child(irep))
    }

    fn block(&mut self, blk: &Block) -> CResult<usize> {
        self.push_scope(Kind::Block, "block", &blk.locals, None);
        let spec = self.arg_spec(&blk.params);
        self.cur().irep.spec = spec;
        self.prologue(&blk.params)?;
        let r = self.alloc();
        let body: &[Node] = match &blk.body {
            Node::Seq(v) if blk.for_loop => {
                self.expr(&v[0], r)?; // `for` variable assignment
                &v[1..]
            }
            b => std::slice::from_ref(b),
        };
        let body_start = self.pc();
        self.cur().redo = Some(body_start);
        if body.is_empty() {
            self.emit(Op::LoadNil(r));
        }
        for n in body {
            self.expr(n, r)?;
        }
        self.emit(Op::Return(r));
        let irep = self.pop_scope();
        Ok(self.add_child(irep))
    }

    fn class_body(&mut self, name: &str, body: &Node, locals: &[String]) -> CResult<usize> {
        self.push_scope(Kind::Class, name, locals, None);
        let r = self.alloc();
        self.expr(body, r)?;
        self.emit(Op::Return(r));
        let irep = self.pop_scope();
        Ok(self.add_child(irep))
    }

    // ---- exceptions ----------------------------------------------------

    fn begin(
        &mut self,
        body: &Node,
        rescues: &[Rescue],
        else_body: Option<&Node>,
        ensure: Option<&Node>,
        dst: Reg,
    ) -> CResult<()> {
        let Some(ens) = ensure else {
            return self.rescue(body, rescues, else_body, dst);
        };
        let start = self.pc();
        self.cur().ensures.push(ens.clone());
        let r = self.rescue(body, rescues, else_body, dst);
        self.cur().ensures.pop();
        r?;
        let end = self.pc();
        let t = self.alloc();
        self.expr(ens, t)?;
        let jdone = self.emit(Op::Jmp(0));
        // exceptional path: run the ensure clause, then re-raise
        let target = self.pc();
        self.cur().irep.catch.push(CatchHandler {
            start,
            end,
            target,
            ensure: true,
        });
        self.emit(Op::Except(t));
        let t2 = self.alloc();
        self.expr(ens, t2)?;
        self.emit(Op::Raise(t));
        self.free_to(t);
        self.patch(jdone);
        Ok(())
    }

    fn rescue(
        &mut self,
        body: &Node,
        rescues: &[Rescue],
        else_body: Option<&Node>,
        dst: Reg,
    ) -> CResult<()> {
        if rescues.is_empty() {
            self.expr(body, dst)?;
            if let Some(e) = else_body {
                self.expr(e, dst)?;
            }
            return Ok(());
        }
        let start = self.pc();
        self.expr(body, dst)?;
        let end = self.pc();
        if let Some(e) = else_body {
            self.expr(e, dst)?;
        }
        let mut done = vec![self.emit(Op::Jmp(0))];
        let target = self.pc();
        self.cur().irep.catch.push(CatchHandler {
            start,
            end,
            target,
            ensure: false,
        });
        let exc = self.alloc();
        self.emit(Op::Except(exc));
        let standard_error = [Node::Const("StandardError".into())];
        self.cur().retry.push(start);
        for r in rescues {
            let classes = if r.classes.is_empty() {
                &standard_error[..]
            } else {
                &r.classes[..]
            };
            let mut matched = Vec::new();
            for class in classes {
                let c = self.alloc();
                match class {
                    Node::Splat(list) => self.expr(list, c)?,
                    _ => self.expr(class, c)?,
                }
                self.emit(Op::Rescue(exc, c));
                matched.push(self.emit(Op::JmpIf(c, 0)));
                self.free_to(c);
            }
            let next = self.emit(Op::Jmp(0));
            for j in matched {
                self.patch(j);
            }
            if let Some(var) = &r.var {
                self.assign_target(&MTarget::Var((**var).clone()), exc)?;
            }
            self.expr(&r.body, dst)?;
            done.push(self.emit(Op::Jmp(0)));
            self.patch(next);
        }
        self.cur().retry.pop();
        // nothing matched: propagate
        self.emit(Op::Raise(exc));
        self.free_to(exc);
        for j in done {
            self.patch(j);
        }
        Ok(())
    }

    // ---- defined? ------------------------------------------------------

    /// `defined?(e)`: a description string, or nil. Sub-expressions that must
    /// be evaluated (receivers, constant path roots) are guarded, so an
    /// exception there makes the answer nil instead of propagating.
    fn defined(&mut self, e: &Node, dst: Reg) -> CResult<()> {
        let mut fails = Vec::new();
        self.defined_into(e, dst, &mut fails)?;
        if !fails.is_empty() {
            let done = self.emit(Op::Jmp(0));
            for j in fails {
                self.patch(j);
            }
            self.emit(Op::LoadNil(dst));
            self.patch(done);
        }
        Ok(())
    }

    /// Like `defined`, but jumps recorded in `fails` mean "not defined".
    fn defined_into(&mut self, e: &Node, dst: Reg, fails: &mut Vec<usize>) -> CResult<()> {
        match e {
            Node::LVar(_) => self.defined_str(dst, "local-variable"),
            Node::IVar(n) | Node::GVar(n) | Node::CVar(n) | Node::Const(n) => {
                let kind = match e {
                    Node::IVar(_) => DefinedKind::IVar,
                    Node::GVar(_) => DefinedKind::GVar,
                    Node::CVar(_) => DefinedKind::CVar,
                    _ => DefinedKind::Const,
                };
                let s = self.sym(n);
                self.emit(Op::Defined(dst, kind, s));
            }
            Node::Colon2(base, n) => {
                self.require_defined(base, dst, fails)?;
                let a = self.alloc();
                self.guarded_expr(base, a, fails)?;
                let s = self.sym(n);
                self.emit(Op::Defined(dst, DefinedKind::ScopedConst(a), s));
                self.free_to(a);
            }
            Node::Colon3(n) => {
                let a = self.alloc();
                self.emit(Op::OClass(a));
                let s = self.sym(n);
                self.emit(Op::Defined(dst, DefinedKind::ScopedConst(a), s));
                self.free_to(a);
            }
            Node::Call { block: Some(_), .. } => self.defined_str(dst, "expression"),
            Node::Call {
                recv,
                name,
                args,
                block_arg,
                ..
            } => {
                for arg in args.iter().chain(block_arg.as_deref()) {
                    self.require_defined(arg, dst, fails)?;
                }
                let a = self.alloc();
                let fcall = match recv {
                    Some(r) => {
                        self.require_defined(r, dst, fails)?;
                        self.guarded_expr(r, a, fails)?;
                        false
                    }
                    None => {
                        self.emit(Op::LoadSelf(a));
                        true
                    }
                };
                let s = self.sym(name);
                self.emit(Op::Defined(dst, DefinedKind::Method(a, fcall), s));
                self.free_to(a);
            }
            Node::Array(elems) => {
                for el in elems {
                    self.require_defined(el, dst, fails)?;
                }
                self.defined_str(dst, "expression");
            }
            Node::Hash(items, _) => {
                for item in items {
                    match item {
                        HashItem::Pair(k, v) => {
                            self.require_defined(k, dst, fails)?;
                            self.require_defined(v, dst, fails)?;
                        }
                        HashItem::Splat(h) => self.require_defined(h, dst, fails)?,
                    }
                }
                self.defined_str(dst, "expression");
            }
            Node::Splat(inner) => self.defined_into(inner, dst, fails)?,
            // a bare `begin ... end` around one expression is transparent
            Node::Begin {
                body,
                rescues,
                else_body: None,
                ensure: None,
            } if rescues.is_empty() => self.defined_into(body, dst, fails)?,
            Node::Yield(_) => {
                self.emit(Op::BlockGiven(dst));
                fails.push(self.emit(Op::JmpNot(dst, 0)));
                self.defined_str(dst, "yield");
            }
            Node::Super { .. } => {
                let s = self.sym("super");
                self.emit(Op::Defined(dst, DefinedKind::Super, s));
            }
            Node::SelfRef => self.defined_str(dst, "self"),
            Node::Nil => self.defined_str(dst, "nil"),
            Node::True => self.defined_str(dst, "true"),
            Node::False => self.defined_str(dst, "false"),
            Node::LAsgn(..)
            | Node::IAsgn(..)
            | Node::GAsgn(..)
            | Node::CVAsgn(..)
            | Node::CAsgn(..)
            | Node::MAsgn(..) => self.defined_str(dst, "assignment"),
            // `a ||= v` / `a &&= v` desugar to Or/And around an assignment
            Node::Or(_, r) | Node::And(_, r)
                if matches!(
                    **r,
                    Node::LAsgn(..)
                        | Node::IAsgn(..)
                        | Node::GAsgn(..)
                        | Node::CVAsgn(..)
                        | Node::CAsgn(..)
                ) =>
            {
                self.defined_str(dst, "assignment")
            }
            Node::BlockGiven | Node::CurrentMethod => self.defined_str(dst, "method"),
            _ => self.defined_str(dst, "expression"),
        }
        Ok(())
    }

    /// Adds a "not defined" exit when `e` (an operand of a call/literal) isn't defined.
    fn require_defined(&mut self, e: &Node, dst: Reg, fails: &mut Vec<usize>) -> CResult<()> {
        let e = match e {
            Node::Splat(x) => &**x,
            x => x,
        };
        if matches!(
            e,
            Node::Int(_)
                | Node::Float(_)
                | Node::Str(_)
                | Node::Sym(_)
                | Node::Nil
                | Node::True
                | Node::False
                | Node::SelfRef
        ) {
            return Ok(());
        }
        self.defined_into(e, dst, fails)?;
        fails.push(self.emit(Op::JmpNot(dst, 0)));
        Ok(())
    }

    /// Evaluates `e` into `dst`; an exception jumps to the "not defined" exit.
    fn guarded_expr(&mut self, e: &Node, dst: Reg, fails: &mut Vec<usize>) -> CResult<()> {
        let start = self.pc();
        self.expr(e, dst)?;
        let end = self.pc();
        let ok = self.emit(Op::Jmp(0));
        let target = self.pc();
        self.cur().irep.catch.push(CatchHandler {
            start,
            end,
            target,
            ensure: false,
        });
        self.emit(Op::Except(dst));
        fails.push(self.emit(Op::Jmp(0)));
        self.patch(ok);
        Ok(())
    }
}

/// How many frames above its own `irep`'s code reaches through upvars
/// (GetUpVar/SetUpVar depth + 1; through a nested block, one less).
fn reach(irep: &Irep) -> usize {
    let own = irep
        .code
        .iter()
        .filter_map(|op| match op {
            Op::GetUpVar(_, _, d) | Op::SetUpVar(_, _, d) => Some(d + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let nested = irep.reps.iter().map(|r| reach(r).saturating_sub(1)).max().unwrap_or(0);
    own.max(nested)
}

/// Whether the code (or a block in it) sets a default visibility, which a
/// block shares with the scope it was written in.
fn sets_vis(irep: &Irep) -> bool {
    irep.code.iter().any(|op| matches!(op, Op::DefaultVis(_))) || irep.reps.iter().any(|r| sets_vis(r))
}

/// Rewrites that keep every instruction at its place (so no jump target
/// moves), for code without exception handlers:
/// - a `Jmp` goes straight to the end of a chain of jumps, and a jump to a
///   `Return` is that `Return` (`c ? x : y` as a method's last expression);
/// - `Move(a, b); Return(a)` returns `b` directly.
fn peephole(code: &mut [Op]) {
    for i in 0..code.len() {
        if let Op::Jmp(mut t) = code[i] {
            let mut hops = 0;
            while let Some(Op::Jmp(next)) = code.get(t) {
                if hops == code.len() {
                    break;
                }
                t = *next;
                hops += 1;
            }
            code[i] = match code.get(t) {
                Some(Op::Return(r)) => Op::Return(*r),
                _ => Op::Jmp(t),
            };
        }
    }
    for i in 0..code.len().saturating_sub(1) {
        if let (Op::Move(a, b), Op::Return(r)) = (code[i], code[i + 1])
            && a == r
        {
            code[i] = Op::Return(b);
        }
    }
}

