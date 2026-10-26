//! Interfaces (lang-fork). `interface Name: Supers { ... }` is a module with
//! a list of required methods (its defaults are ordinary methods, its
//! super-interfaces included modules). `impl I for C { ... }` defines the
//! block's methods on C, puts I in C's ancestors and checks the contract
//! when the block ends, so a broken impl stops the program at load time.
//!
//! The parser turns both forms into calls of the hidden methods below (see
//! `Parser::parse_interface` and `Parser::parse_impl`).

use super::arg;
use super::reflect::method_arity;
use crate::gc::ClassRef;
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{Unwind, Vm};

fn class_of_value(vm: &Vm, v: &Value) -> Option<ClassRef> {
    match vm.kind(*v) {
        V::Class(c) => Some(c),
        _ => None,
    }
}

fn is_interface(vm: &Vm, c: ClassRef) -> bool {
    vm.heap[c].interface.borrow().is_some()
}

fn required(vm: &Vm, i: ClassRef) -> Vec<Required> {
    vm.heap[i].interface.borrow().as_ref().map(|info| info.required.clone()).unwrap_or_default()
}

fn int_arg(vm: &Vm, a: &[Value], i: usize) -> i64 {
    match vm.kind(arg(a, i)) {
        V::Int(n) => n,
        _ => 0,
    }
}

fn list(vm: &Vm, v: Value) -> Vec<Value> {
    match vm.kind(v) {
        V::Array(a) => vm.heap[a].borrow().clone(),
        _ => vec![],
    }
}

