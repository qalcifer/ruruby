//! Tokenizer.
//!
//! mruby delegates lexing and parsing to Prism (`mrbgems/mruby-compiler/lib/prism`).
//! ruruby uses this hand-written lexer to stay pure Rust (it must build for wasm).
//!
//! Ruby's lexer is context sensitive: whether `%w`, `?c`, `<<ID` or `-1` start a
//! literal depends on the previous token and on surrounding whitespace. Those
//! decisions are made with [`Lexer::value_before`] and the `spc` flag.

use crate::parser::MAX_DEPTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kw {
    Def,
    End,
    If,
    Elsif,
    Else,
    Unless,
    While,
    Until,
    Do,
    Then,
    Return,
    Break,
    Next,
    Redo,
    Retry,
    Class,
    Module,
    SelfKw,
    Nil,
    True,
    False,
    And,
    Or,
    Not,
    Yield,
    Begin,
    Rescue,
    Ensure,
    Super,
    Case,
    When,
    In,
    For,
    Alias,
    Undef,
    Defined,
}

const KEYWORDS: &[(&str, Kw)] = &[
    ("def", Kw::Def),
    ("end", Kw::End),
    ("if", Kw::If),
    ("elsif", Kw::Elsif),
    ("else", Kw::Else),
    ("unless", Kw::Unless),
    ("while", Kw::While),
    ("until", Kw::Until),
    ("do", Kw::Do),
    ("then", Kw::Then),
    ("return", Kw::Return),
    ("break", Kw::Break),
    ("next", Kw::Next),
    ("redo", Kw::Redo),
    ("retry", Kw::Retry),
    ("class", Kw::Class),
    ("module", Kw::Module),
    ("self", Kw::SelfKw),
    ("nil", Kw::Nil),
    // lang-fork: the spelling of nil
    ("none", Kw::Nil),
    ("true", Kw::True),
    ("false", Kw::False),
    ("and", Kw::And),
    ("or", Kw::Or),
    ("not", Kw::Not),
    ("yield", Kw::Yield),
    ("begin", Kw::Begin),
    ("rescue", Kw::Rescue),
    ("ensure", Kw::Ensure),
    ("super", Kw::Super),
    ("case", Kw::Case),
    ("when", Kw::When),
    ("in", Kw::In),
    ("for", Kw::For),
    ("alias", Kw::Alias),
    ("undef", Kw::Undef),
    ("defined?", Kw::Defined),
];

impl Kw {
    pub fn lookup(s: &str) -> Option<Kw> {
        KEYWORDS.iter().find(|(k, _)| *k == s).map(|(_, kw)| *kw)
    }

    pub fn as_str(self) -> &'static str {
        KEYWORDS
            .iter()
            .find(|(_, k)| *k == self)
            .map(|(s, _)| *s)
            .unwrap()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Tokens of a `#{...}` interpolation (ending with `Eof`), parsed in the
    /// scope of the enclosing code.
    Code(Vec<Token>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(Vec<StrPart>),
    Sym(String),
    /// `:"a#{b}"`
    DSym(Vec<StrPart>),
    /// `%w[]`/`%W[]` (false) or `%i[]`/`%I[]` (true)
    Words(Vec<Vec<StrPart>>, bool),
    /// `/re/flags`, `%r{re}flags`
    Regexp(Vec<StrPart>, String),
    /// `` `cmd` `` (calls the method named `` ` ``)
    XStr(Vec<StrPart>),
    /// `name:` inside hash literals / keyword arguments / keyword parameters.
    Label(String),
    Ident(String),
    Const(String),
    IVar(String),
    CVar(String),
    GVar(String),
    Kw(Kw),
    Plus,
    Minus,
    Star,
    Pow,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Eqq,
    Match,
    NotMatch,
    Lt,
    Le,
    Gt,
    Ge,
    Cmp,
    AndAnd,
    OrOr,
    Bang,
    Tilde,
    Amp,
    Pipe,
    Caret,
    Shl,
    Shr,
    Assign,
    /// `+=`, `||=`, ... carrying the operator without `=`.
    OpAssign(String),
    Dot,
    /// `&.`
    AndDot,
    Dot2,
    Dot3,
    Colon,
    Colon2,
    Question,
    /// `=>`
    Arrow,
    /// `->`
    Lambda,
    Comma,
    Semi,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Whether whitespace preceded the token (Ruby is space-sensitive: `foo -1` vs `foo - 1`).
    pub spc: bool,
}

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    lex_at(src, 1)
}

/// Lexes source that starts at `line` of its file.
pub fn lex_at(src: &str, line: u32) -> Result<Vec<Token>, String> {
    Lexer::new(src, line).run()
}

