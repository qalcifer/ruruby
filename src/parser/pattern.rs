//! Pattern matching (`case/in`, `expr => pat`, `expr in pat`).
//!
//! Patterns are desugared here into ordinary expressions: `===` tests,
//! `deconstruct` / `deconstruct_keys` calls, local assignments and a loop for
//! find patterns. The code generator and VM need nothing special.

use super::{PREC_ADD, PResult, Parser, b};
use crate::ast::{self, Node};
use crate::lexer::{Kw, Tok};

enum Pat {
    /// Literal, constant, range, pinned value: matched with `===`.
    Value(Node),
    Bind(String),
    Alt(Vec<Pat>),
    Capture(Box<Pat>, String),
    Array {
        konst: Option<Node>,
        pre: Vec<Pat>,
        rest: Option<Option<String>>,
        post: Vec<Pat>,
    },
    /// `[*pre, x, y, *post]`
    Find {
        konst: Option<Node>,
        pre: Option<String>,
        mid: Vec<Pat>,
        post: Option<String>,
    },
    Hash {
        konst: Option<Node>,
        pairs: Vec<(String, Option<Pat>)>,
        rest: HashRest,
    },
}

enum HashRest {
    None,
    /// `**rest` / `**`
    Rest(Option<String>),
    /// `**nil`
    NoRest,
}

fn and_all(mut conds: Vec<Node>) -> Node {
    let mut acc = conds.pop().unwrap_or(Node::True);
    while let Some(c) = conds.pop() {
        acc = Node::And(b(c), b(acc));
    }
    acc
}

fn bind(name: &str, v: Node) -> Node {
    Node::Seq(vec![Node::LAsgn(name.to_string(), b(v)), Node::True])
}

fn lvar(n: &str) -> Node {
    Node::LVar(n.to_string())
}

fn call(recv: Node, name: &str, args: Vec<Node>) -> Node {
    ast::call(Some(recv), name, args)
}

impl Parser {
    /// `case subject; in pat [if guard] then body ... else ... end`
    #[inline(never)]
    pub(super) fn parse_case_in(&mut self, subject: Node, braced: bool) -> PResult<Node> {
        let tmp = self.fresh("pm");
        self.declare_here(&tmp);
        let (cache, flag) = (self.fresh("pmd"), self.fresh("pmf"));
        self.declare_here(&cache);
        self.declare_here(&flag);
        let saved = self.pm_cache.replace((tmp.clone(), cache, flag.clone()));
        let r = self.case_in_clauses(&tmp, braced);
        self.pm_cache = saved;
        let chain = r?;
        Ok(Node::Seq(vec![
            Node::LAsgn(tmp, b(subject)),
            Node::LAsgn(flag, b(Node::Nil)),
            chain,
        ]))
    }

    fn case_in_clauses(&mut self, tmp: &str, braced: bool) -> PResult<Node> {
        let tmp = tmp.to_string();
        let mut clauses = Vec::new();
        while self.eat_kw(Kw::In) {
            let pat = self.parse_top_pattern()?;
            let mut test = self.pattern_test(&pat, lvar(&tmp));
            if self.eat_kw(Kw::If) {
                test = Node::And(b(test), b(self.header(|p| p.parse_expr())?));
            } else if self.eat_kw(Kw::Unless) {
                test = Node::And(b(test), b(Node::Not(b(self.header(|p| p.parse_expr())?))));
            }
            let body = self.clause_body(braced)?;
            clauses.push((test, body));
        }
        let mut chain = if self.eat_kw(Kw::Else) {
            self.clause_body(braced)?
        } else {
            no_match(&tmp)
        };
        self.end_clauses(braced)?;
        for (test, body) in clauses.into_iter().rev() {
            chain = Node::If(b(test), b(body), b(chain));
        }
        Ok(chain)
    }

    /// `value => pattern` (raises on mismatch) or `value in pattern` (true/false).
    pub(super) fn rightward_pattern(&mut self, value: Node, raise: bool) -> PResult<Node> {
        let tmp = self.fresh("pm");
        self.declare(&tmp);
        let pat = self.parse_top_pattern()?;
        let test = self.pattern_test(&pat, lvar(&tmp));
        let result = if raise {
            Node::If(b(test), b(Node::Nil), b(no_match(&tmp)))
        } else {
            Node::If(b(test), b(Node::True), b(Node::False))
        };
        Ok(Node::Seq(vec![Node::LAsgn(tmp, b(value)), result]))
    }

