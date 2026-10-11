//! Recursive-descent parser producing [`crate::ast`].
//!
//! mruby uses Prism for this; ruruby stays pure Rust (it must build for
//! wasm). Like Prism, the parser resolves local variables: it keeps a scope
//! stack so `foo` can be told apart as a local variable read or a method
//! call, and records each scope's locals table (parameters first).

mod pattern;

use crate::ast::*;
use crate::lexer::{Kw, StrPart, Tok, Token, lex, lex_at};

type PResult<T> = Result<T, String>;

/// Call arguments and the `&block` argument.
type ArgList = (Vec<Node>, Option<Box<Node>>);

struct Scope {
    locals: Vec<String>,
    /// `def`/`class`/top-level scopes hide outer locals; blocks don't.
    hard: bool,
    /// Block without explicit parameters, so `_1`..`_9` / `it` may be used.
    implicit_params: bool,
    numbered: usize,
    it: bool,
}

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    scopes: Vec<Scope>,
    /// Set while parsing command args / `while` conditions so `do` binds to the outer construct.
    no_do: bool,
    /// Set while parsing a `for` header so `in` isn't read as a pattern test.
    no_in: bool,
    tmp: usize,
    file: String,
    /// In a `case/in`: (subject var, cached deconstruct var, "cached" flag var),
    /// so every clause shares one `deconstruct` call.
    pm_cache: Option<(String, String, String)>,
}

pub fn parse(src: &str) -> PResult<Program> {
    parse_file(src, "-", 1)
}

/// Parses source from `file`, starting at `line` (for `__FILE__`, `__LINE__` and errors).
pub fn parse_file(src: &str, file: &str, line: u32) -> PResult<Program> {
    let mut p = Parser {
        file: file.to_string(),
        pm_cache: None,
        toks: lex_at(src, line)?,
        pos: 0,
        scopes: vec![Scope::new(true)],
        no_do: false,
        no_in: false,
        tmp: 0,
    };
    let body = p.parse_stmts()?;
    if !p.at(&Tok::Eof) {
        return Err(p.err("unexpected token"));
    }
    let locals = p.scopes.pop().unwrap().locals;
    Ok(Program { body, locals })
}

impl Scope {
    fn new(hard: bool) -> Self {
        Scope {
            locals: vec![],
            hard,
            implicit_params: false,
            numbered: 0,
            it: false,
        }
    }
}

fn b(n: Node) -> Box<Node> {
    Box::new(n)
}

fn is_ident_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_lowercase() || c == '_' || (c as u32) > 0x7f)
        && !name.ends_with(['?', '!'])
}

/// Can `n` be assigned to (`n = v`)?
fn is_assignable(n: &Node) -> bool {
    match n {
        Node::LVar(_)
        | Node::IVar(_)
        | Node::GVar(_)
        | Node::CVar(_)
        | Node::Const(_)
        | Node::Colon2(..)
        | Node::Colon3(_) => true,
        Node::Call {
            recv: None,
            name,
            args,
            block: None,
            block_arg: None,
            ..
        } => args.is_empty() && is_ident_name(name),
        Node::Call {
            recv: Some(_),
            name,
            args,
            block: None,
            block_arg: None,
            ..
        } => {
            name == "[]"
                || (args.is_empty()
                    && (is_ident_name(name) || name.starts_with(char::is_uppercase)))
        }
        _ => false,
    }
}

fn make_assign(lhs: Node, rhs: Node) -> Node {
    match lhs {
        Node::LVar(n)
        | Node::Call {
            recv: None,
            name: n,
            ..
        } => Node::LAsgn(n, b(rhs)),
        Node::IVar(n) => Node::IAsgn(n, b(rhs)),
        Node::GVar(n) => Node::GAsgn(n, b(rhs)),
        Node::CVar(n) => Node::CVAsgn(n, b(rhs)),
        c @ (Node::Const(_) | Node::Colon2(..) | Node::Colon3(_)) => Node::CAsgn(b(c), b(rhs)),
        Node::Call {
            recv: Some(r),
            name,
            mut args,
            safe,
            ..
        } if name == "[]" => {
            args.push(rhs);
            Node::Call {
                recv: Some(r),
                name: "[]=".into(),
                args,
                block: None,
                block_arg: None,
                safe,
            }
        }
        Node::Call {
            recv: Some(r),
            name,
            safe,
            ..
        } => Node::Call {
            recv: Some(r),
            name: format!("{name}="),
            args: vec![rhs],
            block: None,
            block_arg: None,
            safe,
        },
        _ => unreachable!("checked by is_assignable"),
    }
}

/// The read form of an assignment target (`x` for `x += 1`).
fn read_of(lhs: &Node) -> Node {
    match lhs {
        Node::Call {
            recv: None, name, ..
        } => Node::LVar(name.clone()),
        other => other.clone(),
    }
}

