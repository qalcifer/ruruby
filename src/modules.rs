//! Files as modules: `use path::to::file::Item` (lang-fork).
//!
//! Like Rust, `use` is resolved before the program runs: compiling a file
//! follows its `use` statements, parses and compiles every file they name
//! (once per VM), rejects cycles, and checks that the imported constants are
//! defined at the top level of the used file. Errors are compile errors.
//!
//! Unlike Rust, a file's top level is code (class bodies run), so a used
//! file still runs — once, the first time a `use` of it executes, in a
//! namespace module of its own (`Op::Use`). Its classes and constants live
//! in that namespace instead of `Object`; `use` copies the imported ones into
//! the namespace of the file that uses it. Top-level `def`s stay global.
//!
//! Paths are relative to the program root: the directory of the first file
//! compiled with a name (the main script), else the working directory.

use crate::ast::{Node, Program};
use crate::opcode::Irep;
use crate::vm::{Unit, Vm};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Compiles `prog` and, first, every file it uses (transitively).
pub fn compile_program(vm: &mut Vm, prog: &Program) -> Result<Rc<Irep>, String> {
    if vm.module_root.is_none() {
        let dir = match prog.file.as_str() {
            "-" | "-e" | "" => None,
            f => Path::new(f).parent().map(Path::to_path_buf),
        };
        vm.module_root = Some(dir.filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from(".")));
    }
    prepare_uses(vm, prog, &mut Vec::new())?;
    crate::codegen::compile(prog, &mut vm.syms)
}

/// A `use` statement: path, imported names (`None`: all), line.
type UseRef<'a> = (&'a Vec<String>, &'a Option<Vec<String>>, u32);

/// The `use` statements among a file's top-level statements.
fn uses(prog: &Program) -> Vec<UseRef<'_>> {
    top_level(&prog.body)
        .filter_map(|n| match n {
            Node::Use { path, items, line } => Some((path, items, *line)),
            _ => None,
        })
        .collect()
}

fn top_level(body: &Node) -> impl Iterator<Item = &Node> {
    match body {
        Node::Seq(v) => v.iter().collect::<Vec<_>>().into_iter(),
        n => vec![n].into_iter(),
    }
}

/// Constants a file defines at its top level (classes, modules, `X = ...`)
/// or imports by name: what other files may `use` from it.
fn exports(prog: &Program) -> Vec<String> {
    let mut out = Vec::new();
    for n in top_level(&prog.body) {
        match n {
            Node::Class { path, .. } => {
                if let Node::Const(c) = &**path {
                    out.push(c.clone());
                }
            }
            Node::CAsgn(target, _) => {
                if let Node::Const(c) = &**target {
                    out.push(c.clone());
                }
            }
            Node::Use { items: Some(items), .. } => out.extend(items.iter().cloned()),
            _ => {}
        }
    }
    out
}

/// Compiles the files `prog` uses into `vm.units`; `stack` holds the files
/// being prepared (for cycles). Errors start with the line in the file
/// being compiled, or `models/user.rb:3:` for a file it uses.
fn prepare_uses(vm: &mut Vm, prog: &Program, stack: &mut Vec<String>) -> Result<(), String> {
    let current = stack.last().cloned();
    let at = |line: u32| match &current {
        Some(k) => format!("{k}.rb:{line}:"),
        None => format!("line {line}:"),
    };
    for (path, items, line) in uses(prog) {
        let key = path.join("/");
        let shown = format!("use {}", path.join("::"));
        if let Some(i) = stack.iter().position(|k| *k == key) {
            let cycle: Vec<&str> = stack[i..].iter().map(String::as_str).chain([key.as_str()]).collect();
            return Err(format!("{} {shown}: cyclic use ({})", at(line), cycle.join(" -> ")));
        }
        if !vm.units.contains_key(&key) {
            let file = vm.module_root.clone().unwrap_or_default().join(format!("{key}.rb"));
            let name = file.to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&file).map_err(|e| format!("{} {shown}: can't read {name}: {e}", at(line)))?;
            let used = crate::parser::parse_file(&src, &name, 1).map_err(|e| format!("{key}.rb: {e}"))?;
            stack.push(key.clone());
            let r = prepare_uses(vm, &used, stack);
            stack.pop();
            r?;
            let irep = crate::codegen::compile(&used, &mut vm.syms).map_err(|e| format!("{key}.rb: {e}"))?;
            vm.units.insert(key.clone(), Unit { irep, exports: exports(&used), module: None });
        }
        if let Some(items) = items {
            let unit = &vm.units[&key];
            if let Some(missing) = items.iter().find(|i| !unit.exports.contains(i)) {
                return Err(format!("{} {shown}::{missing}: {key}.rb doesn't define {missing}", at(line)));
            }
        }
    }
    Ok(())
}