fn syms(vm: &Vm, v: Value) -> Vec<Sym> {
    list(vm, v)
        .into_iter()
        .filter_map(|v| match vm.kind(v) {
            V::Sym(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// `i` and its super-interfaces, each once.
fn with_supers(vm: &Vm, i: ClassRef, out: &mut Vec<ClassRef>) {
    if out.contains(&i) {
        return;
    }
    out.push(i);
    let supers: Vec<ClassRef> = vm.heap[i].includes.borrow().clone();
    for s in supers {
        if is_interface(vm, s) {
            with_supers(vm, s, out);
        }
    }
}

fn fail(vm: &Vm, line: i64, msg: String) -> Unwind {
    vm.type_err(format!("line {line}: {msg}"))
}

fn names(vm: &Vm, cs: &[ClassRef]) -> String {
    cs.iter().map(|c| vm.heap[*c].name()).collect::<Vec<_>>().join(", ")
}

/// Whether a method of `actual` arity can stand for a declared one (both
/// like `Method#arity`).
fn arity_fits(declared: i64, actual: i64) -> bool {
    let req = |a: i64| if a >= 0 { a } else { -a - 1 };
    (req(actual) == req(declared) && (declared >= 0 || actual < 0)) || (actual < 0 && req(actual) <= req(declared))
}

fn arity_desc(a: i64) -> String {
    match a {
        1 => "1 argument".into(),
        a if a >= 0 => format!("{a} arguments"),
        a => format!("{}+ arguments", -a - 1),
    }
}

/// Makes a core module an interface requiring `name`.
fn core_interface(vm: &mut Vm, i: ClassRef, name: &str, arity: i64, sig: &str) {
    let name = vm.intern(name);
    let r = Required { name, arity, class_method: false, sig: sig.into() };
    *vm.heap[i].interface.borrow_mut() = Some(InterfaceInfo { required: vec![r] });
}

pub fn init(vm: &mut Vm) {
    // their defaults are the native and prelude methods already defined
    core_interface(vm, vm.c.comparable, "<=>", 1, "<=>(other)");
    core_interface(vm, vm.c.enumerable, "each", 0, "each(&block)");
    let m = vm.c.module;
    // `interface I: Supers {` — self is the new module
    vm.def(&m, "__interface", |vm, s, a, _| {
        let line = int_arg(vm, a, 0);
        let Some(i) = class_of_value(vm, s) else {
            return Err(vm.type_err("__interface outside an interface"));
        };
        let n = vm.heap[i].name();
        if is_interface(vm, i) {
            return Err(fail(vm, line, format!("interface {n} is already defined")));
        }
        let k = &vm.heap[i];
        if !k.methods.borrow().is_empty() || !k.consts.borrow().is_empty() || !k.includes.borrow().is_empty() {
            return Err(fail(vm, line, format!("{n} is already a module")));
        }
        let mut supers = vec![];
        for v in &a[1.min(a.len())..] {
            match class_of_value(vm, v) {
                Some(c) if is_interface(vm, c) => supers.push(c),
                _ => {
                    let what = vm.inspect(v)?;
                    return Err(fail(vm, line, format!("interface {n}: {what} is not an interface")));
                }
            }
        }
        *vm.heap[i].interface.borrow_mut() = Some(InterfaceInfo::default());
        for c in supers {
            vm.heap[i].includes.push(c);
        }
        Ok(Value::NIL)
    });
    // a bodiless `fn` in an interface: (name, arity, class method?, signature)
    vm.def(&m, "__require", |vm, s, a, _| {
        let (Some(i), V::Sym(name), V::Str(sig)) = (class_of_value(vm, s), vm.kind(arg(a, 0)), vm.kind(arg(a, 3))) else {
            return Err(vm.type_err("__require outside an interface"));
        };
        let r = Required {
            name,
            arity: int_arg(vm, a, 1),
            class_method: arg(a, 2).truthy(),
            sig: vm.heap[sig].borrow().clone(),
        };
        match vm.heap[i].interface.borrow_mut().as_mut() {
            Some(info) => info.required.push(r),
            None => return Err(vm.type_err("__require outside an interface")),
        }
        Ok(Value::NIL)
    });
    vm.def(&m, "implements?", |vm, s, a, _| {
        let (Some(c), Some(i)) = (class_of_value(vm, s), class_of_value(vm, &arg(a, 0))) else {
            return Ok(Value::FALSE);
        };
        Ok(Value::bool(is_interface(vm, i) && c != i && vm.ancestors(&c).contains(&i)))
    });
    vm.def(&m, "interface?", |vm, s, _, _| {
        Ok(Value::bool(class_of_value(vm, s).is_some_and(|c| is_interface(vm, c))))
    });

    let o = vm.c.object;
    // before an impl block runs: (target, line, interfaces...) -> target
    vm.def(&o, "__impl_target", |vm, _, a, _| {
        let target = arg(a, 0);
        let line = int_arg(vm, a, 1);
        let ifaces = &a[2.min(a.len())..];
        let cls = match class_of_value(vm, &target) {
            Some(c) if !vm.heap[c].is_module => c,
            Some(c) => {
                let n = vm.heap[c].name();
                return Err(fail(vm, line, format!("impl for {n}: {n} is a module; impl is for classes")));
            }
            None => {
                return Err(fail(
                    vm,
                    line,
                    "impl without `for` belongs in a class body (or write `impl I for Class { ... }`)".into(),
                ));
            }
        };
        for v in ifaces {
            if !class_of_value(vm, v).is_some_and(|c| is_interface(vm, c)) {
                let what = vm.inspect(v)?;
                return Err(fail(vm, line, format!("impl {what}: {what} is not an interface")));
            }
        }
        Ok(Value::from(cls))
    });
    // after an impl block: (target, line, [interfaces], [methods], [class methods], by)
    vm.def(&o, "__impl", |vm, _, a, _| {
        let target = arg(a, 0);
        let line = int_arg(vm, a, 1);
        let Some(cls) = class_of_value(vm, &target) else {
            return Err(vm.type_err("__impl: not a class"));
        };
        let ifaces: Vec<ClassRef> = list(vm, arg(a, 2)).iter().filter_map(|v| class_of_value(vm, v)).collect();
        let (defined, class_defined) = (syms(vm, arg(a, 3)), syms(vm, arg(a, 4)));
        let by = match vm.kind(arg(a, 5)) {
            V::Sym(s) => Some(s),
            _ => None,
        };
        let cname = vm.heap[cls].name();
        let what = names(vm, &ifaces);
        for &i in &ifaces {
            if vm.heap[cls].includes.borrow().contains(&i) {
                let n = vm.heap[i].name();
                return Err(fail(vm, line, format!("{cname} already implements {n}")));
            }
        }
        // put each interface in the ancestors, unless it already is (through a
        // super-interface or the superclass), which would change the order
        for &i in &ifaces {
            if !vm.ancestors(&cls).contains(&i) {
                vm.heap[cls].includes.push(i);
            }
        }
        let mut all = vec![];
        for &i in &ifaces {
            with_supers(vm, i, &mut all);
        }
        // strict blocks: only the interfaces' methods
        for name in defined {
            let part = all.iter().any(|&i| {
                vm.heap[i].methods.borrow().contains_key(&name)
                    || required(vm, i).iter().any(|r| r.name == name && !r.class_method)
            });
            if !part {
                let n = vm.sym_name(name).to_string();
                return Err(fail(vm, line, format!("{n}() is not part of {what}: define it in the class")));
            }
        }
        for name in class_defined {
            if !all.iter().any(|&i| required(vm, i).iter().any(|r| r.name == name && r.class_method)) {
                let n = vm.sym_name(name).to_string();
                return Err(fail(vm, line, format!("self.{n}() is not part of {what}: define it in the class")));
            }
        }
        // `by @field`: the required methods the class doesn't define go to the field
        if let Some(iv) = by {
            for &i in &all {
                for r in required(vm, i) {
                    if !r.class_method && !vm.heap[cls].methods.borrow().contains_key(&r.name) {
                        vm.heap[cls].methods.insert(r.name, Method::Forward(iv));
                    }
                }
            }
        }
        // every requirement met, by the class, a superclass or another impl
        // (not by Object's catch-alls like to_s or ==)
        let base = [vm.c.object, vm.c.basic_object, vm.c.kernel];
        for &i in &all {
            let iname = vm.heap[i].name();
            let mut missing = vec![];
            for r in required(vm, i) {
                let holder = if r.class_method { vm.singleton_class(&target)? } else { cls };
                match vm.find_method(&holder, r.name) {
                    Some((_, owner)) if base.contains(&owner) => missing.push(r.sig.clone()),
                    None => missing.push(r.sig.clone()),
                    Some((Method::Forward(_), _)) => {}
                    Some(_) => {
                        let actual = method_arity(vm, holder, r.name);
                        if actual != -1 && !arity_fits(r.arity, actual) {
                            let n = vm.sym_name(r.name).to_string();
                            let sep = if r.class_method { "." } else { "#" };
                            return Err(fail(
                                vm,
                                line,
                                format!(
                                    "{cname} doesn't implement {iname}: {cname}{sep}{n} takes {}, {iname} declares {}",
                                    arity_desc(actual),
                                    r.sig
                                ),
                            ));
                        }
                    }
                }
            }
            if !missing.is_empty() {
                return Err(fail(vm, line, format!("{cname} doesn't implement {iname}: missing {}", missing.join(", "))));
            }
        }
        // two interfaces giving the class the same default: the class decides
        let mut mine = vec![];
        let own: Vec<ClassRef> = vm.heap[cls].includes.borrow().clone();
        for i in own {
            if is_interface(vm, i) {
                with_supers(vm, i, &mut mine);
            }
        }
        let mut defaults: Vec<(String, ClassRef)> = vec![];
        for &i in &mine {
            let keys: Vec<Sym> = vm.heap[i].methods.borrow().keys().copied().collect();
            for k in keys {
                defaults.push((vm.sym_name(k).to_string(), i));
            }
        }
        defaults.sort_by(|x, y| x.0.cmp(&y.0));
        for group in defaults.chunk_by(|x, y| x.0 == y.0) {
            if group.len() < 2 {
                continue;
            }
            let name = vm.intern(&group[0].0);
            if vm.heap[cls].methods.borrow().contains_key(&name) {
                continue;
            }
            let owners: Vec<ClassRef> = group.iter().map(|(_, i)| *i).collect();
            // an owner below another one (Solid: Shape overriding Shape's) wins
            let top: Vec<ClassRef> = owners
                .iter()
                .copied()
                .filter(|&o| {
                    !owners.iter().any(|&p| {
                        let mut s = vec![];
                        with_supers(vm, p, &mut s);
                        p != o && s.contains(&o)
                    })
                })
                .collect();
            if top.len() > 1 {
                let from = names(vm, &top).replace(", ", " and ");
                return Err(fail(
                    vm,
                    line,
                    format!("{cname} gets {}() from both {from}: define it in the class", group[0].0),
                ));
            }
        }
        Ok(target)
    });
}
