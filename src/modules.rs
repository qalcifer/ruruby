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
use std::collections::HashMap;
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
    let fns = prepare_uses(vm, prog, &mut Vec::new())?;
    crate::codegen::compile_with(prog, &mut vm.syms, &fns)
}

/// A `use` statement: file path, modules inside it, imported names
/// (`None`: all), line.
type UseRef<'a> = (&'a Vec<String>, &'a Vec<String>, &'a Option<Vec<String>>, u32);

/// The `use` statements among a file's top-level statements.
fn uses(prog: &Program) -> Vec<UseRef<'_>> {
    top_level(&prog.body)
        .filter_map(|n| match n {
            Node::Use { path, inner, items, line } => Some((path, inner, items, *line)),
            _ => None,
        })
        .collect()
}

/// The constants a file (or a class/module body in it) defines.
#[derive(Default)]
pub struct Exports {
    pub names: HashMap<String, Export>,
}

pub enum Export {
    /// A class or module, with its own constants.
    Scope(Exports),
    /// A constant whose value is a literal: nothing inside it.
    Value,
    /// Only known when it runs (`X = make()`, a name the file imports).
    Unknown,
    /// A module's public function (`fn f` in `module M { }`).
    Fn,
}

impl Exports {
    /// What `body` defines; `module`: a module's body, whose public `fn`s
    /// are its functions.
    fn of(body: &Node, module: bool) -> Exports {
        let mut out = Exports::default();
        let mut public = true;
        for n in top_level(body) {
            match n {
                Node::Class { path, body, is_module, interface, .. } => {
                    if let Node::Const(c) = &**path {
                        // a reopened class or module adds to what it had
                        let entry = out.names.entry(c.clone()).or_insert_with(|| Export::Scope(Exports::default()));
                        if let Export::Scope(e) = entry {
                            e.names.extend(Exports::of(body, *is_module && !*interface).names);
                        }
                    }
                }
                Node::Def { recv, name, .. } if module && public && recv.as_deref().is_none_or(|r| matches!(r, Node::SelfRef)) => {
                    out.names.insert(name.clone(), Export::Fn);
                }
                // `private` / `public` on a line of their own
                Node::Call { recv: None, name, args, .. } if args.is_empty() && (name == "private" || name == "public") => {
                    public = name == "public";
                }
                Node::CAsgn(target, value) => {
                    if let Node::Const(c) = &**target {
                        let literal = matches!(
                            **value,
                            Node::Int(_) | Node::Float(_) | Node::Str(_) | Node::DStr(_) | Node::Sym(_) | Node::Array(_)
                                | Node::Hash(..) | Node::Range(..) | Node::Nil | Node::True | Node::False
                        );
                        out.names.insert(c.clone(), if literal { Export::Value } else { Export::Unknown });
                    }
                }
                Node::Use { items: Some(items), .. } => {
                    for i in items {
                        out.names.insert(i.clone(), Export::Unknown);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Where `inner` leads: `Ok(Some(e))`, `Ok(None)` past what's known, or
    /// what's wrong with it.
    fn find(&self, inner: &[String]) -> Result<Option<&Exports>, String> {
        let mut cur = self;
        for (i, seg) in inner.iter().enumerate() {
            let name = inner[..=i].join("::");
            match cur.names.get(seg) {
                None => return Err(format!("doesn't define {name}")),
                Some(Export::Value) => return Err(format!("defines {name} as a value, not a module or class")),
                Some(Export::Unknown) => return Ok(None),
                Some(Export::Fn) => return Err(format!("defines {name} as a function, not a module or class")),
                Some(Export::Scope(e)) => cur = e,
            }
        }
        Ok(Some(cur))
    }
}

fn top_level(body: &Node) -> impl Iterator<Item = &Node> {
    match body {
        Node::Seq(v) => v.iter().collect::<Vec<_>>().into_iter(),
        n => vec![n].into_iter(),
    }
}

/// Compiles the files `prog` uses into `vm.units`; `stack` holds the files
/// being prepared (for cycles). Errors start with the line in the file
/// being compiled, or `models/user.rb:3:` for a file it uses. Returns the
/// module functions `prog` imports (for the code generator).
fn prepare_uses(vm: &mut Vm, prog: &Program, stack: &mut Vec<String>) -> Result<Vec<String>, String> {
    let mut fns: Vec<String> = Vec::new();
    // the file's own top-level `fn`s, which an imported function can't share a name with
    let own: Vec<&str> = top_level(&prog.body)
        .filter_map(|n| match n {
            Node::Def { recv: None, name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let current = stack.last().cloned();
    let at = |line: u32| match &current {
        Some(k) => format!("{k}.rb:{line}:"),
        None => format!("line {line}:"),
    };
    // name → (where it comes from, line): one name, one meaning
    let mut imported: HashMap<&str, (String, u32)> = HashMap::new();
    for (path, inner, items, line) in uses(prog) {
        let key = path.join("/");
        let origin = path.iter().chain(inner).cloned().collect::<Vec<_>>().join("::");
        for item in items.iter().flatten() {
            match imported.get(item.as_str()) {
                Some((from, first)) if *from != origin => {
                    return Err(format!(
                        "{} use {origin}::{item}: {item} is already imported from {from} (line {first})",
                        at(line)
                    ));
                }
                Some(_) => {}
                None => _ = imported.insert(item, (origin.clone(), line)),
            }
        }
        let what = match items {
            Some(v) if v.len() == 1 => v[0].clone(),
            Some(v) => format!("{{{}}}", v.join(", ")),
            None => "*".into(),
        };
        let shown = format!("use {}", path.iter().chain(inner).chain([&what]).cloned().collect::<Vec<_>>().join("::"));
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
            let used_fns = r?;
            let irep = crate::codegen::compile_with(&used, &mut vm.syms, &used_fns).map_err(|e| format!("{key}.rb: {e}"))?;
            vm.units.insert(key.clone(), Unit { irep, exports: Exports::of(&used.body, false), module: None });
        }
        let unit = &vm.units[&key];
        let scope = unit.exports.find(inner).map_err(|e| format!("{} {shown}: {key}.rb {e}", at(line)))?;
        if let (Some(scope), Some(items)) = (scope, items)
            && let Some(missing) = items.iter().find(|i| !scope.names.contains_key(*i))
        {
            let name = inner.iter().chain([missing]).cloned().collect::<Vec<_>>().join("::");
            let what = if missing.starts_with(char::is_lowercase) { " (a public fn of the module)" } else { "" };
            return Err(format!("{} {shown}: {key}.rb doesn't define {name}{what}", at(line)));
        }
        let new: Vec<String> = match (items, scope) {
            (Some(items), _) => items.iter().filter(|i| i.starts_with(|c: char| !c.is_uppercase())).cloned().collect(),
            (None, Some(scope)) => {
                let mut v: Vec<String> =
                    scope.names.iter().filter(|(_, e)| matches!(e, Export::Fn)).map(|(n, _)| n.clone()).collect();
                v.sort();
                v
            }
            (None, None) => vec![],
        };
        for f in new {
            if own.contains(&f.as_str()) {
                return Err(format!("{} {shown}: {f} is also defined in this file", at(line)));
            }
            if !fns.contains(&f) {
                fns.push(f);
            }
        }
    }
    Ok(fns)
}