/// Processes the body of a double-quoted literal (escapes and `#{}`), e.g. a heredoc body.
fn dq_text(text: &str, line: u32) -> Result<Vec<StrPart>, String> {
    let mut lx = Lexer::new(text, line);
    lx.dq_string(None)
}

struct Heredoc {
    tok_index: usize,
    id: String,
    /// `<<~`: strip common indentation
    squiggly: bool,
    /// `<<-` / `<<~`: terminator may be indented
    indented_end: bool,
    raw: bool,
}

struct Lexer {
    /// bytes from `\xNN` / octal escapes, decoded as UTF-8 once a run ends
    bytes: Vec<u8>,
    src: Vec<char>,
    pos: usize,
    line: u32,
    toks: Vec<Token>,
    heredocs: Vec<Heredoc>,
    /// `#{}` nesting, bounded by [`MAX_DEPTH`]
    depth: usize,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || (c as u32) > 0x7f
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || (c as u32) > 0x7f
}

fn closing(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        c => c,
    }
}

impl Lexer {
    fn new(src: &str, line: u32) -> Self {
        Lexer {
            bytes: Vec::new(),
            src: src.chars().collect(),
            pos: 0,
            line,
            toks: Vec::new(),
            heredocs: Vec::new(),
            depth: 0,
        }
    }

    fn peek(&self) -> char {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> char {
        self.src.get(self.pos + n).copied().unwrap_or('\0')
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn bump(&mut self) -> char {
        let c = self.peek();
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
        }
        c
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.peek_at(i) == c)
    }

    fn push(&mut self, tok: Tok, spc: bool) {
        self.toks.push(Token {
            tok,
            line: self.line,
            spc,
        });
    }

    fn last_tok(&self) -> Option<&Tok> {
        self.toks.last().map(|t| &t.tok)
    }

    fn err(&self, msg: &str) -> String {
        format!("line {}: {}", self.line, msg)
    }

    /// Does the previous token end a value? Then `%`, `?`, `<<`, `/`, `[`
    /// are operators; otherwise they start literals.
    fn value_before(&self) -> bool {
        matches!(
            self.last_tok(),
            Some(
                Tok::Int(_)
                    | Tok::Float(_)
                    | Tok::Str(_)
                    | Tok::Sym(_)
                    | Tok::DSym(_)
                    | Tok::Words(..)
                    | Tok::Ident(_)
                    | Tok::Const(_)
                    | Tok::IVar(_)
                    | Tok::CVar(_)
                    | Tok::GVar(_)
                    | Tok::RParen
                    | Tok::RBracket
                    | Tok::RBrace
                    | Tok::Kw(Kw::End | Kw::SelfKw | Kw::Nil | Kw::True | Kw::False)
            )
        )
    }

    /// `foo %w[a]`, `foo ?a`, `foo <<EOS`: an identifier followed by a space
    /// and a literal start is a command call with an argument.
    fn literal_allowed(&self, spc: bool) -> bool {
        if matches!(
            self.last_tok(),
            Some(Tok::Kw(Kw::Def) | Tok::Dot | Tok::AndDot)
        ) {
            return false; // `def /(x)`, `a./(b)`
        }
        !self.value_before()
            || (spc
                && matches!(self.last_tok(), Some(Tok::Ident(_)))
                && !matches!(self.peek_at(1), ' ' | '=' | '\n'))
    }

    fn run(mut self) -> Result<Vec<Token>, String> {
        loop {
            let spc = self.skip_space();
            if self.at_end() || self.at_data_end() {
                self.push(Tok::Eof, spc);
                return Ok(self.toks);
            }
            self.token(spc)?;
        }
    }

