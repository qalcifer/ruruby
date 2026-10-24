//! Loading Ruby files: `load`, `require`, `require_relative` (CRuby's
//! `load.c`; mruby has none in its core, `mruby-require` adds them).
//!
//! A loaded file runs at the top level (`self` is main) in the same VM.
//! `require` loads a file once, recording its canonical path in
//! `$LOADED_FEATURES`; names that aren't paths are looked up in
//! `$LOAD_PATH`. Codegen passes the calling file to `require_relative` as a
//! hidden second argument.

use super::{arg, check_argc, str_of};
use crate::value::*;
use crate::vm::{Unwind, Vm, VmResult};
use std::path::{Path, PathBuf};

pub fn init(vm: &mut Vm) {
    let load_path = vm.array(vec![]);
    let features = vm.array(vec![]);
    for (name, v) in [("$LOAD_PATH", load_path), ("$:", load_path), ("$LOADED_FEATURES", features), ("$\"", features)] {
        let s = vm.intern(name);
        vm.globals.insert(s, v);
    }
    let o = vm.c.object;
    vm.def(&o, "load", |vm, _, a, _| {
        check_argc(vm, a, 1, 2)?;
        let path = PathBuf::from(str_of(vm, &a[0])?);
        if !path.is_file() {
            return Err(load_error(vm, &path.to_string_lossy()));
        }
        run_file(vm, &path)?;
        Ok(Value::TRUE)
    });
    vm.def(&o, "require", |vm, _, a, _| {
        check_argc(vm, a, 1, 1)?;
        let name = str_of(vm, &a[0])?;
        let Some(path) = find_feature(vm, &name)? else {
            return Err(load_error(vm, &name));
        };
        require_path(vm, &path)
    });
    vm.def(&o, "require_relative", |vm, _, a, _| {
        check_argc(vm, a, 1, 2)?;
        let name = str_of(vm, &a[0])?;
        let from = match arg(a, 1) {
            v if v.is_nil() => String::new(),
            v => str_of(vm, &v)?,
        };
        if matches!(from.as_str(), "" | "-" | "-e") {
            return Err(vm.err(&vm.c.load_error, "cannot infer basepath"));
        }
        let base = Path::new(&from).parent().unwrap_or(Path::new("."));
        let path = with_rb(&base.join(&name));
        if !path.is_file() {
            return Err(load_error(vm, &path.to_string_lossy()));
        }
        require_path(vm, &path)
    });
}

fn load_error(vm: &Vm, name: &str) -> Unwind {
    vm.err(&vm.c.load_error, format!("cannot load such file -- {name}"))
}

/// `name`, or `name.rb` when `name` has no extension.
fn with_rb(name: &Path) -> PathBuf {
    if name.extension().is_some() { name.to_path_buf() } else { name.with_extension("rb") }
}

/// The file `require name` means: a path (absolute, `./`, `../`) as given,
/// anything else searched in `$LOAD_PATH`.
fn find_feature(vm: &mut Vm, name: &str) -> Result<Option<PathBuf>, Unwind> {
    let direct = Path::new(name);
    if direct.is_absolute() || name.starts_with("./") || name.starts_with("../") {
        let p = with_rb(direct);
        return Ok(p.is_file().then_some(p));
    }
    for dir in global_array(vm, "$LOAD_PATH") {
        let dir = str_of(vm, &dir)?;
        let p = with_rb(&Path::new(&dir).join(name));
        if p.is_file() {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

fn global_array(vm: &mut Vm, name: &str) -> Vec<Value> {
    let s = vm.intern(name);
    match vm.globals.get(&s).map(|v| vm.kind(*v)) {
        Some(V::Array(a)) => vm.heap[a].borrow().clone(),
        _ => Vec::new(),
    }
}

/// Loads `path` unless `$LOADED_FEATURES` has it; true when it loaded.
fn require_path(vm: &mut Vm, path: &Path) -> VmResult {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned();
    let s = vm.intern("$LOADED_FEATURES");
    let features = vm.globals.get(&s).copied();
    let Some(V::Array(list)) = features.map(|v| vm.kind(v)) else {
        return Err(vm.type_err("$LOADED_FEATURES is not an Array"));
    };
    let loaded = vm.heap[list].borrow().clone();
    for v in &loaded {
        if matches!(vm.kind(*v), V::Str(s) if *vm.heap[s].borrow() == key) {
            return Ok(Value::FALSE);
        }
    }
    // recorded first, like Ruby: a file requiring itself doesn't loop
    let k = vm.str(key);
    vm.heap[list].borrow_mut().push(k);
    run_file(vm, path)?;
    Ok(Value::TRUE)
}

/// Compiles and runs a file at the top level.
fn run_file(vm: &mut Vm, path: &Path) -> Result<(), Unwind> {
    let name = path.to_string_lossy().into_owned();
    let src = std::fs::read_to_string(path).map_err(|_| load_error(vm, &name))?;
    let prog = crate::parser::parse_file(&src, &name, 1)
        .map_err(|e| vm.err(&vm.c.syntax_error, format!("{name}: {e}")))?;
    let irep = crate::modules::compile_program(vm, &prog).map_err(|e| vm.err(&vm.c.syntax_error, format!("{name}: {e}")))?;
    vm.run_main(irep).map_err(Unwind::Error)?;
    Ok(())
}
