//! Abstract syntax tree. In mruby this is Prism's `pm_node_t` tree.
//!
//! As in Prism, local variables are resolved by the parser: every scope
//! (`Program`, `Def`, `Class`, `SClass`, `Block`) carries its local-variable
//! table, parameters first, and the code generator turns each name into a
//! register index.

#[derive(Debug, Clone)]
pub enum Node {
    Nil,
    True,
    False,
    SelfRef,
    Int(i64),
    Float(f64),
    Str(String),
    /// Interpolated string: `"a#{b}c"`.
    DStr(Vec<Node>),
    Sym(String),
    /// Interpolated symbol: `:"a#{b}"`.
    DSym(Vec<Node>),
    /// Elements may be `Splat`.
    Array(Vec<Node>),
    /// Hash literal; the flag marks keyword arguments (`f(a: 1)`).
    Hash(Vec<HashItem>, bool),
    /// `lo..hi`; `Nil` for beginless/endless ranges.
    Range(Box<Node>, Box<Node>, bool),
    LVar(String),
    IVar(String),
    GVar(String),
    CVar(String),
    Const(String),
    /// `Outer::Name`
    Colon2(Box<Node>, String),
    /// `::Name`
    Colon3(String),
    LAsgn(String, Box<Node>),
    IAsgn(String, Box<Node>),
    GAsgn(String, Box<Node>),
    CVAsgn(String, Box<Node>),
    /// Constant assignment; the target is `Const`, `Colon2` or `Colon3`.
    CAsgn(Box<Node>, Box<Node>),
    /// `a, (b, *c), d.e = rhs` — rhs is an `Array` when several values are given.
    MAsgn(MLhs, Box<Node>),
    Call {
        recv: Option<Box<Node>>,
        name: String,
        /// May contain `Splat` and a trailing keyword `Hash`.
        args: Vec<Node>,
        block: Option<Box<Block>>,
        /// `&blk` / `&:sym`
        block_arg: Option<Box<Node>>,
        /// `recv&.name`
        safe: bool,
    },
    /// `super` (args None = implicit, like `zsuper`) / `super(...)`
    Super {
        args: Option<Vec<Node>>,
        block: Option<Box<Block>>,
        block_arg: Option<Box<Node>>,
    },
    Yield(Vec<Node>),
    BlockGiven,
    /// `*expr` in argument lists, array literals and `when` clauses.
    Splat(Box<Node>),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    If(Box<Node>, Box<Node>, Box<Node>),
    While {
        cond: Box<Node>,
        body: Box<Node>,
        until: bool,
        /// `begin ... end while cond` runs the body first.
        do_while: bool,
    },
    Seq(Vec<Node>),
    Def {
        /// `def recv.name` (singleton method)
        recv: Option<Box<Node>>,
        name: String,
        /// boxed: `Params` is large and would make every `Node` large
        params: Box<Params>,
        body: Box<Node>,
        locals: Vec<String>,
    },
    Class {
        /// `Const`, `Colon2` or `Colon3`
        path: Box<Node>,
        superclass: Option<Box<Node>>,
        body: Box<Node>,
        locals: Vec<String>,
        is_module: bool,
    },
    /// `class << obj; body; end`
    SClass(Box<Node>, Box<Node>, Vec<String>),
    /// lang-fork `impl I for C { body }`: runs the body with the class the
    /// first node evaluates to as self and method target.
    ClassBody(Box<Node>, Box<Node>, Vec<String>),
    Return(Option<Box<Node>>),
    /// `expr?` (lang-fork): `Err(e)` and `none` return from the method,
    /// `Ok(v)` gives `v`, anything else itself.
    Try(Box<Node>),
    Break(Option<Box<Node>>),
    Next(Option<Box<Node>>),
    Redo,
    Retry,
    Begin {
        body: Box<Node>,
        rescues: Vec<Rescue>,
        else_body: Option<Box<Node>>,
        ensure: Option<Box<Node>>,
    },
    Lambda(Box<Block>),
    /// lang-fork: `|x| x + 1`, `|x| { ... }`, `|| ...`: a lambda; as the last
    /// argument of a call it is the call's block instead.
    Closure(Box<Block>),
    Defined(Box<Node>),
    Alias(String, String),
    /// `use models::user::User`, `use shapes::{Circle, Square}`, `use util::*`
    /// (items `None`), `use geometry::Shapes::Circle`: the file
    /// `models/user.rb` under the program's root directory (`path`), the
    /// modules/classes inside it to go into (`inner`, here `Shapes`), and the
    /// constants this file imports from there.
    Use {
        path: Vec<String>,
        inner: Vec<String>,
        items: Option<Vec<String>>,
        line: u32,
    },
    GAlias(String, String),
    Undef(Vec<String>),
    /// `__method__`
    CurrentMethod,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // short-lived parser output
pub enum HashItem {
    Pair(Node, Node),
    /// `**h`
    Splat(Node),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub params: Params,
    pub body: Node,
    pub locals: Vec<String>,
    /// Desugared `for` loop: the body starts with the loop-variable
    /// assignment, which `redo` must not repeat.
    pub for_loop: bool,
}

/// Parameter list (mruby: the `aspec` of OP_ENTER). Registers are assigned in
/// this order: required, optional, rest, post, keywords, kwrest, block.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub required: Vec<ParamTarget>,
    pub optional: Vec<(String, Node)>,
    pub rest: Option<String>,
    pub post: Vec<ParamTarget>,
    /// `k:` (None) / `k: default`
    pub kw: Vec<(String, Option<Node>)>,
    pub kwrest: Option<String>,
    pub block: Option<String>,
    /// `|a, |` — a trailing comma makes a block splat its single array argument.
    pub trailing_comma: bool,
}