    /// Lexes one token (or line break) at a non-blank position.
    fn token(&mut self, spc: bool) -> Result<(), String> {
        {
            let c = self.peek();
            if c == '\n' {
                self.bump();
                self.read_heredoc_bodies()?;
                self.newline();
                return Ok(());
            }
            let tok = if c.is_ascii_digit() {
                self.number()?
            } else if is_ident_start(c) {
                self.ident()
            } else if c == '@' {
                self.bump();
                if self.peek() == '@' {
                    self.bump();
                    Tok::CVar(format!("@@{}", self.read_ident()))
                } else {
                    Tok::IVar(format!("@{}", self.read_ident()))
                }
            } else if c == '$' {
                self.bump();
                let name = if is_ident_start(self.peek()) {
                    self.read_ident()
                } else if self.peek().is_ascii_digit() {
                    // `$1`, `$10`: regexp groups
                    let mut n = String::new();
                    while self.peek().is_ascii_digit() {
                        n.push(self.bump());
                    }
                    n
                } else {
                    self.bump().to_string()
                };
                Tok::GVar(format!("${name}"))
            } else if c == '"' {
                self.bump();
                let parts = self.dq_string(Some('"'))?;
                self.string_label(parts)
            } else if c == '\'' {
                self.bump();
                let s = self.sq_string('\'', '\'')?;
                self.string_label(vec![StrPart::Lit(s)])
            } else if c == '`' {
                self.bump();
                if matches!(
                    self.last_tok(),
                    Some(Tok::Kw(Kw::Def) | Tok::Dot | Tok::AndDot)
                ) {
                    Tok::Ident("`".into()) // `def `(cmd)`
                } else {
                    Tok::XStr(self.dq_string(Some('`'))?)
                }
            } else if c == ':' {
                self.colon()?
            } else if c == '%' && self.literal_allowed(spc) && self.percent_literal_start() {
                self.percent_literal()?
            } else if c == '?' && self.literal_allowed(spc) && self.char_literal_start() {
                self.char_literal()?
            } else if c == '<' && self.peek_at(1) == '<' && self.heredoc_start(spc) {
                self.heredoc()?
            } else if c == '/'
                && self.literal_allowed(spc)
                && !(spc && self.peek_at(1) == ' ' && self.value_before())
            {
                self.bump();
                self.regexp('/', '/')?
            } else if let Some((len, tok)) = self.operator() {
                for _ in 0..len {
                    self.bump();
                }
                tok
            } else {
                return Err(self.err(&format!("unexpected character {c:?}")));
            };
            self.push(tok, spc);
        }
        Ok(())
    }

    fn at_data_end(&self) -> bool {
        (self.pos == 0 || self.src[self.pos - 1] == '\n')
            && self.starts_with("__END__")
            && matches!(self.peek_at(7), '\n' | '\r' | '\0')
    }

    /// Skips blanks, comments, `\`-continued lines and `=begin/=end` blocks.
    fn skip_space(&mut self) -> bool {
        let mut spc = false;
        loop {
            match self.peek() {
                ' ' | '\t' | '\r' | '\x0c' | '\x0b' => {
                    self.bump();
                    spc = true;
                }
                '\\' if self.peek_at(1) == '\n' => {
                    self.bump();
                    self.bump();
                    spc = true;
                }
                '\\' if self.peek_at(1) == '\r' && self.peek_at(2) == '\n' => {
                    self.bump();
                    self.bump();
                    self.bump();
                    spc = true;
                }
                '#' => {
                    while !matches!(self.peek(), '\n' | '\0') {
                        self.bump();
                    }
                }
                '=' if (self.pos == 0 || self.src[self.pos - 1] == '\n')
                    && self.starts_with("=begin") =>
                {
                    loop {
                        while !matches!(self.peek(), '\n' | '\0') {
                            self.bump();
                        }
                        if self.at_end() {
                            break;
                        }
                        self.bump();
                        if self.starts_with("=end") {
                            while !matches!(self.peek(), '\n' | '\0') {
                                self.bump();
                            }
                            break;
                        }
                    }
                }
                _ => return spc,
            }
        }
    }

    fn newline(&mut self) {
        if matches!(self.last_tok(), None | Some(Tok::Newline)) {
            return;
        }
        // A line starting with `.method` / `&.method` continues the previous expression.
        let mut i = self.pos;
        loop {
            while matches!(self.src.get(i), Some(' ' | '\t' | '\r' | '\n')) {
                i += 1;
            }
            if self.src.get(i) == Some(&'#') {
                while !matches!(self.src.get(i), Some('\n') | None) {
                    i += 1;
                }
                continue;
            }
            break;
        }
        let next = self.src.get(i).copied();
        let next2 = self.src.get(i + 1).copied();
        if (next == Some('.') && next2 != Some('.')) || (next == Some('&') && next2 == Some('.')) {
            return;
        }
        self.push(Tok::Newline, false);
    }