    // ---- parsing ---------------------------------------------------------

    fn pattern_end(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Newline
                | Tok::Semi
                | Tok::Eof
                | Tok::RParen
                | Tok::Kw(Kw::Then | Kw::If | Kw::Unless | Kw::And | Kw::Or)
        )
    }

    fn parse_top_pattern(&mut self) -> PResult<Pat> {
        if matches!(self.peek(), Tok::Label(_)) || (self.at(&Tok::Pow) && !self.spc_at(1)) {
            return self.parse_hash_pattern_items(None, |p| p.pattern_end());
        }
        let first = match self.at(&Tok::Star) {
            true => None,
            false => Some(self.parse_pattern()?),
        };
        let first = match first {
            Some(p) if !self.at(&Tok::Comma) => return Ok(p),
            f => f,
        };
        // `in a, *rest` — an array pattern without brackets
        let mut items: Vec<Item> = first.map(Item::Pat).into_iter().collect();
        if items.is_empty() || self.eat(&Tok::Comma) {
            items.extend(self.parse_array_items(|p| p.pattern_end())?);
        }
        self.array_pattern(None, items)
    }

    fn parse_pattern(&mut self) -> PResult<Pat> {
        self.nested(Self::parse_pattern_alts)
    }

    fn parse_pattern_alts(&mut self) -> PResult<Pat> {
        let mut alts = vec![self.parse_pattern_primary()?];
        while self.at(&Tok::Pipe) {
            self.advance();
            self.skip_nl();
            alts.push(self.parse_pattern_primary()?);
        }
        let mut p = if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Pat::Alt(alts)
        };
        while self.eat(&Tok::Arrow) {
            let Tok::Ident(name) = self.advance() else {
                return Err(self.err("expected a variable name after '=>' in pattern"));
            };
            self.declare(&name);
            p = Pat::Capture(Box::new(p), name);
        }
        Ok(p)
    }

    fn parse_pattern_primary(&mut self) -> PResult<Pat> {
        match self.peek().clone() {
            Tok::LBracket => {
                self.advance();
                self.skip_nl();
                let items = self.parse_array_items(|p| p.at(&Tok::RBracket))?;
                self.skip_nl();
                self.expect(&Tok::RBracket)?;
                self.array_pattern(None, items)
            }
            Tok::LBrace => {
                self.advance();
                self.skip_nl();
                let p = self.parse_hash_pattern_items(None, |p| p.at(&Tok::RBrace))?;
                self.skip_nl();
                self.expect(&Tok::RBrace)?;
                Ok(p)
            }
            Tok::LParen => {
                self.advance();
                let p = self.parse_pattern()?;
                self.expect(&Tok::RParen)?;
                Ok(p)
            }
            Tok::Caret => {
                self.advance();
                let v = match self.advance() {
                    Tok::Ident(n) => Node::LVar(n),
                    Tok::IVar(n) => Node::IVar(n),
                    Tok::GVar(n) => Node::GVar(n),
                    Tok::CVar(n) => Node::CVar(n),
                    Tok::LParen => {
                        let e = self.parse_expr()?;
                        self.expect(&Tok::RParen)?;
                        e
                    }
                    _ => return Err(self.err("bad pinned expression")),
                };
                Ok(Pat::Value(v))
            }
            Tok::Ident(name) if !matches!(self.peek2(), Tok::LParen | Tok::Dot) => {
                self.advance();
                self.declare(&name);
                Ok(Pat::Bind(name))
            }
            Tok::Const(_) | Tok::Colon2 => {
                let mut konst = match self.advance() {
                    Tok::Const(c) => Node::Const(c),
                    _ => {
                        let Tok::Const(c) = self.advance() else {
                            return Err(self.err("expected constant"));
                        };
                        Node::Colon3(c)
                    }
                };
                while self.at(&Tok::Colon2) {
                    self.advance();
                    let Tok::Const(c) = self.advance() else {
                        return Err(self.err("expected constant"));
                    };
                    konst = Node::Colon2(b(konst), c);
                }
                if !self.spc() && (self.at(&Tok::LParen) || self.at(&Tok::LBracket)) {
                    let close = if self.advance() == Tok::LParen {
                        Tok::RParen
                    } else {
                        Tok::RBracket
                    };
                    self.skip_nl();
                    let p = if matches!(self.peek(), Tok::Label(_)) {
                        self.parse_hash_pattern_items(Some(konst), |p| p.at(&close))?
                    } else {
                        let items = self.parse_array_items(|p| p.at(&close))?;
                        self.array_pattern(Some(konst), items)?
                    };
                    self.skip_nl();
                    self.expect(&close)?;
                    return Ok(p);
                }
                self.range_pattern(konst)
            }
            _ => {
                let v = if matches!(self.peek(), Tok::Dot2 | Tok::Dot3) {
                    Node::Nil
                } else {
                    self.parse_binary(PREC_ADD)?
                };
                self.range_pattern(v)
            }
        }
    }

    /// A value, possibly the start of a range: `1..5`, `..5`, `1..`.
    fn range_pattern(&mut self, lo: Node) -> PResult<Pat> {
        let excl = match self.peek() {
            Tok::Dot2 => false,
            Tok::Dot3 => true,
            _ => return Ok(Pat::Value(lo)),
        };
        self.advance();
        let hi = if self.pattern_end()
            || matches!(
                self.peek(),
                Tok::Pipe | Tok::Comma | Tok::RBracket | Tok::RBrace | Tok::Arrow
            ) {
            Node::Nil
        } else {
            self.parse_binary(PREC_ADD)?
        };
        Ok(Pat::Value(Node::Range(b(lo), b(hi), excl)))
    }

    fn parse_array_items(&mut self, at_end: impl Fn(&Self) -> bool) -> PResult<Vec<Item>> {
        let mut items = Vec::new();
        while !at_end(self) {
            if self.eat(&Tok::Star) {
                let name = match self.peek().clone() {
                    Tok::Ident(n) => {
                        self.advance();
                        self.declare(&n);
                        Some(n)
                    }
                    _ => None,
                };
                items.push(Item::Splat(name));
            } else {
                items.push(Item::Pat(self.parse_pattern()?));
            }
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
            if at_end(self) {
                items.push(Item::Splat(None)); // `[a,]` means `[a, *]`
            }
        }
        Ok(items)
    }

    fn array_pattern(&mut self, konst: Option<Node>, items: Vec<Item>) -> PResult<Pat> {
        let splats: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::Splat(_)))
            .map(|(i, _)| i)
            .collect();
        match splats.as_slice() {
            [] => Ok(Pat::Array {
                konst,
                pre: items.into_iter().map(Item::into_pat).collect(),
                rest: None,
                post: vec![],
            }),
            [i] => {
                let mut items = items;
                let post: Vec<Pat> = items
                    .split_off(i + 1)
                    .into_iter()
                    .map(Item::into_pat)
                    .collect();
                let Some(Item::Splat(rest)) = items.pop() else {
                    unreachable!()
                };
                let pre = items.into_iter().map(Item::into_pat).collect();
                Ok(Pat::Array {
                    konst,
                    pre,
                    rest: Some(rest),
                    post,
                })
            }
            [first, last] if *first == 0 && *last == items.len() - 1 => {
                let mut items = items;
                let Some(Item::Splat(post)) = items.pop() else {
                    unreachable!()
                };
                let mut it = items.into_iter();
                let Some(Item::Splat(pre)) = it.next() else {
                    unreachable!()
                };
                Ok(Pat::Find {
                    konst,
                    pre,
                    mid: it.map(Item::into_pat).collect(),
                    post,
                })
            }
            _ => Err(self.err("bad splats in array pattern")),
        }
    }

    fn parse_hash_pattern_items(
        &mut self,
        konst: Option<Node>,
        at_end: impl Fn(&Self) -> bool,
    ) -> PResult<Pat> {
        let mut pairs = Vec::new();
        let mut rest = HashRest::None;
        while !at_end(self) {
            match self.advance() {
                Tok::Label(k) => {
                    let value_follows = !(at_end(self)
                        || self.pattern_end()
                        || matches!(
                            self.peek(),
                            Tok::Comma | Tok::RBrace | Tok::RParen | Tok::RBracket | Tok::Pipe
                        ));
                    if value_follows {
                        pairs.push((k, Some(self.parse_pattern()?)));
                    } else {
                        self.declare(&k);
                        pairs.push((k, None));
                    }
                }
                Tok::Pow => {
                    rest = match self.peek().clone() {
                        Tok::Kw(Kw::Nil) => {
                            self.advance();
                            HashRest::NoRest
                        }
                        Tok::Ident(n) => {
                            self.advance();
                            self.declare(&n);
                            HashRest::Rest(Some(n))
                        }
                        _ => HashRest::Rest(None),
                    };
                }
                _ => {
                    self.pos -= 1;
                    return Err(self.err("bad hash pattern"));
                }
            }
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        Ok(Pat::Hash { konst, pairs, rest })
    }

    // ---- desugaring --------------------------------------------------------

    /// An expression that is truthy when `v` (a local variable read) matches,
    /// binding pattern variables as a side effect.
    fn pattern_test(&mut self, pat: &Pat, v: Node) -> Node {
        match pat {
            Pat::Value(e) => call(e.clone(), "===", vec![v]),
            Pat::Bind(n) => bind(n, v),
            Pat::Alt(ps) => {
                let mut tests: Vec<Node> =
                    ps.iter().map(|p| self.pattern_test(p, v.clone())).collect();
                let mut acc = tests.pop().unwrap();
                while let Some(t) = tests.pop() {
                    acc = Node::Or(b(t), b(acc));
                }
                acc
            }
            Pat::Capture(p, n) => Node::And(b(self.pattern_test(p, v.clone())), b(bind(n, v))),
            Pat::Array {
                konst,
                pre,
                rest,
                post,
            } => {
                let d = self.fresh("pd");
                self.declare(&d);
                let mut conds = self.deconstruct(konst, &v, &d);
                let n = (pre.len() + post.len()) as i64;
                let size = call(lvar(&d), "length", vec![]);
                conds.push(call(
                    size,
                    if rest.is_some() { ">=" } else { "==" },
                    vec![Node::Int(n)],
                ));
                for (i, p) in pre.iter().enumerate() {
                    conds.push(
                        self.element_test(p, call(lvar(&d), "[]", vec![Node::Int(i as i64)])),
                    );
                }
                if let Some(Some(name)) = rest {
                    let range = Node::Range(
                        b(Node::Int(pre.len() as i64)),
                        b(Node::Int(-(post.len() as i64) - 1)),
                        false,
                    );
                    conds.push(bind(name, call(lvar(&d), "[]", vec![range])));
                }
                for (j, p) in post.iter().enumerate() {
                    let idx = -((post.len() - j) as i64);
                    conds.push(self.element_test(p, call(lvar(&d), "[]", vec![Node::Int(idx)])));
                }
                and_all(conds)
            }
            Pat::Find {
                konst,
                pre,
                mid,
                post,
            } => {
                let d = self.fresh("pd");
                let i = self.fresh("pi");
                let found = self.fresh("pf");
                for n in [&d, &i, &found] {
                    self.declare(n);
                }
                let mut conds = self.deconstruct(konst, &v, &d);
                let n = mid.len() as i64;
                let mut tests = Vec::new();
                for (j, p) in mid.iter().enumerate() {
                    let at = call(lvar(&i), "+", vec![Node::Int(j as i64)]);
                    tests.push(self.element_test(p, call(lvar(&d), "[]", vec![at])));
                }
                let search = Node::Seq(vec![
                    Node::LAsgn(i.clone(), b(Node::Int(0))),
                    Node::LAsgn(found.clone(), b(Node::False)),
                    Node::While {
                        cond: b(Node::And(
                            b(Node::Not(b(lvar(&found)))),
                            b(call(
                                call(lvar(&i), "+", vec![Node::Int(n)]),
                                "<=",
                                vec![call(lvar(&d), "length", vec![])],
                            )),
                        )),
                        body: b(Node::Seq(vec![
                            Node::LAsgn(found.clone(), b(and_all(tests))),
                            Node::If(
                                b(lvar(&found)),
                                b(Node::Nil),
                                b(Node::LAsgn(
                                    i.clone(),
                                    b(call(lvar(&i), "+", vec![Node::Int(1)])),
                                )),
                            ),
                        ])),
                        until: false,
                        do_while: false,
                    },
                    lvar(&found),
                ]);
                conds.push(search);
                if let Some(name) = pre {
                    conds.push(bind(
                        name,
                        call(
                            lvar(&d),
                            "[]",
                            vec![Node::Range(b(Node::Int(0)), b(lvar(&i)), true)],
                        ),
                    ));
                }
                if let Some(name) = post {
                    let from = call(lvar(&i), "+", vec![Node::Int(n)]);
                    conds.push(bind(
                        name,
                        call(
                            lvar(&d),
                            "[]",
                            vec![Node::Range(b(from), b(Node::Int(-1)), false)],
                        ),
                    ));
                }
                and_all(conds)
            }
            Pat::Hash { konst, pairs, rest } => {
                let h = self.fresh("ph");
                self.declare(&h);
                let mut conds = Vec::new();
                if let Some(k) = konst {
                    conds.push(call(k.clone(), "===", vec![v.clone()]));
                }
                let keys = match rest {
                    HashRest::Rest(Some(_)) => Node::Nil,
                    _ => Node::Array(pairs.iter().map(|(k, _)| Node::Sym(k.clone())).collect()),
                };
                // nil (no match) when the subject has no deconstruct_keys; TypeError unless it answers a Hash
                let dk = ast::call(None, "__pm_deconstruct_keys", vec![v, keys]);
                conds.push(Node::Seq(vec![Node::LAsgn(h.clone(), b(dk)), lvar(&h)]));
                for (k, p) in pairs {
                    conds.push(call(lvar(&h), "key?", vec![Node::Sym(k.clone())]));
                    let value = call(lvar(&h), "[]", vec![Node::Sym(k.clone())]);
                    conds.push(match p {
                        Some(p) => self.element_test(p, value),
                        None => bind(k, value),
                    });
                }
                let syms: Vec<Node> = pairs.iter().map(|(k, _)| Node::Sym(k.clone())).collect();
                match rest {
                    HashRest::Rest(Some(name)) => {
                        conds.push(bind(name, call(lvar(&h), "except", syms)))
                    }
                    HashRest::NoRest => conds.push(call(
                        call(lvar(&h), "size", vec![]),
                        "==",
                        vec![Node::Int(pairs.len() as i64)],
                    )),
                    HashRest::None if pairs.is_empty() => {
                        conds.push(call(lvar(&h), "empty?", vec![]))
                    }
                    _ => {}
                }
                and_all(conds)
            }
        }
    }

    /// Tests a sub-pattern against an element, evaluating the element once.
    fn element_test(&mut self, p: &Pat, value: Node) -> Node {
        if let Pat::Bind(n) = p {
            return bind(n, value);
        }
        let e = self.fresh("pe");
        self.declare(&e);
        let test = self.pattern_test(p, lvar(&e));
        Node::Seq(vec![Node::LAsgn(e, b(value)), test])
    }

    /// Conditions that put `v.deconstruct` in `d` (false when there's no hook).
    fn deconstruct(&mut self, konst: &Option<Node>, v: &Node, d: &str) -> Vec<Node> {
        let mut conds = Vec::new();
        if let Some(k) = konst {
            conds.push(call(k.clone(), "===", vec![v.clone()]));
        }
        let fetch = ast::call(None, "__pm_deconstruct", vec![v.clone()]);
        let value = match &self.pm_cache {
            // the case subject: deconstruct once, share it across clauses
            Some((subject, cache, flag)) if matches!(v, Node::LVar(n) if n == subject) => {
                conds.push(Node::Or(
                    b(lvar(flag)),
                    b(Node::Seq(vec![
                        Node::LAsgn(cache.clone(), b(fetch)),
                        Node::LAsgn(flag.clone(), b(Node::True)),
                    ])),
                ));
                lvar(cache)
            }
            _ => fetch,
        };
        conds.push(Node::Seq(vec![
            Node::LAsgn(d.to_string(), b(value)),
            lvar(d),
        ]));
        conds
    }
}

#[allow(clippy::large_enum_variant)] // short-lived parser output
enum Item {
    Pat(Pat),
    Splat(Option<String>),
}

impl Item {
    fn into_pat(self) -> Pat {
        match self {
            Item::Pat(p) => p,
            Item::Splat(_) => unreachable!(),
        }
    }
}

fn no_match(tmp: &str) -> Node {
    ast::call(
        None,
        // lang-fork: `raise` is gone from the language; a value no pattern
        // matches is a panic of this kind (src/core/panic.rs)
        "__raise",
        vec![
            Node::Const("NoMatchingPatternError".into()),
            call(lvar(tmp), "inspect", vec![]),
        ],
    )
}