impl Parser {
    // ---- token helpers -------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_n(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn peek2(&self) -> &Tok {
        self.peek_n(1)
    }

    fn spc(&self) -> bool {
        self.toks[self.pos].spc
    }

    fn spc_at(&self, n: usize) -> bool {
        self.toks.get(self.pos + n).is_some_and(|t| t.spc)
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn advance(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn at_kw(&self, k: Kw) -> bool {
        matches!(self.peek(), Tok::Kw(x) if *x == k)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Kw) -> bool {
        if self.at_kw(k) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok) -> PResult<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.err(&format!("expected {t:?}")))
        }
    }

    fn expect_kw(&mut self, k: Kw) -> PResult<()> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", k.as_str())))
        }
    }

    fn skip_nl(&mut self) {
        while self.at(&Tok::Newline) {
            self.advance();
        }
    }

    fn skip_terms(&mut self) {
        while matches!(self.peek(), Tok::Newline | Tok::Semi) {
            self.advance();
        }
    }

    fn eat_then(&mut self) {
        self.skip_terms();
        self.eat_kw(Kw::Then);
        self.skip_terms();
    }

    fn err(&self, msg: &str) -> String {
        format!(
            "syntax error, line {}: {} (found {:?})",
            self.line(),
            msg,
            self.peek()
        )
    }

    fn with_no_do<T>(&mut self, v: bool, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        let old = std::mem::replace(&mut self.no_do, v);
        let r = f(self);
        self.no_do = old;
        r
    }

    /// Consumes the current token, then runs `f`.
    fn with_advance<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.advance();
        f(self)
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.tmp += 1;
        format!("%{prefix}{}", self.tmp)
    }

    // ---- scopes --------------------------------------------------------

    fn push_scope(&mut self, hard: bool) {
        self.scopes.push(Scope::new(hard));
    }

    fn pop_scope(&mut self) -> Vec<String> {
        self.scopes.pop().unwrap().locals
    }

    fn is_local(&self, name: &str) -> bool {
        for s in self.scopes.iter().rev() {
            if s.locals.iter().any(|l| l == name) {
                return true;
            }
            if s.hard {
                break;
            }
        }
        false
    }

    fn declare(&mut self, name: &str) {
        if !self.is_local(name) {
            self.declare_here(name);
        }
    }

    /// Declares in the innermost scope even if an outer one has the name (parameters).
    fn declare_here(&mut self, name: &str) {
        let s = self.scopes.last_mut().unwrap();
        if !s.locals.iter().any(|l| l == name) {
            s.locals.push(name.to_string());
        }
    }

    /// Puts parameters first in the current scope's locals, in register order.
    fn reorder_locals(&mut self, params: &[String]) {
        let s = self.scopes.last_mut().unwrap();
        let mut locals: Vec<String> = params.to_vec();
        for l in s.locals.drain(..) {
            if !locals.contains(&l) {
                locals.push(l);
            }
        }
        s.locals = locals;
    }

    // ---- statements ----------------------------------------------------

    fn at_stmts_end(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Eof
                | Tok::RBrace
                | Tok::RParen
                | Tok::Kw(
                    Kw::End | Kw::Else | Kw::Elsif | Kw::When | Kw::In | Kw::Rescue | Kw::Ensure
                )
        )
    }

    fn parse_stmts(&mut self) -> PResult<Node> {
        let mut v = Vec::new();
        self.skip_terms();
        while !self.at_stmts_end() {
            v.push(self.parse_stmt()?);
            if !self.at_stmts_end() && !matches!(self.peek(), Tok::Newline | Tok::Semi) {
                return Err(self.err("unexpected token"));
            }
            self.skip_terms();
        }
        Ok(match v.len() {
            0 => Node::Nil,
            1 => v.pop().unwrap(),
            _ => Node::Seq(v),
        })
    }

    fn parse_stmt(&mut self) -> PResult<Node> {
        let mut node = match self.try_masgn()? {
            Some(n) => n,
            None => self.parse_expr()?,
        };
        // `a = 1, 2` assigns an array
        if self.at(&Tok::Comma) && is_single_assign(&node) {
            let mut items = vec![assign_value(&node)];
            while self.eat(&Tok::Comma) {
                self.skip_nl();
                items.push(self.parse_arg_or_splat()?);
            }
            node = replace_assign_value(node, Node::Array(items));
        }
        loop {
            node = if self.eat_kw(Kw::If) {
                Node::If(b(self.parse_expr()?), b(node), b(Node::Nil))
            } else if self.eat_kw(Kw::Unless) {
                Node::If(b(self.parse_expr()?), b(Node::Nil), b(node))
            } else if self.at_kw(Kw::While) || self.at_kw(Kw::Until) {
                let until = self.advance() == Tok::Kw(Kw::Until);
                let do_while = matches!(node, Node::Begin { .. });
                Node::While {
                    cond: b(self.parse_expr()?),
                    body: b(node),
                    until,
                    do_while,
                }
            } else if self.eat_kw(Kw::Rescue) {
                let fallback = self.parse_expr()?;
                rescue_modifier(node, fallback)
            } else {
                return Ok(node);
            };
        }
    }

    /// Recognizes `a, b = ...`, `*a = ...`, `(a, b), c = ...` by a quick scan,
    /// then parses the left-hand side speculatively.
    fn try_masgn(&mut self) -> PResult<Option<Node>> {
        if !self.looks_like_masgn() {
            return Ok(None);
        }
        let start = self.pos;
        let saved_tmp = self.tmp;
        let lhs = match self.parse_mlhs(&[Tok::Assign]) {
            Ok(l) if self.at(&Tok::Assign) => l,
            _ => {
                self.pos = start;
                self.tmp = saved_tmp;
                return Ok(None);
            }
        };
        self.advance();
        self.skip_nl();
        self.declare_mlhs(&lhs);
        let mut items = vec![self.parse_arg_or_splat()?];
        while self.eat(&Tok::Comma) {
            self.skip_nl();
            items.push(self.parse_arg_or_splat()?);
        }
        let rhs = if items.len() == 1 && !matches!(items[0], Node::Splat(_)) {
            items.pop().unwrap()
        } else {
            Node::Array(items)
        };
        let rhs = if self.eat_kw(Kw::Rescue) {
            rescue_modifier(rhs, self.parse_expr()?)
        } else {
            rhs
        };
        Ok(Some(Node::MAsgn(lhs, b(rhs))))
    }

    fn looks_like_masgn(&self) -> bool {
        if matches!(self.peek(), Tok::Star) {
            return true;
        }
        let mut depth = 0i32;
        let mut comma = false;
        let mut i = self.pos;
        loop {
            match &self.toks[i].tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                Tok::Comma if depth == 0 => comma = true,
                Tok::Assign if depth == 0 => return comma,
                Tok::Newline if depth == 0 && i > 0 && self.toks[i - 1].tok == Tok::Comma => {}
                Tok::Newline | Tok::Semi | Tok::Eof if depth == 0 => return false,
                Tok::Eof => return false,
                Tok::Kw(
                    Kw::Do | Kw::If | Kw::Unless | Kw::While | Kw::Until | Kw::And | Kw::Or,
                ) if depth == 0 => {
                    return false;
                }
                Tok::OpAssign(_) | Tok::Arrow | Tok::Pipe if depth == 0 => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// Parses multiple-assignment targets until one of `stop` (not consumed).
    fn parse_mlhs(&mut self, stop: &[Tok]) -> PResult<MLhs> {
        let mut m = MLhs::default();
        loop {
            if stop.iter().any(|t| self.at(t)) {
                break;
            }
            if self.eat(&Tok::Star) {
                if m.splat.is_some() {
                    return Err(self.err("multiple splats in assignment"));
                }
                let target = if matches!(self.peek(), Tok::Comma | Tok::RParen)
                    || stop.iter().any(|t| self.at(t))
                {
                    None
                } else {
                    Some(Box::new(self.parse_mtarget()?))
                };
                m.splat = Some(target);
            } else {
                let t = self.parse_mtarget()?;
                if m.splat.is_some() {
                    m.post.push(t)
                } else {
                    m.pre.push(t)
                }
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        Ok(m)
    }

    fn parse_mtarget(&mut self) -> PResult<MTarget> {
        if self.at(&Tok::LParen) {
            self.advance();
            let inner = self.parse_mlhs(&[Tok::RParen])?;
            self.expect(&Tok::RParen)?;
            return Ok(MTarget::Nested(inner));
        }
        let node = self.parse_primary_postfix_no_call_args()?;
        if !is_assignable(&node) {
            return Err(self.err("can't assign to this"));
        }
        Ok(MTarget::Var(read_of(&node)))
    }

    /// A primary with postfix `.attr`, `[i]`, `::C`, but no command arguments.
    fn parse_primary_postfix_no_call_args(&mut self) -> PResult<Node> {
        let node = match self.peek().clone() {
            Tok::Ident(name) if !self.is_local(&name) => {
                self.advance();
                ast::call(None, &name, vec![])
            }
            _ => self.parse_primary()?,
        };
        let mut node = node;
        loop {
            match self.peek() {
                Tok::Dot | Tok::AndDot => {
                    let safe = self.advance() == Tok::AndDot;
                    let name = match self.advance() {
                        Tok::Ident(s) | Tok::Const(s) => s,
                        _ => return Err(self.err("expected attribute name")),
                    };
                    node = Node::Call {
                        recv: Some(b(node)),
                        name,
                        args: vec![],
                        block: None,
                        block_arg: None,
                        safe,
                    };
                }
                Tok::LBracket if !self.spc() => {
                    self.advance();
                    let (args, _) =
                        self.with_no_do(false, |p| p.parse_paren_args(Tok::RBracket))?;
                    node = ast::call(Some(node), "[]", args);
                }
                Tok::Colon2 => {
                    self.advance();
                    let Tok::Const(c) = self.advance() else {
                        return Err(self.err("expected constant"));
                    };
                    node = Node::Colon2(b(node), c);
                }
                _ => return Ok(node),
            }
        }
    }

    fn declare_mlhs(&mut self, m: &MLhs) {
        let mut targets: Vec<&MTarget> = m.pre.iter().chain(m.post.iter()).collect();
        if let Some(Some(t)) = &m.splat {
            targets.push(t);
        }
        for t in targets {
            match t {
                MTarget::Var(Node::LVar(n)) => {
                    let n = n.clone();
                    self.declare(&n)
                }
                MTarget::Nested(inner) => self.declare_mlhs(inner),
                _ => {}
            }
        }
    }

    // ---- expressions (lowest precedence first) -------------------------

    fn parse_expr(&mut self) -> PResult<Node> {
        let mut l = self.parse_not()?;
        loop {
            if self.eat_kw(Kw::And) {
                self.skip_nl();
                l = Node::And(b(l), b(self.parse_not()?));
            } else if self.eat_kw(Kw::Or) {
                self.skip_nl();
                l = Node::Or(b(l), b(self.parse_not()?));
            } else {
                return Ok(l);
            }
        }
    }

    fn parse_not(&mut self) -> PResult<Node> {
        if self.eat_kw(Kw::Not) {
            return Ok(Node::Not(b(self.parse_not()?)));
        }
        let v = self.parse_arg()?;
        // one-line pattern matching: `expr => pattern`, `expr in pattern`
        if self.at(&Tok::Arrow) {
            self.advance();
            return self.rightward_pattern(v, true);
        }
        if self.at_kw(Kw::In) && !self.no_in {
            self.advance();
            return self.rightward_pattern(v, false);
        }
        Ok(v)
    }

    fn parse_arg_or_splat(&mut self) -> PResult<Node> {
        if self.eat(&Tok::Star) {
            return Ok(Node::Splat(b(self.parse_arg()?)));
        }
        self.parse_arg()
    }

    /// Assignment level (`a = b`, `a += b`, `a[i] = v`, `o.x = v`).
    fn parse_arg(&mut self) -> PResult<Node> {
        let lhs = self.parse_ternary()?;
        if !is_assignable(&lhs) {
            return Ok(lhs);
        }
        match self.peek().clone() {
            Tok::Assign => {
                self.advance();
                self.skip_nl();
                self.declare_lhs(&lhs);
                let rhs = if self.at(&Tok::Star) {
                    Node::Array(vec![self.parse_arg_or_splat()?])
                } else {
                    self.parse_arg()?
                };
                Ok(make_assign(lhs, rhs))
            }
            Tok::OpAssign(op) => {
                self.advance();
                self.skip_nl();
                self.declare_lhs(&lhs);
                let rhs = self.parse_arg()?;
                let cur = read_of(&lhs);
                Ok(match op.as_str() {
                    // `a ||= v` is `a || a = v`
                    "||" => Node::Or(b(cur), b(make_assign(lhs, rhs))),
                    "&&" => Node::And(b(cur), b(make_assign(lhs, rhs))),
                    _ => make_assign(lhs, ast::call(Some(cur), &op, vec![rhs])),
                })
            }
            _ => Ok(lhs),
        }
    }

    fn declare_lhs(&mut self, lhs: &Node) {
        if let Node::Call {
            recv: None, name, ..
        } = lhs
        {
            self.declare(name);
        }
    }

    fn parse_ternary(&mut self) -> PResult<Node> {
        let c = self.parse_range()?;
        if !self.eat(&Tok::Question) {
            return Ok(c);
        }
        self.skip_nl();
        let t = self.parse_ternary()?;
        self.skip_nl();
        // `a ? b :c` lexes `:c` as a symbol; it's really the identifier `c`
        if let Tok::Sym(s) = self.peek().clone()
            && (!self.spc_at(1) || is_ident_name(&s))
        {
            self.advance();
            let e = if self.is_local(&s) {
                Node::LVar(s)
            } else {
                ast::call(None, &s, vec![])
            };
            let e = self.parse_postfix(e)?;
            return Ok(Node::If(b(c), b(t), b(e)));
        }
        self.expect(&Tok::Colon)?;
        self.skip_nl();
        let e = self.parse_ternary()?;
        Ok(Node::If(b(c), b(t), b(e)))
    }

    fn parse_range(&mut self) -> PResult<Node> {
        // beginless range: `..10`
        if matches!(self.peek(), Tok::Dot2 | Tok::Dot3) {
            let excl = self.advance() == Tok::Dot3;
            let hi = self.parse_oror()?;
            return Ok(Node::Range(b(Node::Nil), b(hi), excl));
        }
        let l = self.parse_oror()?;
        let excl = match self.peek() {
            Tok::Dot2 => false,
            Tok::Dot3 => true,
            _ => return Ok(l),
        };
        self.advance();
        // endless range: `a..`
        let r = if self.starts_value() && !matches!(self.peek(), Tok::Comma | Tok::Kw(Kw::Then)) {
            self.parse_oror()?
        } else {
            Node::Nil
        };
        Ok(Node::Range(b(l), b(r), excl))
    }

    fn parse_oror(&mut self) -> PResult<Node> {
        let mut l = self.parse_andand()?;
        while self.eat(&Tok::OrOr) {
            self.skip_nl();
            l = Node::Or(b(l), b(self.parse_andand()?));
        }
        Ok(l)
    }

    fn parse_andand(&mut self) -> PResult<Node> {
        let mut l = self.parse_notop()?;
        while self.eat(&Tok::AndAnd) {
            self.skip_nl();
            l = Node::And(b(l), b(self.parse_notop()?));
        }
        Ok(l)
    }

    /// `not` binds looser than `==` but `defined?` lives here too.
    fn parse_notop(&mut self) -> PResult<Node> {
        if self.at_kw(Kw::Defined) {
            self.advance();
            let e = if self.at(&Tok::LParen) {
                self.advance();
                self.skip_nl();
                let e = self.with_no_do(false, |p| p.parse_expr())?;
                self.skip_nl();
                self.expect(&Tok::RParen)?;
                self.parse_postfix(Node::Defined(b(e)))?
            } else {
                Node::Defined(b(self.parse_eq()?))
            };
            return Ok(e);
        }
        self.parse_eq()
    }

    fn parse_eq(&mut self) -> PResult<Node> {
        let mut l = self.parse_cmp()?;
        loop {
            let name = match self.peek() {
                Tok::EqEq => "==",
                Tok::NotEq => "!=",
                Tok::Eqq => "===",
                Tok::Cmp => "<=>",
                Tok::Match => "=~",
                Tok::NotMatch => "!~",
                _ => return Ok(l),
            };
            self.advance();
            self.skip_nl();
            let r = self.parse_cmp()?;
            l = match name {
                "!=" => ast::call(Some(l), "!=", vec![r]),
                "!~" => Node::Not(b(ast::call(Some(l), "=~", vec![r]))),
                _ => ast::call(Some(l), name, vec![r]),
            };
        }
    }

    fn binary(
        &mut self,
        next: fn(&mut Self) -> PResult<Node>,
        ops: &[(Tok, &str)],
    ) -> PResult<Node> {
        let mut l = next(self)?;
        'outer: loop {
            for (t, name) in ops {
                if self.at(t) {
                    self.advance();
                    self.skip_nl();
                    let r = next(self)?;
                    l = ast::call(Some(l), name, vec![r]);
                    continue 'outer;
                }
            }
            return Ok(l);
        }
    }

    fn parse_cmp(&mut self) -> PResult<Node> {
        self.binary(
            Self::parse_bitor,
            &[
                (Tok::Lt, "<"),
                (Tok::Le, "<="),
                (Tok::Gt, ">"),
                (Tok::Ge, ">="),
            ],
        )
    }

    fn parse_bitor(&mut self) -> PResult<Node> {
        self.binary(Self::parse_bitand, &[(Tok::Pipe, "|"), (Tok::Caret, "^")])
    }

    fn parse_bitand(&mut self) -> PResult<Node> {
        self.binary(Self::parse_shift, &[(Tok::Amp, "&")])
    }

    fn parse_shift(&mut self) -> PResult<Node> {
        self.binary(Self::parse_add, &[(Tok::Shl, "<<"), (Tok::Shr, ">>")])
    }

    fn parse_add(&mut self) -> PResult<Node> {
        self.binary(Self::parse_mul, &[(Tok::Plus, "+"), (Tok::Minus, "-")])
    }

    fn parse_mul(&mut self) -> PResult<Node> {
        self.binary(
            Self::parse_uminus,
            &[(Tok::Star, "*"), (Tok::Slash, "/"), (Tok::Percent, "%")],
        )
    }

    fn parse_uminus(&mut self) -> PResult<Node> {
        if !matches!(self.peek(), Tok::Minus | Tok::Plus) {
            return self.parse_pow();
        }
        let neg = self.advance() == Tok::Minus;
        // `-2` is a literal, but `-2 ** 2` is `-(2 ** 2)`.
        if matches!(self.peek(), Tok::Int(_) | Tok::Float(_))
            && !self.spc()
            && !matches!(self.peek2(), Tok::Pow)
        {
            let lit = match (self.advance(), neg) {
                (Tok::Int(i), true) => Node::Int(-i),
                (Tok::Float(f), true) => Node::Float(-f),
                (Tok::Int(i), false) => Node::Int(i),
                (Tok::Float(f), false) => Node::Float(f),
                _ => unreachable!(),
            };
            return self.parse_postfix(lit);
        }
        let e = self.parse_uminus()?;
        Ok(match (e, neg) {
            (Node::Int(i), true) => Node::Int(-i),
            (Node::Float(f), true) => Node::Float(-f),
            (e, true) => ast::call(Some(e), "-@", vec![]),
            (e, false) => ast::call(Some(e), "+@", vec![]),
        })
    }

    fn parse_pow(&mut self) -> PResult<Node> {
        let base = self.parse_unary()?;
        if self.eat(&Tok::Pow) {
            self.skip_nl();
            let e = self.parse_uminus()?;
            return Ok(ast::call(Some(base), "**", vec![e]));
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> PResult<Node> {
        if self.eat(&Tok::Bang) {
            return Ok(Node::Not(b(self.parse_unary()?)));
        }
        if self.eat(&Tok::Tilde) {
            return Ok(ast::call(Some(self.parse_unary()?), "~", vec![]));
        }
        if self.at(&Tok::Amp) {
            return Err(self.err("unexpected '&'"));
        }
        let p = self.parse_primary()?;
        self.parse_postfix(p)
    }

    /// Method name after `.`: identifiers, keywords and operators.
    fn method_name_after_dot(&mut self) -> PResult<String> {
        let name = match self.peek().clone() {
            Tok::Ident(s) | Tok::Const(s) => s,
            Tok::Kw(k) => k.as_str().to_string(),
            Tok::LParen => return Ok("call".into()), // `f.(x)`
            Tok::LBracket => {
                self.advance();
                self.expect(&Tok::RBracket)?;
                if self.at(&Tok::Assign) && !self.spc() && matches!(self.peek2(), Tok::LParen) {
                    self.advance();
                    return Ok("[]=".into());
                }
                return Ok("[]".into());
            }
            t => match op_method_name(&t) {
                Some(op) => op.to_string(),
                None => return Err(self.err("expected method name after '.'")),
            },
        };
        self.advance();
        Ok(name)
    }

    fn parse_postfix(&mut self, mut node: Node) -> PResult<Node> {
        loop {
            match self.peek() {
                Tok::Dot | Tok::AndDot => {
                    let safe = self.advance() == Tok::AndDot;
                    self.skip_nl();
                    let name = self.method_name_after_dot()?;
                    node = self.parse_call_rest(Some(node), name, safe)?;
                }
                Tok::Colon2 => {
                    self.advance();
                    self.skip_nl();
                    node = match self.advance() {
                        Tok::Const(c) if !(self.at(&Tok::LParen) && !self.spc()) => {
                            Node::Colon2(b(node), c)
                        }
                        Tok::Const(m) | Tok::Ident(m) => {
                            self.parse_call_rest(Some(node), m, false)?
                        }
                        _ => return Err(self.err("expected constant after '::'")),
                    };
                }
                Tok::LBracket if !self.spc() || !matches!(node, Node::Call { .. }) => {
                    self.advance();
                    let (mut args, _) =
                        self.with_no_do(false, |p| p.parse_paren_args(Tok::RBracket))?;
                    if let Some(Node::Hash(_, kw)) = args.last_mut() {
                        *kw = false;
                    }
                    node = ast::call(Some(node), "[]", args);
                }
                _ => return Ok(node),
            }
        }
    }

    // ---- method calls --------------------------------------------------

    fn can_start_command_arg(&self) -> bool {
        if !self.spc() {
            return false;
        }
        // `a -1` is a command call; `a -` at the end of a line continues the expression
        let next_tight =
            !self.spc_at(1) && !matches!(self.peek_n(1), Tok::Newline | Tok::Semi | Tok::Eof);
        match self.peek() {
            Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::Sym(_)
            | Tok::DSym(_)
            | Tok::Words(..)
            | Tok::Regexp(..)
            | Tok::XStr(_)
            | Tok::Label(_)
            | Tok::Ident(_)
            | Tok::Const(_)
            | Tok::IVar(_)
            | Tok::CVar(_)
            | Tok::GVar(_)
            | Tok::LBracket
            | Tok::LParen
            | Tok::Lambda => true,
            Tok::Kw(k) => matches!(
                k,
                Kw::Nil
                    | Kw::True
                    | Kw::False
                    | Kw::SelfKw
                    | Kw::Not
                    | Kw::Def
                    | Kw::Super
                    | Kw::Yield
                    | Kw::Case
                    | Kw::Defined
                    | Kw::Begin
                    | Kw::Class
                    | Kw::Module
            ),
            Tok::Minus
            | Tok::Plus
            | Tok::Amp
            | Tok::Bang
            | Tok::Colon2
            | Tok::Star
            | Tok::Pow
            | Tok::Tilde => next_tight,
            Tok::Dot2 | Tok::Dot3 => next_tight,
            _ => false,
        }
    }

    fn parse_call_rest(&mut self, recv: Option<Node>, name: String, safe: bool) -> PResult<Node> {
        let mut args = Vec::new();
        let mut block_arg = None;
        let mut parens = false;
        if self.at(&Tok::LParen) && !self.spc() {
            self.advance();
            (args, block_arg) = self.with_no_do(false, |p| p.parse_paren_args(Tok::RParen))?;
            parens = true;
        } else if self.can_start_command_arg() {
            (args, block_arg) = self.with_no_do(true, |p| p.parse_arg_list(None))?;
        }
        let block = if block_arg.is_none() {
            self.parse_block_opt(parens || args.is_empty())?
        } else {
            None
        };
        if recv.is_none() && args.is_empty() && block.is_none() && block_arg.is_none() {
            match name.as_str() {
                "block_given?" | "iterator?" => return Ok(Node::BlockGiven),
                "__method__" => return Ok(Node::CurrentMethod),
                _ => {}
            }
        }
        Ok(Node::Call {
            recv: recv.map(Box::new),
            name,
            args,
            block,
            block_arg,
            safe,
        })
    }

    fn parse_paren_args(&mut self, close: Tok) -> PResult<ArgList> {
        self.skip_nl();
        if self.eat(&close) {
            return Ok((vec![], None));
        }
        let r = self.parse_arg_list(Some(&close))?;
        self.skip_nl();
        self.expect(&close)?;
        Ok(r)
    }

    /// Comma-separated arguments. `*a`, `**h`, `&b`, and trailing
    /// `key: v` / `k => v` pairs (collected into one keyword Hash).
    fn parse_arg_list(&mut self, close: Option<&Tok>) -> PResult<ArgList> {
        let mut args = Vec::new();
        let mut pairs: Vec<HashItem> = Vec::new();
        let mut block_arg = None;
        loop {
            if close.is_some_and(|c| self.at(c)) {
                break; // trailing comma
            }
            match self.peek().clone() {
                Tok::Dot3 if self.arg_ends_at(1) => {
                    self.advance();
                    args.push(Node::Splat(b(Node::LVar("%rest".into()))));
                    pairs.push(HashItem::Splat(Node::LVar("%kwrest".into())));
                    block_arg = Some(b(Node::LVar("%block".into())));
                    break;
                }
                Tok::Amp => {
                    self.advance();
                    block_arg = Some(b(if self.arg_ends() {
                        Node::LVar("%block".into())
                    } else {
                        self.parse_arg()?
                    }));
                    break;
                }
                Tok::Star if !self.spc_at(1) || self.arg_ends_at(1) => {
                    self.advance();
                    args.push(Node::Splat(b(if self.arg_ends() {
                        Node::LVar("%rest".into())
                    } else {
                        self.parse_arg()?
                    })));
                }
                Tok::Pow if !self.spc_at(1) || self.arg_ends_at(1) => {
                    self.advance();
                    pairs.push(HashItem::Splat(if self.arg_ends() {
                        Node::LVar("%kwrest".into())
                    } else {
                        self.parse_arg()?
                    }));
                }
                Tok::Label(k) => {
                    self.advance();
                    self.skip_nl();
                    let v = if self.arg_ends() {
                        self.label_value(&k)
                    } else {
                        self.parse_arg()?
                    };
                    pairs.push(HashItem::Pair(Node::Sym(k), v));
                }
                _ => {
                    let a = self.parse_arg()?;
                    if self.eat(&Tok::Arrow) {
                        self.skip_nl();
                        pairs.push(HashItem::Pair(a, self.parse_arg()?));
                    } else {
                        args.push(a);
                    }
                }
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        if !pairs.is_empty() {
            args.push(Node::Hash(pairs, true));
        }
        Ok((args, block_arg))
    }

    fn arg_ends(&self) -> bool {
        self.arg_ends_at(0)
    }

    fn arg_ends_at(&self, n: usize) -> bool {
        matches!(
            self.peek_n(n),
            Tok::Comma
                | Tok::RParen
                | Tok::RBracket
                | Tok::RBrace
                | Tok::Newline
                | Tok::Semi
                | Tok::Eof
                | Tok::Pipe
        )
    }

    /// Value of an omitted hash/keyword value: `{x:}` means `{x: x}`.
    fn label_value(&self, k: &str) -> Node {
        if self.is_local(k) {
            Node::LVar(k.to_string())
        } else {
            ast::call(None, k, vec![])
        }
    }

    fn parse_block_opt(&mut self, allow_brace: bool) -> PResult<Option<Box<Block>>> {
        if allow_brace && self.eat(&Tok::LBrace) {
            return Ok(Some(Box::new(self.parse_block_body(true)?)));
        }
        if !self.no_do && self.eat_kw(Kw::Do) {
            return Ok(Some(Box::new(self.parse_block_body(false)?)));
        }
        Ok(None)
    }

    fn parse_block_body(&mut self, brace: bool) -> PResult<Block> {
        self.push_scope(false);
        self.skip_nl();
        let mut params = Params::default();
        if self.eat(&Tok::OrOr) {
        } else if self.eat(&Tok::Pipe) {
            params = self.parse_params(Some(Tok::Pipe), true)?;
            self.expect(&Tok::Pipe)?;
        } else {
            self.scopes.last_mut().unwrap().implicit_params = true;
        }
        let body = if brace {
            let body = self.with_no_do(false, |p| p.parse_stmts())?;
            self.expect(&Tok::RBrace)?;
            body
        } else {
            self.with_no_do(false, |p| p.parse_body_rescue())?
        };
        self.finish_block(params, body)
    }

    /// Applies numbered/`it` parameters and builds the Block.
    fn finish_block(&mut self, mut params: Params, body: Node) -> PResult<Block> {
        let scope = self.scopes.last().unwrap();
        if scope.numbered > 0 {
            params.required = (1..=scope.numbered)
                .map(|i| ParamTarget::Name(format!("_{i}")))
                .collect();
        } else if scope.it {
            params.required = vec![ParamTarget::Name("it".into())];
        }
        let mut names = params.names();
        names.extend(destructured_names(&params));
        self.reorder_locals(&names);
        let locals = self.pop_scope();
        Ok(Block {
            params,
            body,
            locals,
            for_loop: false,
        })
    }

    // ---- parameters ----------------------------------------------------

    /// Parameter list up to `close` (not consumed); `None` = until end of line.
    fn parse_params(&mut self, close: Option<Tok>, block: bool) -> PResult<Params> {
        let mut p = Params::default();
        let mut destructure_inner: Vec<MLhs> = Vec::new();
        let at_close = |s: &Self| match &close {
            Some(c) => s.at(c) || (block && s.at(&Tok::Semi)),
            None => matches!(
                s.peek(),
                Tok::Newline | Tok::Semi | Tok::Eof | Tok::Assign | Tok::LBrace | Tok::Kw(Kw::Do)
            ),
        };
        if close.is_some() {
            self.skip_nl();
        }
        while !at_close(self) {
            match self.advance() {
                Tok::Star => {
                    let name = match self.peek().clone() {
                        Tok::Ident(n) => {
                            self.advance();
                            n
                        }
                        _ => "%rest".to_string(),
                    };
                    self.declare_here(&name);
                    p.rest = Some(name);
                }
                Tok::Pow => {
                    let name = match self.peek().clone() {
                        Tok::Ident(n) => {
                            self.advance();
                            n
                        }
                        Tok::Kw(Kw::Nil) => {
                            self.advance();
                            "%nokw".to_string()
                        }
                        _ => "%kwrest".to_string(),
                    };
                    self.declare_here(&name);
                    p.kwrest = Some(name);
                }
                Tok::Amp => {
                    let name = match self.peek().clone() {
                        Tok::Ident(n) => {
                            self.advance();
                            n
                        }
                        _ => "%block".to_string(),
                    };
                    self.declare_here(&name);
                    p.block = Some(name);
                }
                Tok::Label(k) => {
                    self.declare_here(&k);
                    let default = if matches!(
                        self.peek(),
                        Tok::Comma | Tok::Pipe | Tok::RParen | Tok::Newline | Tok::Semi
                    ) || close.as_ref().is_some_and(|c| self.at(c))
                    {
                        None
                    } else {
                        Some(self.param_default(block)?)
                    };
                    p.kw.push((k, default));
                }
                Tok::LParen => {
                    let hidden = self.fresh("d");
                    self.declare_here(&hidden);
                    let inner = self.parse_param_mlhs()?;
                    self.expect(&Tok::RParen)?;
                    self.declare_param_mlhs(&inner);
                    destructure_inner.push(inner.clone());
                    let t = ParamTarget::Destructure(hidden, inner);
                    if p.rest.is_some() || !p.optional.is_empty() {
                        p.post.push(t)
                    } else {
                        p.required.push(t)
                    }
                }
                Tok::Ident(n) => {
                    self.declare_here(&n);
                    if self.eat(&Tok::Assign) {
                        let default = self.param_default(block)?;
                        p.optional.push((n, default));
                    } else if p.rest.is_some() || !p.optional.is_empty() {
                        p.post.push(ParamTarget::Name(n));
                    } else {
                        p.required.push(ParamTarget::Name(n));
                    }
                }
                Tok::Dot3 => {
                    // `...` argument forwarding
                    for (name, slot) in [("%rest", 0), ("%kwrest", 1), ("%block", 2)] {
                        self.declare_here(name);
                        match slot {
                            0 => p.rest = Some(name.into()),
                            1 => p.kwrest = Some(name.into()),
                            _ => p.block = Some(name.into()),
                        }
                    }
                }
                _ => {
                    self.pos -= 1;
                    return Err(self.err("unexpected token in parameter list"));
                }
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
            if block && close.as_ref().is_some_and(|c| self.at(c)) {
                p.trailing_comma = true;
            }
            if close.is_some() {
                self.skip_nl();
            }
        }
        // block-local variables: `|a; b, c|`
        if block && self.eat(&Tok::Semi) {
            loop {
                if let Tok::Ident(n) = self.advance() {
                    self.declare_here(&n);
                }
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        if close.is_some() {
            self.skip_nl();
        }
        for m in &destructure_inner {
            self.declare_param_mlhs(m);
        }
        Ok(p)
    }

    /// Default value of a parameter. In `|a = 1|` the `|` ends the value,
    /// so block parameters take only a primary (as in Ruby's grammar).
    fn param_default(&mut self, block: bool) -> PResult<Node> {
        if block {
            self.parse_unary()
        } else {
            self.with_no_do(false, |p| p.parse_arg())
        }
    }

    /// `(a, (b, *c))` inside a parameter list: names only.
    fn parse_param_mlhs(&mut self) -> PResult<MLhs> {
        let mut m = MLhs::default();
        loop {
            if self.at(&Tok::RParen) {
                break;
            }
            if self.eat(&Tok::Star) {
                let t = match self.peek().clone() {
                    Tok::Ident(n) => {
                        self.advance();
                        Some(Box::new(MTarget::Var(Node::LVar(n))))
                    }
                    _ => None,
                };
                m.splat = Some(t);
            } else {
                let t = match self.advance() {
                    Tok::Ident(n) => MTarget::Var(Node::LVar(n)),
                    Tok::LParen => {
                        let inner = self.parse_param_mlhs()?;
                        self.expect(&Tok::RParen)?;
                        MTarget::Nested(inner)
                    }
                    _ => return Err(self.err("bad destructuring parameter")),
                };
                if m.splat.is_some() {
                    m.post.push(t)
                } else {
                    m.pre.push(t)
                }
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        Ok(m)
    }

    fn declare_param_mlhs(&mut self, m: &MLhs) {
        let mut all: Vec<&MTarget> = m.pre.iter().chain(m.post.iter()).collect();
        if let Some(Some(t)) = &m.splat {
            all.push(t);
        }
        for t in all {
            match t {
                MTarget::Var(Node::LVar(n)) => {
                    let n = n.clone();
                    self.declare_here(&n)
                }
                MTarget::Nested(inner) => self.declare_param_mlhs(inner),
                _ => {}
            }
        }
    }

    // ---- primary -------------------------------------------------------

    fn starts_value(&self) -> bool {
        !matches!(
            self.peek(),
            Tok::Newline
                | Tok::Semi
                | Tok::Eof
                | Tok::RBrace
                | Tok::RParen
                | Tok::RBracket
                | Tok::Kw(
                    Kw::If
                        | Kw::Unless
                        | Kw::While
                        | Kw::Until
                        | Kw::End
                        | Kw::Rescue
                        | Kw::Then
                        | Kw::Do
                )
        )
    }

    /// Value of `return` / `break` / `next`: none, one, or several (an Array).
    fn jump_value(&mut self) -> PResult<Option<Box<Node>>> {
        if !self.starts_value() {
            return Ok(None);
        }
        let (mut args, _) = self.with_no_do(true, |p| p.parse_arg_list(None))?;
        Ok(Some(b(
            if args.len() == 1 && !matches!(args[0], Node::Splat(_)) {
                match args.pop().unwrap() {
                    Node::Hash(items, true) => Node::Hash(items, false),
                    n => n,
                }
            } else {
                Node::Array(args)
            },
        )))
    }

    fn parse_primary(&mut self) -> PResult<Node> {
        let tok = self.peek().clone();
        let line = self.line();
        match tok {
            Tok::Int(i) => {
                self.advance();
                Ok(Node::Int(i))
            }
            Tok::Float(f) => {
                self.advance();
                Ok(Node::Float(f))
            }
            Tok::Str(parts) => {
                self.advance();
                let mut parts = parts;
                // adjacent literals concatenate: "a" "b"
                while let Tok::Str(more) = self.peek().clone() {
                    self.advance();
                    parts.extend(more);
                }
                self.string_node(parts)
            }
            Tok::Sym(s) => {
                self.advance();
                Ok(Node::Sym(s))
            }
            Tok::Regexp(parts, flags) => {
                // as in mruby, a regexp literal is `Regexp.compile(source, flags)`;
                // the Regexp class itself comes from a gem
                self.advance();
                let src = self.string_node(parts)?;
                let mut args = vec![src];
                if !flags.is_empty() {
                    args.push(Node::Str(flags));
                }
                Ok(ast::call(
                    Some(Node::Const("Regexp".into())),
                    "compile",
                    args,
                ))
            }
            Tok::XStr(parts) => {
                self.advance();
                let cmd = self.string_node(parts)?;
                Ok(ast::call(None, "`", vec![cmd]))
            }
            Tok::DSym(parts) => {
                self.advance();
                match self.string_node(parts)? {
                    Node::DStr(v) => Ok(Node::DSym(v)),
                    Node::Str(s) => Ok(Node::Sym(s)),
                    _ => unreachable!(),
                }
            }
            Tok::Words(words, symbols) => {
                self.advance();
                let mut out = Vec::new();
                for w in words {
                    out.push(match (self.string_node(w)?, symbols) {
                        (Node::Str(s), true) => Node::Sym(s),
                        (Node::DStr(v), true) => Node::DSym(v),
                        (n, _) => n,
                    });
                }
                Ok(Node::Array(out))
            }
            Tok::Ident(name) => {
                self.advance();
                self.ident_primary(name, line)
            }
            Tok::Const(name)
                if (name == "END" || name == "BEGIN") && matches!(self.peek2(), Tok::LBrace) =>
            {
                // `END { }` runs at exit; `BEGIN { }` runs in place
                self.advance();
                self.advance();
                let block = self.parse_block_body(true)?;
                if name == "BEGIN" {
                    return Ok(block.body);
                }
                Ok(Node::Call {
                    recv: None,
                    name: "at_exit".into(),
                    args: vec![],
                    block: Some(Box::new(block)),
                    block_arg: None,
                    safe: false,
                })
            }
            Tok::Const(name) => {
                self.advance();
                if self.at(&Tok::LParen) && !self.spc() {
                    // `Integer("42")`: a method whose name looks like a constant
                    return self.parse_call_rest(None, name, false);
                }
                if self.can_start_command_arg() && matches!(self.peek(), Tok::Str(_)) {
                    return self.parse_call_rest(None, name, false);
                }
                Ok(Node::Const(name))
            }
            Tok::Colon2 => {
                self.advance();
                let Tok::Const(name) = self.advance() else {
                    return Err(self.err("expected constant after '::'"));
                };
                Ok(Node::Colon3(name))
            }
            Tok::IVar(name) => {
                self.advance();
                Ok(Node::IVar(name))
            }
            Tok::CVar(name) => {
                self.advance();
                Ok(Node::CVar(name))
            }
            Tok::GVar(name) => {
                self.advance();
                Ok(Node::GVar(name))
            }
            Tok::LParen => {
                self.advance();
                let saved_in = std::mem::replace(&mut self.no_in, false);
                let n = self.with_no_do(false, |p| p.parse_stmts());
                self.no_in = saved_in;
                let n = n?;
                self.expect(&Tok::RParen)?;
                Ok(n)
            }
            Tok::LBracket => {
                self.advance();
                let (mut elems, _) =
                    self.with_no_do(false, |p| p.parse_paren_args(Tok::RBracket))?;
                if let Some(Node::Hash(_, kw)) = elems.last_mut() {
                    *kw = false;
                }
                Ok(Node::Array(elems))
            }
            Tok::LBrace => {
                self.advance();
                self.with_no_do(false, |p| p.parse_hash_literal())
            }
            Tok::Lambda => {
                self.advance();
                self.parse_lambda()
            }
            Tok::Dot2 | Tok::Dot3 => self.parse_range(),
            Tok::Kw(kw) => {
                self.advance();
                // inside `def ... end`, `if ... end` etc. a `do` belongs to calls within
                match kw {
                    Kw::Return
                    | Kw::Break
                    | Kw::Next
                    | Kw::Yield
                    | Kw::Super
                    | Kw::Not
                    | Kw::Defined => self.parse_keyword(kw),
                    _ => self.with_no_do(false, |p| p.parse_keyword(kw)),
                }
            }
            Tok::Bang => {
                self.advance();
                Ok(Node::Not(b(self.parse_unary()?)))
            }
            Tok::Minus | Tok::Plus | Tok::Tilde => self.parse_uminus(),
            Tok::Star => Err(self.err("unexpected splat")),
            _ => Err(self.err("unexpected token")),
        }
    }

    fn ident_primary(&mut self, name: String, line: u32) -> PResult<Node> {
        let call_follows = self.at(&Tok::LParen) && !self.spc();
        if self.is_local(&name) && !call_follows {
            return Ok(Node::LVar(name));
        }
        match name.as_str() {
            "__LINE__" => return Ok(Node::Int(line as i64)),
            "__FILE__" => return Ok(Node::Str(self.file.clone())),
            "__ENCODING__" => return Ok(Node::Str("UTF-8".into())),
            _ => {}
        }
        // numbered block parameters `_1`..`_9` and `it`
        if !call_follows {
            let numbered = name
                .strip_prefix('_')
                .and_then(|d| d.parse::<usize>().ok())
                .filter(|n| (1..=9).contains(n));
            let it_ok = name == "it"
                && !self.can_start_command_arg()
                && !matches!(self.peek(), Tok::LBrace);
            let scope = self.scopes.last_mut().unwrap();
            if scope.implicit_params {
                if let Some(n) = numbered {
                    scope.numbered = scope.numbered.max(n);
                    for i in 1..=scope.numbered {
                        let p = format!("_{i}");
                        if !scope.locals.contains(&p) {
                            scope.locals.push(p);
                        }
                    }
                    return Ok(Node::LVar(name));
                }
                if it_ok {
                    scope.it = true;
                    if !scope.locals.iter().any(|l| l == "it") {
                        scope.locals.push("it".into());
                    }
                    return Ok(Node::LVar(name));
                }
            }
        }
        self.parse_call_rest(None, name, false)
    }

    fn parse_keyword(&mut self, kw: Kw) -> PResult<Node> {
        match kw {
            Kw::Nil => Ok(Node::Nil),
            Kw::True => Ok(Node::True),
            Kw::False => Ok(Node::False),
            Kw::SelfKw => Ok(Node::SelfRef),
            Kw::If => {
                let cond = self.with_no_do(false, |p| p.parse_expr())?;
                self.eat_then();
                let then = self.parse_stmts()?;
                let els = self.parse_else_chain()?;
                Ok(Node::If(b(cond), b(then), b(els)))
            }
            Kw::Unless => {
                let cond = self.parse_expr()?;
                self.eat_then();
                let then = self.parse_stmts()?;
                let els = if self.eat_kw(Kw::Else) {
                    self.parse_stmts()?
                } else {
                    Node::Nil
                };
                self.expect_kw(Kw::End)?;
                Ok(Node::If(b(cond), b(els), b(then)))
            }
            Kw::While | Kw::Until => {
                let cond = self.with_no_do(true, |p| p.parse_expr())?;
                self.eat_kw(Kw::Do);
                let body = self.parse_stmts()?;
                self.expect_kw(Kw::End)?;
                Ok(Node::While {
                    cond: b(cond),
                    body: b(body),
                    until: kw == Kw::Until,
                    do_while: false,
                })
            }
            Kw::For => self.parse_for(),
            Kw::Case => self.parse_case(),
            Kw::Def => self.parse_def(),
            Kw::Class => self.parse_class(false),
            Kw::Module => self.parse_class(true),
            Kw::Begin => {
                let n = self.parse_body_rescue()?;
                Ok(match n {
                    n @ Node::Begin { .. } => n,
                    n => Node::Begin {
                        body: b(n),
                        rescues: vec![],
                        else_body: None,
                        ensure: None,
                    },
                })
            }
            Kw::Return => Ok(Node::Return(self.jump_value()?)),
            Kw::Break => Ok(Node::Break(self.jump_value()?)),
            Kw::Next => Ok(Node::Next(self.jump_value()?)),
            Kw::Redo => Ok(Node::Redo),
            Kw::Retry => Ok(Node::Retry),
            Kw::Yield => {
                let args = self.parse_bare_args()?.map(|(a, _)| a).unwrap_or_default();
                Ok(Node::Yield(args))
            }
            Kw::Super => {
                let (args, block_arg) = match self.parse_bare_args()? {
                    Some((a, ba)) => (Some(a), ba),
                    None => (None, None),
                };
                let block = if block_arg.is_none() {
                    self.parse_block_opt(true)?
                } else {
                    None
                };
                Ok(Node::Super {
                    args,
                    block,
                    block_arg,
                })
            }
            Kw::Not => Ok(Node::Not(b(self.parse_expr()?))),
            Kw::Defined => {
                self.pos -= 1;
                self.parse_notop()
            }
            Kw::Alias => {
                if let Tok::GVar(new) = self.peek().clone() {
                    self.advance();
                    let Tok::GVar(old) = self.advance() else {
                        return Err(self.err("expected global variable"));
                    };
                    return Ok(Node::GAlias(new, old));
                }
                let new = self.parse_method_ref()?;
                let old = self.parse_method_ref()?;
                Ok(Node::Alias(new, old))
            }
            Kw::Undef => {
                self.skip_nl();
                let mut names = vec![self.parse_method_ref()?];
                while self.eat(&Tok::Comma) {
                    self.skip_nl();
                    names.push(self.parse_method_ref()?);
                }
                Ok(Node::Undef(names))
            }
            _ => {
                self.pos -= 1;
                Err(self.err("unexpected keyword"))
            }
        }
    }

    /// A method name in `alias`/`undef`: `foo`, `:foo`, `+`, `[]=`, `foo=`.
    fn parse_method_ref(&mut self) -> PResult<String> {
        let t = self.advance();
        let mut name = match t {
            Tok::Ident(s) | Tok::Const(s) | Tok::Sym(s) => s,
            Tok::Kw(k) => k.as_str().to_string(),
            Tok::Label(s) => return Ok(s), // unusual, but lexes this way before a symbol
            Tok::LBracket => {
                self.expect(&Tok::RBracket)?;
                "[]".to_string()
            }
            t => match op_method_name(&t) {
                Some(op) => op.to_string(),
                None => {
                    self.pos -= 1;
                    return Err(self.err("expected method name"));
                }
            },
        };
        if self.at(&Tok::Assign) && !self.spc() {
            self.advance();
            name.push('=');
        }
        Ok(name)
    }

    /// Arguments of `yield` / `super`: `(a, b)`, `a, b`, or none (None).
    fn parse_bare_args(&mut self) -> PResult<Option<ArgList>> {
        if self.at(&Tok::LParen) && !self.spc() {
            self.advance();
            Ok(Some(
                self.with_no_do(false, |p| p.parse_paren_args(Tok::RParen))?,
            ))
        } else if self.can_start_command_arg() {
            Ok(Some(self.with_no_do(true, |p| p.parse_arg_list(None))?))
        } else {
            Ok(None)
        }
    }

    fn parse_else_chain(&mut self) -> PResult<Node> {
        if self.eat_kw(Kw::Elsif) {
            let cond = self.parse_expr()?;
            self.eat_then();
            let then = self.parse_stmts()?;
            let els = self.parse_else_chain()?;
            return Ok(Node::If(b(cond), b(then), b(els)));
        }
        let els = if self.eat_kw(Kw::Else) {
            self.parse_stmts()?
        } else {
            Node::Nil
        };
        self.expect_kw(Kw::End)?;
        Ok(els)
    }

    /// `for a, b in expr do body end` → `expr.each { |%for| a, b = %for; body }`.
    /// The loop variables and body locals belong to the enclosing scope.
    fn parse_for(&mut self) -> PResult<Node> {
        let saved_in = std::mem::replace(&mut self.no_in, true);
        let lhs = self.parse_mlhs(&[Tok::Kw(Kw::In)]);
        self.no_in = saved_in;
        let lhs = lhs?;
        self.expect_kw(Kw::In)?;
        self.declare_mlhs(&lhs);
        let iter = self.with_no_do(true, |p| p.parse_expr())?;
        self.eat_kw(Kw::Do);
        let body = self.parse_stmts()?;
        self.expect_kw(Kw::End)?;
        let param = self.fresh("for");
        let assign = match (&lhs.pre[..], &lhs.splat, &lhs.post[..]) {
            ([MTarget::Var(v)], None, []) => make_assign(v.clone(), Node::LVar(param.clone())),
            _ => Node::MAsgn(lhs, b(Node::LVar(param.clone()))),
        };
        let block = Block {
            params: Params {
                required: vec![ParamTarget::Name(param.clone())],
                ..Default::default()
            },
            body: Node::Seq(vec![assign, body]),
            locals: vec![param],
            for_loop: true,
        };
        Ok(Node::Call {
            recv: Some(b(iter)),
            name: "each".into(),
            args: vec![],
            block: Some(Box::new(block)),
            block_arg: None,
            safe: false,
        })
    }

    /// `case/when` is desugared into an `if` chain using `===`, as Ruby defines it.
    fn parse_case(&mut self) -> PResult<Node> {
        let subject = if matches!(self.peek(), Tok::Newline | Tok::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.skip_terms();
        if self.at_kw(Kw::In) {
            let subject = subject.ok_or_else(|| self.err("case/in needs a subject"))?;
            return self.parse_case_in(subject);
        }
        let tmp = self.fresh("case");
        if subject.is_some() {
            self.declare_here(&tmp);
        }
        let mut whens = Vec::new();
        while self.eat_kw(Kw::When) {
            let mut conds = vec![self.parse_when_cond()?];
            while self.eat(&Tok::Comma) {
                self.skip_nl();
                conds.push(self.parse_when_cond()?);
            }
            self.eat_then();
            whens.push((conds, self.parse_stmts()?));
        }
        let mut chain = if self.eat_kw(Kw::Else) {
            self.parse_stmts()?
        } else {
            Node::Nil
        };
        self.expect_kw(Kw::End)?;
        for (conds, body) in whens.into_iter().rev() {
            let mut test: Option<Node> = None;
            for c in conds {
                let t = match (&subject, c) {
                    (Some(_), Node::Splat(list)) => {
                        any_matches(*list, Node::LVar(tmp.clone()), self.fresh("w"))
                    }
                    (Some(_), c) => ast::call(Some(c), "===", vec![Node::LVar(tmp.clone())]),
                    (None, Node::Splat(list)) => ast::call(Some(*list), "any?", vec![]),
                    (None, c) => c,
                };
                test = Some(match test {
                    None => t,
                    Some(prev) => Node::Or(b(prev), b(t)),
                });
            }
            chain = Node::If(b(test.unwrap()), b(body), b(chain));
        }
        Ok(match subject {
            Some(s) => Node::Seq(vec![Node::LAsgn(tmp, b(s)), chain]),
            None => chain,
        })
    }

    fn parse_when_cond(&mut self) -> PResult<Node> {
        if self.eat(&Tok::Star) {
            return Ok(Node::Splat(b(self.parse_ternary()?)));
        }
        self.parse_ternary()
    }

    fn parse_def(&mut self) -> PResult<Node> {
        // singleton target: `def self.x`, `def obj.x`, `def Const.x`, `def (expr).x`
        let mut recv = None;
        if matches!(self.peek2(), Tok::Dot) {
            let line = self.line();
            let target = match self.peek().clone() {
                Tok::Kw(Kw::SelfKw) => Some(Node::SelfRef),
                Tok::Kw(Kw::Nil) => Some(Node::Nil),
                // via ident_primary so `def _1.m` declares the numbered parameter
                Tok::Ident(n) => Some(self.with_advance(|p| p.ident_primary(n, line))?),
                Tok::Const(c) => Some(Node::Const(c)),
                Tok::IVar(n) => Some(Node::IVar(n)),
                _ => None,
            };
            if let Some(t) = target {
                if !matches!(t, Node::LVar(_) | Node::Call { .. }) {
                    self.advance(); // the target token (ident_primary consumed its own)
                }
                self.expect(&Tok::Dot)?;
                recv = Some(b(t));
            }
        } else if self.at(&Tok::LParen) {
            self.advance();
            let e = self.parse_expr()?;
            self.expect(&Tok::RParen)?;
            self.expect(&Tok::Dot)?;
            recv = Some(b(e));
        }
        let t = self.advance();
        let mut name: String = match t {
            Tok::Ident(s) | Tok::Const(s) => s,
            Tok::Kw(k) => k.as_str().into(),
            Tok::Label(s) => {
                // `def foo:` can't happen; `def a?:`-like oddities
                s
            }
            Tok::LBracket => {
                self.expect(&Tok::RBracket)?;
                "[]".into()
            }
            t => match op_method_name(&t) {
                Some(op) => op.to_string(),
                None => {
                    self.pos -= 1;
                    return Err(self.err("bad method name"));
                }
            },
        };
        // unary operators `-@`, `+@`, `!@`, `~@`
        if matches!(name.as_str(), "+" | "-" | "!" | "~") && self.at(&Tok::IVar("@".into())) {
            self.advance();
            if name == "+" || name == "-" {
                name.push('@');
            }
        }
        if self.at(&Tok::Assign)
            && !self.spc()
            && (is_ident_name(&name) || name == "[]" || name.starts_with(char::is_uppercase))
        {
            // setter, unless it's an endless def without spaces (`def a=1` is not valid Ruby anyway)
            if matches!(self.peek2(), Tok::LParen | Tok::Ident(_)) {
                self.advance();
                name.push('=');
            }
        }
        self.push_scope(true);
        let params = if self.at(&Tok::LParen) {
            self.advance();
            let p = self.parse_params(Some(Tok::RParen), false)?;
            self.expect(&Tok::RParen)?;
            p
        } else if matches!(
            self.peek(),
            Tok::Ident(_) | Tok::Star | Tok::Pow | Tok::Amp | Tok::Label(_) | Tok::Dot3
        ) {
            self.parse_params(None, false)?
        } else {
            Params::default()
        };
        let body = if self.at(&Tok::Assign) {
            // endless def: `def m(a) = expr`
            self.advance();
            self.skip_nl();
            let e = self.parse_expr()?;
            if self.eat_kw(Kw::Rescue) {
                rescue_modifier(e, self.parse_expr()?)
            } else {
                e
            }
        } else {
            self.parse_body_rescue()?
        };
        let mut names = params.names();
        names.extend(destructured_names(&params));
        self.reorder_locals(&names);
        let locals = self.pop_scope();
        Ok(Node::Def {
            recv,
            name,
            params,
            body: b(body),
            locals,
        })
    }

    fn parse_class(&mut self, is_module: bool) -> PResult<Node> {
        if !is_module && self.eat(&Tok::Shl) {
            // `class << obj`
            let target = self.parse_expr()?;
            self.push_scope(true);
            let body = self.parse_body_rescue()?;
            let locals = self.pop_scope();
            return Ok(Node::SClass(b(target), b(body), locals));
        }
        let path = self.parse_cpath()?;
        let superclass = if !is_module && self.eat(&Tok::Lt) {
            Some(b(self.parse_expr()?))
        } else {
            None
        };
        self.push_scope(true);
        let body = self.parse_body_rescue()?;
        let locals = self.pop_scope();
        Ok(Node::Class {
            path: b(path),
            superclass,
            body: b(body),
            locals,
            is_module,
        })
    }

    /// `Name`, `::Name`, `Outer::Name`, `expr::Name`
    fn parse_cpath(&mut self) -> PResult<Node> {
        let mut node = match self.peek().clone() {
            Tok::Const(c) => {
                self.advance();
                Node::Const(c)
            }
            Tok::Colon2 => {
                self.advance();
                let Tok::Const(c) = self.advance() else {
                    return Err(self.err("expected constant"));
                };
                Node::Colon3(c)
            }
            _ => {
                let base = self.parse_primary()?;
                if !self.at(&Tok::Colon2) {
                    return Err(self.err("class/module name must be a CONSTANT"));
                }
                base
            }
        };
        while self.at(&Tok::Colon2) {
            self.advance();
            let Tok::Const(c) = self.advance() else {
                return Err(self.err("expected constant"));
            };
            node = Node::Colon2(b(node), c);
        }
        Ok(node)
    }

    /// Body with optional `rescue`/`else`/`ensure` clauses, up to and including `end`.
    fn parse_body_rescue(&mut self) -> PResult<Node> {
        let body = self.parse_stmts()?;
        let mut rescues = Vec::new();
        while self.eat_kw(Kw::Rescue) {
            let mut classes = Vec::new();
            if !matches!(
                self.peek(),
                Tok::Newline | Tok::Semi | Tok::Arrow | Tok::Kw(Kw::Then)
            ) {
                loop {
                    classes.push(if self.eat(&Tok::Star) {
                        Node::Splat(b(self.parse_ternary()?))
                    } else {
                        self.parse_ternary()?
                    });
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                    self.skip_nl();
                }
            }
            let mut var = None;
            if self.eat(&Tok::Arrow) {
                let target = self.parse_primary_postfix_no_call_args()?;
                if !is_assignable(&target) {
                    return Err(self.err("can't assign exception to this"));
                }
                self.declare_lhs(&target);
                var = Some(b(read_of(&target)));
            }
            self.eat_then();
            rescues.push(Rescue {
                classes,
                var,
                body: self.parse_stmts()?,
            });
        }
        let else_body = if self.eat_kw(Kw::Else) {
            Some(b(self.parse_stmts()?))
        } else {
            None
        };
        let ensure = if self.eat_kw(Kw::Ensure) {
            Some(b(self.parse_stmts()?))
        } else {
            None
        };
        self.expect_kw(Kw::End)?;
        if rescues.is_empty() && ensure.is_none() && else_body.is_none() {
            return Ok(body);
        }
        Ok(Node::Begin {
            body: b(body),
            rescues,
            else_body,
            ensure,
        })
    }

    fn parse_lambda(&mut self) -> PResult<Node> {
        self.push_scope(false);
        let params = if self.at(&Tok::LParen) {
            self.advance();
            let p = self.parse_params(Some(Tok::RParen), true)?;
            self.expect(&Tok::RParen)?;
            p
        } else if matches!(
            self.peek(),
            Tok::Ident(_) | Tok::Star | Tok::Pow | Tok::Amp | Tok::Label(_)
        ) {
            self.parse_params(None, true)?
        } else {
            self.scopes.last_mut().unwrap().implicit_params = true;
            Params::default()
        };
        let body = if self.eat(&Tok::LBrace) {
            let body = self.with_no_do(false, |p| p.parse_stmts())?;
            self.expect(&Tok::RBrace)?;
            body
        } else if self.eat_kw(Kw::Do) {
            self.with_no_do(false, |p| p.parse_body_rescue())?
        } else {
            return Err(self.err("expected lambda body"));
        };
        let block = self.finish_block(params, body)?;
        Ok(Node::Lambda(Box::new(block)))
    }

    fn parse_hash_literal(&mut self) -> PResult<Node> {
        let mut items = Vec::new();
        self.skip_nl();
        while !self.at(&Tok::RBrace) {
            match self.peek().clone() {
                Tok::Label(k) => {
                    self.advance();
                    self.skip_nl();
                    let v = if self.arg_ends() {
                        self.label_value(&k)
                    } else {
                        self.parse_arg()?
                    };
                    items.push(HashItem::Pair(Node::Sym(k), v));
                }
                Tok::Pow => {
                    self.advance();
                    items.push(HashItem::Splat(self.parse_arg()?));
                }
                Tok::DSym(parts) if matches!(self.peek2(), Tok::Colon) => {
                    // `"a#{b}": v`
                    self.advance();
                    self.advance();
                    let k = match self.string_node(parts)? {
                        Node::DStr(v) => Node::DSym(v),
                        Node::Str(s) => Node::Sym(s),
                        _ => unreachable!(),
                    };
                    items.push(HashItem::Pair(k, self.parse_arg()?));
                }
                _ => {
                    let k = self.parse_arg()?;
                    self.skip_nl();
                    self.expect(&Tok::Arrow)?;
                    self.skip_nl();
                    items.push(HashItem::Pair(k, self.parse_arg()?));
                }
            }
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        self.expect(&Tok::RBrace)?;
        Ok(Node::Hash(items, false))
    }

    fn string_node(&mut self, parts: Vec<StrPart>) -> PResult<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => match nodes.last_mut() {
                    Some(Node::Str(prev)) => prev.push_str(&s),
                    _ => nodes.push(Node::Str(s)),
                },
                StrPart::Code(src) => nodes.push(self.parse_sub(&src)?),
            }
        }
        Ok(match nodes.len() {
            0 => Node::Str(String::new()),
            1 if matches!(nodes[0], Node::Str(_)) => nodes.pop().unwrap(),
            _ => Node::DStr(nodes),
        })
    }

    /// Parses `#{...}` code in the current scope (so it sees local variables).
    fn parse_sub(&mut self, src: &str) -> PResult<Node> {
        let toks = lex(src)?;
        let saved_toks = std::mem::replace(&mut self.toks, toks);
        let saved_pos = std::mem::replace(&mut self.pos, 0);
        let saved_in = std::mem::replace(&mut self.no_in, false);
        let r = self.with_no_do(false, |p| p.parse_stmts());
        let ok = self.at(&Tok::Eof);
        self.toks = saved_toks;
        self.pos = saved_pos;
        self.no_in = saved_in;
        let node = r?;
        if !ok {
            return Err("syntax error in string interpolation".into());
        }
        Ok(node)
    }
}

use crate::ast;

/// Method names spelled with operator tokens (`a.+(b)`, `def <=>`, `alias`).
fn op_method_name(t: &Tok) -> Option<&'static str> {
    Some(match t {
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Pow => "**",
        Tok::Slash => "/",
        Tok::Percent => "%",
        Tok::EqEq => "==",
        Tok::NotEq => "!=",
        Tok::Eqq => "===",
        Tok::Match => "=~",
        Tok::NotMatch => "!~",
        Tok::Lt => "<",
        Tok::Le => "<=",
        Tok::Gt => ">",
        Tok::Ge => ">=",
        Tok::Cmp => "<=>",
        Tok::Shl => "<<",
        Tok::Shr => ">>",
        Tok::Amp => "&",
        Tok::Pipe => "|",
        Tok::Caret => "^",
        Tok::Tilde => "~",
        Tok::Bang => "!",
        _ => return None,
    })
}

fn rescue_modifier(body: Node, fallback: Node) -> Node {
    let wrap = |body: Node| Node::Begin {
        body: b(body),
        rescues: vec![Rescue {
            classes: vec![],
            var: None,
            body: fallback,
        }],
        else_body: None,
        ensure: None,
    };
    // `x = expr rescue v` means `x = (expr rescue v)`
    match body {
        Node::LAsgn(n, rhs) => Node::LAsgn(n, b(wrap(*rhs))),
        Node::IAsgn(n, rhs) => Node::IAsgn(n, b(wrap(*rhs))),
        Node::GAsgn(n, rhs) => Node::GAsgn(n, b(wrap(*rhs))),
        Node::CVAsgn(n, rhs) => Node::CVAsgn(n, b(wrap(*rhs))),
        other => wrap(other),
    }
}

fn is_single_assign(n: &Node) -> bool {
    matches!(
        n,
        Node::LAsgn(..) | Node::IAsgn(..) | Node::GAsgn(..) | Node::CVAsgn(..) | Node::CAsgn(..)
    )
}

fn assign_value(n: &Node) -> Node {
    match n {
        Node::LAsgn(_, v)
        | Node::IAsgn(_, v)
        | Node::GAsgn(_, v)
        | Node::CVAsgn(_, v)
        | Node::CAsgn(_, v) => (**v).clone(),
        _ => unreachable!(),
    }
}

fn replace_assign_value(n: Node, v: Node) -> Node {
    match n {
        Node::LAsgn(a, _) => Node::LAsgn(a, b(v)),
        Node::IAsgn(a, _) => Node::IAsgn(a, b(v)),
        Node::GAsgn(a, _) => Node::GAsgn(a, b(v)),
        Node::CVAsgn(a, _) => Node::CVAsgn(a, b(v)),
        Node::CAsgn(a, _) => Node::CAsgn(a, b(v)),
        n => n,
    }
}

/// `when *list` → `list.any? { |w| w === subject }`
fn any_matches(list: Node, subject: Node, w: String) -> Node {
    let block = Block {
        params: Params {
            required: vec![ParamTarget::Name(w.clone())],
            ..Default::default()
        },
        body: ast::call(Some(Node::LVar(w.clone())), "===", vec![subject]),
        locals: vec![w],
        for_loop: false,
    };
    Node::Call {
        recv: Some(b(Node::Array(vec![Node::Splat(b(list))]))),
        name: "any?".into(),
        args: vec![],
        block: Some(Box::new(block)),
        block_arg: None,
        safe: false,
    }
}

/// Names bound by destructuring parameters, in order.
fn destructured_names(p: &Params) -> Vec<String> {
    fn walk(m: &MLhs, out: &mut Vec<String>) {
        let mut all: Vec<&MTarget> = m.pre.iter().collect();
        if let Some(Some(t)) = &m.splat {
            all.push(t);
        }
        all.extend(m.post.iter());
        for t in all {
            match t {
                MTarget::Var(Node::LVar(n)) => out.push(n.clone()),
                MTarget::Nested(inner) => walk(inner, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for t in p.required.iter().chain(p.post.iter()) {
        if let ParamTarget::Destructure(_, m) = t {
            walk(m, &mut out);
        }
    }
    out
}