    fn number(&mut self) -> Result<Tok, String> {
        if self.peek() == '0'
            && matches!(
                self.peek_at(1),
                'x' | 'X' | 'b' | 'B' | 'o' | 'O' | 'd' | 'D'
            )
        {
            self.bump();
            let radix = match self.bump().to_ascii_lowercase() {
                'x' => 16,
                'b' => 2,
                'd' => 10,
                _ => 8,
            };
            let mut s = String::new();
            while self.peek().is_ascii_alphanumeric() || self.peek() == '_' {
                let c = self.bump();
                if c != '_' {
                    s.push(c);
                }
            }
            return i64::from_str_radix(&s, radix)
                .map(Tok::Int)
                .map_err(|_| self.err("bad numeric literal"));
        }
        if self.peek() == '0'
            && (self.peek_at(1).is_ascii_digit()
                || (self.peek_at(1) == '_' && self.peek_at(2).is_ascii_digit()))
        {
            let mut s = String::new();
            while self.peek().is_ascii_digit() || self.peek() == '_' {
                let c = self.bump();
                if c != '_' {
                    s.push(c);
                }
            }
            return i64::from_str_radix(&s, 8)
                .map(Tok::Int)
                .map_err(|_| self.err("bad octal literal"));
        }
        let mut s = String::new();
        let mut is_float = false;
        self.digits(&mut s);
        if self.peek() == '.' && self.peek_at(1).is_ascii_digit() {
            is_float = true;
            s.push(self.bump());
            self.digits(&mut s);
        }
        if matches!(self.peek(), 'e' | 'E')
            && (self.peek_at(1).is_ascii_digit()
                || (matches!(self.peek_at(1), '+' | '-') && self.peek_at(2).is_ascii_digit()))
        {
            is_float = true;
            s.push(self.bump());
            if matches!(self.peek(), '+' | '-') {
                s.push(self.bump());
            }
            self.digits(&mut s);
        }
        if matches!(self.peek(), 'r' | 'i') && !is_ident_char(self.peek_at(1)) {
            return Err(self.err("rational/imaginary literals are not supported yet"));
        }
        if is_float {
            s.parse()
                .map(Tok::Float)
                .map_err(|_| self.err("bad float literal"))
        } else {
            match s.parse() {
                Ok(i) => Ok(Tok::Int(i)),
                // no Bignum yet: keep the magnitude as a Float
                Err(_) => s
                    .parse()
                    .map(Tok::Float)
                    .map_err(|_| self.err("bad integer literal")),
            }
        }
    }

    fn digits(&mut self, s: &mut String) {
        while self.peek().is_ascii_digit()
            || (self.peek() == '_' && self.peek_at(1).is_ascii_digit())
        {
            let c = self.bump();
            if c != '_' {
                s.push(c);
            }
        }
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while is_ident_char(self.peek()) {
            s.push(self.bump());
        }
        s
    }

    fn ident(&mut self) -> Tok {
        let mut s = self.read_ident();
        if matches!(self.peek(), '?' | '!') && self.peek_at(1) != '=' {
            s.push(self.bump());
        } else if matches!(self.peek(), '?' | '!')
            && self.peek_at(1) == '='
            && self.peek_at(2) == '='
        {
            s.push(self.bump()); // `a!==b`
        }
        let after_dot = matches!(self.last_tok(), Some(Tok::Dot | Tok::AndDot));
        let after_def = matches!(self.last_tok(), Some(Tok::Kw(Kw::Def)));
        if self.peek() == ':'
            && self.peek_at(1) != ':'
            && self.last_tok() != Some(&Tok::Question)
            && !after_dot
            && !s.ends_with('?')
        {
            self.bump();
            return Tok::Label(s);
        }
        if s.starts_with(|c: char| c.is_uppercase()) {
            return Tok::Const(s);
        }
        match Kw::lookup(&s) {
            // After a dot or `def`, keywords are ordinary method names: `obj.class`, `def end`.
            Some(kw) if !after_dot && !(after_def && kw != Kw::SelfKw && kw != Kw::Nil) => {
                Tok::Kw(kw)
            }
            _ => Tok::Ident(s),
        }
    }

    /// `"str": value` in hash literals.
    fn string_label(&mut self, parts: Vec<StrPart>) -> Tok {
        if self.peek() == ':'
            && self.peek_at(1) != ':'
            && self.last_tok() != Some(&Tok::Question)
            && let [StrPart::Lit(s)] = parts.as_slice()
        {
            let s = s.clone();
            self.bump();
            return Tok::Label(s);
        }
        Tok::Str(parts)
    }

