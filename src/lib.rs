//! ruruby — a Ruby interpreter in Rust, structured after [mruby](https://github.com/mruby/mruby).
//!
//! Pipeline (same as mruby):
//!
//! ```text
//! source ─lexer/parser─▶ AST ─codegen─▶ Irep (register bytecode) ─vm─▶ result
//! ```
//!
//! | ruruby module | mruby counterpart |
//! |---|---|
//! | `lexer`, `parser`, `ast` | Prism (`mrbgems/mruby-compiler/lib/prism`) |
//! | `codegen` | `mrbgems/mruby-compiler/src/codegen.c` |
//! | `opcode` | `include/mruby/ops.h`, `irep.h`, `src/codedump.c` |
//! | `vm` | `src/vm.c`, `state.c`, `class.c`, `error.c` |
//! | `value` | `include/mruby/value.h`, `object.h`, `class.h`, `proc.h` |
//! | `symbol` | `src/symbol.c` |
//! | `core::*` | `src/{kernel,numeric,string,array,hash,range}.c`, `mrblib/*.rb`, default mrbgems |

pub mod ast;
pub mod codegen;
pub mod core;
pub mod gc;
pub mod lexer;
pub mod modules;
pub mod opcode;
pub mod parser;
pub mod reactive;
pub mod rhash;
pub mod symbol;
pub mod value;
pub mod vm;

use opcode::Irep;
use std::fmt;
use std::rc::Rc;
use value::Value;
use vm::Vm;

#[derive(Debug)]
pub enum Error {
    Syntax(String),
    Compile(String),
    /// Uncaught Ruby exception.
    Runtime {
        class: String,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax(m) => write!(f, "{m} (SyntaxError)"),
            Error::Compile(m) => write!(f, "{m} (SyntaxError)"),
            Error::Runtime { class, message } => write!(f, "{message} ({class})"),
        }
    }
}

impl std::error::Error for Error {}

/// An interpreter instance (mruby: `mrb_state` from `mrb_open()`).
/// Classes, methods and globals persist across `eval` calls.
pub struct Interpreter {
    pub vm: Vm,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter { vm: Vm::new() }
    }

    /// Parses and compiles to bytecode (mruby: `mrbc`).
    pub fn compile(&mut self, src: &str) -> Result<Rc<Irep>, Error> {
        self.compile_file(src, "-", 1)
    }

    /// Compiles source from `file` that starts at `line` (used by `__FILE__`/`__LINE__`).
    pub fn compile_file(&mut self, src: &str, file: &str, line: u32) -> Result<Rc<Irep>, Error> {
        let prog = parser::parse_file(src, file, line).map_err(Error::Syntax)?;
        modules::compile_program(&mut self.vm, &prog).map_err(Error::Compile)
    }

    pub fn run(&mut self, irep: Rc<Irep>) -> Result<Value, Error> {
        self.vm.run_main(irep).map_err(|e| Error::Runtime {
            class: self.vm.class_name(&e),
            message: self.vm.exc_message(&e),
        })
    }

    /// Compiles and runs source code (mruby: `mrb_load_string`).
    pub fn eval(&mut self, src: &str) -> Result<Value, Error> {
        let irep = self.compile(src)?;
        self.run(irep)
    }

    /// Runs the `at_exit` / `END { }` blocks, most recent first.
    pub fn run_at_exit(&mut self) -> Result<(), Error> {
        while let Some(b) = self.vm.at_exit.pop() {
            let r = self.vm.call_block(&b, vec![]).map_err(|u| self.vm.materialize(u));
            r.map_err(|u| match u {
                vm::Unwind::Error(e) => Error::Runtime {
                    class: self.vm.class_name(&e),
                    message: self.vm.exc_message(&e),
                },
                _ => Error::Runtime {
                    class: "LocalJumpError".into(),
                    message: "unexpected jump in at_exit".into(),
                },
            })?;
        }
        Ok(())
    }

    /// `value.inspect` as a Rust string.
    pub fn inspect(&mut self, v: &Value) -> String {
        self.vm.inspect(v).unwrap_or_else(|_| "#<?>".into())
    }

    /// Redirects `puts`/`print`/`p` into a buffer (see [`Interpreter::take_output`]).
    pub fn capture_output(&mut self) {
        self.vm.capture = Some(String::new());
    }

    pub fn take_output(&mut self) -> String {
        self.vm
            .capture
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

/// Runs `src` in a fresh interpreter and returns what it printed.
pub fn run_capture(src: &str) -> Result<String, Error> {
    let mut interp = Interpreter::new();
    interp.capture_output();
    interp.eval(src)?;
    Ok(interp.take_output())
}
