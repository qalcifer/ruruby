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

use crate::ast::{CaseHead, EnumDecl, Node, Program};
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
    check_cases(prog, &known_enums(vm, prog)).map_err(|e| format!("line {e}"))?;
    crate::codegen::compile_with(prog, &mut vm.syms, &fns)
}

/// The enums a file can name: by name, and the variants it can name bare.
struct Known {
    enums: HashMap<String, Vec<String>>,
    bare: HashMap<String, String>,
}

/// Its own enums, the ones it imports (and their variants imported bare),
/// and Result (Ok/Err).
fn known_enums(vm: &Vm, prog: &Program) -> Known {
    let mut k = Known { enums: HashMap::new(), bare: HashMap::new() };
    k.enums.insert("Result".into(), vec!["Ok".into(), "Err".into()]);
    k.bare.insert("Ok".into(), "Result".into());
    k.bare.insert("Err".into(), "Result".into());
    // two enums of one name: neither is checked
    let add = |k: &mut Known, e: &EnumDecl| match k.enums.get(&e.name) {
        Some(v) if *v != e.variants => _ = k.enums.insert(e.name.clone(), vec![]),
        _ => _ = k.enums.insert(e.name.clone(), e.variants.clone()),
    };
    for e in &prog.enums {
        add(&mut k, e);
    }
    for (path, inner, items, _) in uses(prog) {
        let Some(unit) = vm.units.get(&path.join("/")) else { continue };
        let of = |name: &str| unit.enums.iter().find(|e| e.name == name);
        // `use f::Shape::{Circle, Empty}` / `use f::Shape::*`: variants, bare
        let owner = inner.last().and_then(|n| of(n));
        match items {
            Some(items) => {
                for i in items {
                    if let Some(e) = of(i) {
                        add(&mut k, e);
                    }
                    if let Some(e) = owner
                        && e.variants.contains(i)
                    {
                        add(&mut k, e);
                        k.bare.insert(i.clone(), e.name.clone());
                    }
                }
            }
            None => match owner {
                Some(e) => {
                    add(&mut k, e);
                    for v in &e.variants {
                        k.bare.insert(v.clone(), e.name.clone());
                    }
                }
                None => {
                    for e in &unit.enums {
                        add(&mut k, e);
                    }
                }
            },
        }
    }
    k
}

/// Rust's exhaustiveness check, where it can be decided: a `case/in`
/// without `else` whose branches all name variants of one known enum must
/// cover every variant (a guarded branch or one with a pattern inside the
/// fields doesn't count). Errors start with the line number.
fn check_cases(prog: &Program, known: &Known) -> Result<(), String> {
    'cases: for case in &prog.cases {
        let mut target: Option<&str> = None;
        let mut covered: Vec<&str> = vec![];
        for head in &case.heads {
            let (path, full) = match head {
                None | Some(CaseHead::Any) => continue 'cases,
                Some(CaseHead::Partial) => continue,
                Some(CaseHead::Path(p, full)) => (p, *full),
            };
            let (e, v) = match path.as_slice() {
                [.., e, v] => (e.as_str(), v.as_str()),
                [v] => {
                    let own = case.in_enum.as_deref().filter(|e| known.enums.get(*e).is_some_and(|vs| vs.contains(v)));
                    match own.or(known.bare.get(v).map(String::as_str)) {
                        Some(e) => (e, v.as_str()),
                        None => continue 'cases,
                    }
                }
                [] => continue 'cases,
            };
            let Some(variants) = known.enums.get(e) else { continue 'cases };
            if !variants.iter().any(|x| x == v) || target.is_some_and(|t| t != e) {
                continue 'cases;
            }
            target = Some(e);
            if full {
                covered.push(v);
            }
        }
        let Some(e) = target else { continue };
        let missing: Vec<&str> =
            known.enums[e].iter().map(String::as_str).filter(|v| !covered.contains(v)).collect();
        if !missing.is_empty() {
            let fix = if missing.len() == 1 { "an `in` branch for it" } else { "`in` branches for them" };
            return Err(format!(
                "{}: this case on {e} doesn't handle {}: add {fix} or an `else`",
                case.line,
                missing.join(", ")
            ));
        }
    }
    Ok(())
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
    /// An enum variant with fields: a constant (its class) and a
    /// constructor function (`Circle(2)`).
    Variant,
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
                // an enum's variants (see `Parser::parse_enum`)
                Node::Call { recv: None, name, args, .. } if name == "__variant" => {
                    if let [Node::Sym(v), _, Node::Int(kind)] = args.as_slice() {
                        out.names.insert(v.clone(), if *kind == 0 { Export::Value } else { Export::Variant });
                    }
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
                Some(Export::Variant) => return Err(format!("defines {name} as an enum variant, not a module")),
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
            check_cases(&used, &known_enums(vm, &used)).map_err(|e| format!("{key}.rb:{e}"))?;
            let irep = crate::codegen::compile_with(&used, &mut vm.syms, &used_fns).map_err(|e| format!("{key}.rb: {e}"))?;
            let unit = Unit { irep, exports: Exports::of(&used.body, false), module: None, enums: used.enums.clone() };
            vm.units.insert(key.clone(), unit);
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
        // the functions it brings: a module's fns, an enum's constructors
        let callable = |e: Option<&Export>| matches!(e, Some(Export::Fn | Export::Variant));
        let new: Vec<String> = match (items, scope) {
            (Some(items), scope) => items
                .iter()
                .filter(|i| i.starts_with(|c: char| !c.is_uppercase()) || scope.is_some_and(|s| callable(s.names.get(*i))))
                .cloned()
                .collect(),
            (None, Some(scope)) => {
                let mut v: Vec<String> =
                    scope.names.iter().filter(|(_, e)| callable(Some(e))).map(|(n, _)| n.clone()).collect();
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