    fn colon(&mut self) -> Result<Tok, String> {
        if self.peek_at(1) == ':' {
            self.bump();
            self.bump();
            return Ok(Tok::Colon2);
        }
        if self.peek_at(1) == '"' {
            self.bump();
            self.bump();
            let parts = self.dq_string(Some('"'))?;
            return Ok(match parts.as_slice() {
                [StrPart::Lit(s)] => Tok::Sym(s.clone()),
                _ => Tok::DSym(parts),
            });
        }
        if self.peek_at(1) == '\'' {
            self.bump();
            self.bump();
            return Ok(Tok::Sym(self.sq_string('\'', '\'')?));
        }
        if is_ident_start(self.peek_at(1)) {
            self.bump();
            let mut s = self.read_ident();
            if matches!(self.peek(), '?' | '!' | '=') && !matches!(self.peek_at(1), '=' | '>' | '~')
            {
                s.push(self.bump());
            }
            return Ok(Tok::Sym(s));
        }
        if matches!(self.peek_at(1), '@' | '$') {
            self.bump();
            let mut s = String::new();
            while matches!(self.peek(), '@' | '$') {
                s.push(self.bump());
            }
            s.push_str(&self.read_ident());
            return Ok(Tok::Sym(s));
        }
        const OP_SYMS: &[&str] = &[
            "[]=", "[]", "<=>", "===", "==", "=~", "!=", "!~", "<=", ">=", "<<", ">>", "**", "+@",
            "-@", "+", "-", "*", "/", "%", "<", ">", "!", "&", "|", "^", "~",
        ];
        for op in OP_SYMS {
            if op
                .chars()
                .enumerate()
                .all(|(i, c)| self.peek_at(i + 1) == c)
            {
                for _ in 0..=op.len() {
                    self.bump();
                }
                return Ok(Tok::Sym(op.to_string()));
            }
        }
        self.bump();
        Ok(Tok::Colon)
    }

    fn operator(&self) -> Option<(usize, Tok)> {
        const OP_ASSIGN: &[&str] = &[
            "**=", "||=", "&&=", "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=",
        ];
        for op in OP_ASSIGN {
            if self.starts_with(op) {
                return Some((op.len(), Tok::OpAssign(op[..op.len() - 1].to_string())));
            }
        }
        let table = [
            ("...", Tok::Dot3),
            ("..", Tok::Dot2),
            ("===", Tok::Eqq),
            ("==", Tok::EqEq),
            ("=~", Tok::Match),
            ("=>", Tok::Arrow),
            ("<=>", Tok::Cmp),
            ("<=", Tok::Le),
            ("<<", Tok::Shl),
            (">=", Tok::Ge),
            (">>", Tok::Shr),
            ("!=", Tok::NotEq),
            ("!~", Tok::NotMatch),
            ("&&", Tok::AndAnd),
            ("&.", Tok::AndDot),
            ("||", Tok::OrOr),
            ("**", Tok::Pow),
            ("->", Tok::Lambda),
            ("+", Tok::Plus),
            ("-", Tok::Minus),
            ("*", Tok::Star),
            ("/", Tok::Slash),
            ("%", Tok::Percent),
            ("=", Tok::Assign),
            ("<", Tok::Lt),
            (">", Tok::Gt),
            ("!", Tok::Bang),
            ("~", Tok::Tilde),
            ("&", Tok::Amp),
            ("|", Tok::Pipe),
            ("^", Tok::Caret),
            (".", Tok::Dot),
            (",", Tok::Comma),
            (";", Tok::Semi),
            ("(", Tok::LParen),
            (")", Tok::RParen),
            ("[", Tok::LBracket),
            ("]", Tok::RBracket),
            ("{", Tok::LBrace),
            ("}", Tok::RBrace),
            ("?", Tok::Question),
        ];
        table
            .into_iter()
            .find(|(s, _)| self.starts_with(s))
            .map(|(s, t)| (s.len(), t))
    }

    // ---- string literals -----------------------------------------------

    /// Appends pending escaped bytes to `out` as UTF-8.
    fn flush_bytes(&mut self, out: &mut String) {
        if !self.bytes.is_empty() {
            out.push_str(&String::from_utf8_lossy(&self.bytes));
            self.bytes.clear();
        }
    }