impl Params {
    /// Local names in register order.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.required.iter().map(|p| p.name().to_string()).collect();
        v.extend(self.optional.iter().map(|(n, _)| n.clone()));
        v.extend(self.rest.clone());
        v.extend(self.post.iter().map(|p| p.name().to_string()));
        v.extend(self.kw.iter().map(|(n, _)| n.clone()));
        v.extend(self.kwrest.clone());
        v.extend(self.block.clone());
        v
    }

    pub fn is_empty(&self) -> bool {
        self.names().is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum ParamTarget {
    Name(String),
    /// `(a, b)` in a parameter list: a hidden local, destructured on entry.
    Destructure(String, MLhs),
}

impl ParamTarget {
    pub fn name(&self) -> &str {
        match self {
            ParamTarget::Name(n) | ParamTarget::Destructure(n, _) => n,
        }
    }
}

/// Left-hand side of a multiple assignment.
#[derive(Debug, Clone, Default)]
pub struct MLhs {
    pub pre: Vec<MTarget>,
    /// `*a` (Some(Some)), bare `*` (Some(None))
    pub splat: Option<Option<Box<MTarget>>>,
    pub post: Vec<MTarget>,
}

#[derive(Debug, Clone)]
pub enum MTarget {
    /// A variable, constant, `recv.attr` or `recv[index]` (in its read form).
    Var(Node),
    Nested(MLhs),
}

#[derive(Debug, Clone)]
pub struct Rescue {
    pub classes: Vec<Node>,
    pub var: Option<Box<Node>>,
    pub body: Node,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub body: Node,
    pub locals: Vec<String>,
    /// The file it was parsed from (`-` for stdin, `-e`): `require_relative`
    /// resolves against it.
    pub file: String,
}

pub fn call(recv: Option<Node>, name: &str, args: Vec<Node>) -> Node {
    Node::Call {
        recv: recv.map(Box::new),
        name: name.to_string(),
        args,
        block: None,
        block_arg: None,
        safe: false,
    }
}