    /// Reads one escape after a backslash in a double-quoted context.
    fn escape(&mut self, out: &mut String) -> Result<(), String> {
        let e = self.bump();
        if !matches!(e, 'x' | '0'..='7') {
            self.flush_bytes(out);
        }
        match e {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            's' => out.push(' '),
            'r' => out.push('\r'),
            'e' => out.push('\x1b'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'v' => out.push('\x0b'),
            '\n' => {}
            '0'..='7' => {
                let mut n = e.to_digit(8).unwrap();
                for _ in 0..2 {
                    match self.peek().to_digit(8) {
                        Some(d) => {
                            n = n * 8 + d;
                            self.bump();
                        }
                        None => break,
                    }
                }
                self.bytes.push(n as u8);
            }
            'x' => {
                let mut n = 0;
                let mut digits = 0;
                while digits < 2 {
                    match self.peek().to_digit(16) {
                        Some(d) => {
                            n = n * 16 + d;
                            self.bump();
                            digits += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    return Err(self.err("invalid hex escape"));
                }
                self.bytes.push(n as u8);
            }
            'u' => {
                let mut codes = Vec::new();
                if self.peek() == '{' {
                    self.bump();
                    loop {
                        while self.peek() == ' ' {
                            self.bump();
                        }
                        if self.peek() == '}' {
                            self.bump();
                            break;
                        }
                        let mut hex = String::new();
                        while self.peek().is_ascii_hexdigit() {
                            hex.push(self.bump());
                        }
                        if hex.is_empty() {
                            return Err(self.err("invalid Unicode escape"));
                        }
                        codes.push(hex);
                    }
                } else {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        hex.push(self.bump());
                    }
                    codes.push(hex);
                }
                for hex in codes {
                    let ch = u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| self.err("invalid Unicode escape"))?;
                    out.push(ch);
                }
            }
            '\0' if self.at_end() => return Err(self.err("unterminated string")),
            other => out.push(other),
        }
        Ok(())
    }

    /// Double-quoted string body (opening quote already consumed). With
    /// `close = None` it reads to the end of input (heredoc bodies).
    fn dq_string(&mut self, close: Option<char>) -> Result<Vec<StrPart>, String> {
        self.dq_string_nested(close, None)
    }

    fn dq_string_nested(
        &mut self,
        close: Option<char>,
        open: Option<char>,
    ) -> Result<Vec<StrPart>, String> {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut depth = 0;
        loop {
            if self.at_end() {
                if close.is_none() {
                    break;
                }
                return Err(self.err("unterminated string"));
            }
            let c = self.peek();
            if Some(c) == open && open != close {
                depth += 1;
            } else if Some(c) == close {
                if depth == 0 {
                    self.bump();
                    break;
                }
                depth -= 1;
            }
            if c != '\\' {
                self.flush_bytes(&mut cur);
            }
            match c {
                '\\' => {
                    self.bump();
                    self.escape(&mut cur)?;
                }
                '#' if self.peek_at(1) == '{' => {
                    self.bump();
                    self.bump();
                    if !cur.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                    }
                    parts.push(StrPart::Code(self.interpolation()?));
                }
                '#' if matches!(self.peek_at(1), '@' | '$')
                    && (is_ident_start(self.peek_at(2))
                        || (self.peek_at(1) == '@'
                            && self.peek_at(2) == '@'
                            && is_ident_start(self.peek_at(3)))) =>
                {
                    // "#@ivar", "#@@cvar", "#$gvar"
                    self.bump();
                    if !cur.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                    }
                    let mut name = String::new();
                    while matches!(self.peek(), '@' | '$') {
                        name.push(self.bump());
                    }
                    name.push_str(&self.read_ident());
                    let tok = match name.as_bytes() {
                        [b'$', ..] => Tok::GVar(name),
                        [b'@', b'@', ..] => Tok::CVar(name),
                        _ => Tok::IVar(name),
                    };
                    let line = self.line;
                    parts.push(StrPart::Code(vec![
                        Token { tok, line, spc: false },
                        Token { tok: Tok::Eof, line, spc: false },
                    ]));
                }
                _ => cur.push(self.bump()),
            }
        }
        self.flush_bytes(&mut cur);
        if !cur.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        Ok(parts)
    }

    /// Tokens of `#{ ... }` up to the matching brace, lexed by the main
    /// tokenizer so anything valid in code is valid here.
    fn interpolation(&mut self) -> Result<Vec<Token>, String> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(&format!("nesting too deep (limit {MAX_DEPTH})")));
        }
        let outer = std::mem::take(&mut self.toks);
        let outer_heredocs = std::mem::take(&mut self.heredocs);
        self.depth += 1;
        let r = self.interpolation_tokens();
        self.depth -= 1;
        let started_heredoc = !std::mem::replace(&mut self.heredocs, outer_heredocs).is_empty();
        let toks = std::mem::replace(&mut self.toks, outer);
        r?;
        if started_heredoc {
            return Err(self.err("a heredoc can't start inside string interpolation"));
        }
        Ok(toks)
    }

    fn interpolation_tokens(&mut self) -> Result<(), String> {
        let mut depth = 0usize;
        loop {
            let spc = self.skip_space();
            if self.at_end() {
                return Err(self.err("unterminated string interpolation"));
            }
            if self.peek() == '}' && depth == 0 {
                self.bump();
                self.push(Tok::Eof, spc);
                return Ok(());
            }
            self.token(spc)?;
            match self.last_tok() {
                Some(Tok::LBrace) => depth += 1,
                Some(Tok::RBrace) => depth -= 1,
                _ => {}
            }
        }
    }

    /// Single-quoted body up to `close` (only `\\` and `\<close>` are escapes).
    fn sq_string(&mut self, open: char, close: char) -> Result<String, String> {
        let mut s = String::new();
        let mut depth = 0;
        loop {
            if self.at_end() {
                return Err(self.err("unterminated string"));
            }
            let c = self.bump();
            if c == '\\' && (self.peek() == '\\' || self.peek() == close || self.peek() == open) {
                s.push(self.bump());
            } else if c == open && open != close {
                depth += 1;
                s.push(c);
            } else if c == close {
                if depth == 0 {
                    return Ok(s);
                }
                depth -= 1;
                s.push(c);
            } else {
                s.push(c);
            }
        }
    }

    /// A regexp body (opening delimiter consumed): escapes are kept for the
    /// regexp engine, only `#{}` is processed here. Then the option letters.
    fn regexp(&mut self, open: char, close: char) -> Result<Tok, String> {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut depth = 0;
        loop {
            if self.at_end() {
                return Err(self.err("unterminated regexp"));
            }
            let c = self.bump();
            match c {
                '\\' => {
                    let n = self.bump();
                    if n == close || (n == open && open != close) {
                        if close != '/' {
                            cur.push('\\');
                        }
                        cur.push(n);
                    } else {
                        cur.push('\\');
                        cur.push(n);
                    }
                }
                '#' if self.peek() == '{' => {
                    self.bump();
                    if !cur.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                    }
                    parts.push(StrPart::Code(self.interpolation()?));
                }
                c if c == open && open != close => {
                    depth += 1;
                    cur.push(c);
                }
                c if c == close => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    cur.push(c);
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        let mut flags = String::new();
        while matches!(self.peek(), 'i' | 'm' | 'x' | 'o' | 'u' | 'n' | 'e' | 's') {
            flags.push(self.bump());
        }
        Ok(Tok::Regexp(parts, flags))
    }

    // ---- %-literals, ?c, heredocs ---------------------------------------

    fn percent_literal_start(&self) -> bool {
        let c1 = self.peek_at(1);
        let delim = |c: char| !c.is_alphanumeric() && !c.is_whitespace() && c != '\0' && c != '=';
        (matches!(c1, 'w' | 'W' | 'i' | 'I' | 'q' | 'Q' | 's' | 'r') && delim(self.peek_at(2)))
            || matches!(c1, '(' | '[' | '{' | '<' | '|' | '!')
    }

    fn percent_literal(&mut self) -> Result<Tok, String> {
        self.bump(); // %
        let kind = if self.peek().is_alphabetic() {
            self.bump()
        } else {
            'Q'
        };
        let open = self.bump();
        let close = closing(open);
        match kind {
            'q' => Ok(Tok::Str(vec![StrPart::Lit(self.sq_string(open, close)?)])),
            'r' => self.regexp(open, close),
            'Q' => Ok(Tok::Str(self.dq_string_nested(Some(close), Some(open))?)),
            's' => Ok(Tok::Sym(self.sq_string(open, close)?)),
            'w' | 'W' | 'i' | 'I' => {
                let raw = self.sq_string_keep_escapes(open, close)?;
                let interpolate = kind.is_uppercase();
                let mut words = Vec::new();
                for w in split_words(&raw) {
                    words.push(if interpolate {
                        dq_text(&w.replace("\\\n", "\n"), self.line)?
                    } else {
                        vec![StrPart::Lit(unescape_word(&w, open, close))]
                    });
                }
                Ok(Tok::Words(words, kind.eq_ignore_ascii_case(&'i')))
            }
            _ => Err(self.err(&format!("%{kind} literals are not supported"))),
        }
    }

    /// Body of a %-literal with escapes kept as written (handled per word later).
    fn sq_string_keep_escapes(&mut self, open: char, close: char) -> Result<String, String> {
        let mut s = String::new();
        let mut depth = 0;
        loop {
            if self.at_end() {
                return Err(self.err("unterminated literal"));
            }
            let c = self.bump();
            if c == '\\' {
                s.push(c);
                s.push(self.bump());
            } else if c == open && open != close {
                depth += 1;
                s.push(c);
            } else if c == close {
                if depth == 0 {
                    return Ok(s);
                }
                depth -= 1;
                s.push(c);
            } else {
                s.push(c);
            }
        }
    }

    fn char_literal_start(&self) -> bool {
        let c = self.peek_at(1);
        if c == '\\' {
            return true;
        }
        !c.is_whitespace() && c != '\0' && !(is_ident_char(c) && is_ident_char(self.peek_at(2)))
    }

    fn char_literal(&mut self) -> Result<Tok, String> {
        self.bump(); // ?
        let mut s = String::new();
        if self.peek() == '\\' {
            self.bump();
            self.escape(&mut s)?;
        } else {
            s.push(self.bump());
        }
        Ok(Tok::Str(vec![StrPart::Lit(s)]))
    }

    fn heredoc_start(&self, spc: bool) -> bool {
        if matches!(
            self.last_tok(),
            Some(Tok::Kw(Kw::Class | Kw::Def) | Tok::Dot | Tok::AndDot)
        ) {
            return false; // `class << obj`
        }
        let c2 = self.peek_at(2);
        let id_start = |c: char| is_ident_start(c) || c == '"' || c == '\'';
        if matches!(c2, '~' | '-') {
            return id_start(self.peek_at(3)) && (!self.value_before() || spc);
        }
        if !self.value_before() {
            return id_start(c2);
        }
        // `write <<EOS` is a command call with a heredoc argument, `a <<b` a
        // shift: the lexer doesn't know locals, so require an uppercase or
        // quoted terminator after an identifier
        (c2.is_ascii_uppercase() || c2 == '"' || c2 == '\'') && self.literal_allowed(spc)
    }

    fn heredoc(&mut self) -> Result<Tok, String> {
        self.bump();
        self.bump();
        let (mut squiggly, mut indented_end) = (false, false);
        match self.peek() {
            '~' => {
                squiggly = true;
                indented_end = true;
                self.bump();
            }
            '-' => {
                indented_end = true;
                self.bump();
            }
            _ => {}
        }
        let (id, raw) = match self.peek() {
            q @ ('"' | '\'') => {
                self.bump();
                let mut id = String::new();
                while !self.at_end() && self.peek() != q {
                    id.push(self.bump());
                }
                self.bump();
                (id, q == '\'')
            }
            _ => (self.read_ident(), false),
        };
        self.heredocs.push(Heredoc {
            tok_index: self.toks.len(),
            id,
            squiggly,
            indented_end,
            raw,
        });
        Ok(Tok::Str(vec![]))
    }

    /// After a newline: read the bodies of heredocs started on the previous line.
    fn read_heredoc_bodies(&mut self) -> Result<(), String> {
        for h in std::mem::take(&mut self.heredocs) {
            let start_line = self.line;
            let mut lines = Vec::new();
            loop {
                if self.at_end() {
                    return Err(self.err(&format!(
                        "can't find string \"{}\" anywhere before EOF",
                        h.id
                    )));
                }
                let mut line = String::new();
                while !matches!(self.peek(), '\n' | '\0') || (self.peek() == '\0' && !self.at_end())
                {
                    line.push(self.bump());
                }
                if !self.at_end() {
                    self.bump();
                }
                let trimmed = line.trim_end_matches('\r');
                let candidate = if h.indented_end {
                    trimmed.trim_start()
                } else {
                    trimmed
                };
                if candidate == h.id {
                    break;
                }
                lines.push(line);
            }
            if h.squiggly {
                let indent = lines
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
                    .min()
                    .unwrap_or(0);
                for l in &mut lines {
                    *l = l
                        .chars()
                        .skip(indent.min(l.chars().take_while(|c| c.is_whitespace()).count()))
                        .collect();
                }
            }
            let mut body = String::new();
            for l in &lines {
                body.push_str(l);
                body.push('\n');
            }
            let parts = if h.raw {
                vec![StrPart::Lit(body)]
            } else {
                dq_text(&body, start_line)?
            };
            self.toks[h.tok_index].tok = Tok::Str(parts);
        }
        Ok(())
    }
}

/// `%w`/`%i` words: a backslash escapes whitespace, itself and the delimiters.
fn unescape_word(w: &str, open: char, close: char) -> String {
    let mut out = String::new();
    let mut chars = w.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n) if n.is_whitespace() || n == '\\' || n == open || n == close => out.push(n),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits the body of `%w[...]` on unescaped whitespace.
fn split_words(raw: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut interp = 0; // inside `#{...}` whitespace doesn't split
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
        } else if c == '#' && chars.peek() == Some(&'{') {
            interp += 1;
            cur.push(c);
            cur.push(chars.next().unwrap());
        } else if interp > 0 && c == '{' {
            interp += 1;
            cur.push(c);
        } else if interp > 0 && c == '}' {
            interp -= 1;
            cur.push(c);
        } else if c.is_whitespace() && interp == 0 {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}
