//! Properties (lang-fork-ui):
//!
//! ```text
//! class Person {
//!   prop name                    # read outside; set inside (and by init)
//!   prop mut age: Integer = 0    # read and write outside; typed; a default
//!   prop email = "" {            # accessors of its own
//!     set(v) { ...; @email = v }
//!     changed(old) { ... }       # after a change through the setter
//!   }
//! }
//! Person::new(name: "Ann", age: 3)   # the init props make when there's none
//! ```
//!
//! A prop is stored in `@name`, read by `name` and written by `name=`
//! (private without `mut`). The parser turns `prop` into `__prop(...)` plus
//! the accessor `fn`s it needs (see `Parser::parse_prop`); this file keeps
//! the class's [`PropInfo`]s, checks types, fills in defaults when an object
//! is made, and is the init of a class without one.

use super::arg;
use crate::gc::ClassRef;
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{ProcCall, Unwind, Vm, VmResult};

fn class_of_value(vm: &Vm, v: &Value) -> Option<ClassRef> {
    match vm.kind(*v) {
        V::Class(c) => Some(c),
        _ => None,
    }
}

/// The props of `cls` and its superclasses, the oldest first.
fn props_of(vm: &Vm, cls: ClassRef) -> Vec<PropInfo> {
    let mut chain = vec![];
    let mut c = Some(cls);
    while let Some(k) = c {
        chain.push(k);
        c = vm.heap[k].superclass;
    }
    chain.iter().rev().flat_map(|k| vm.heap[*k].props.borrow().clone()).collect()
}

fn has_props(vm: &Vm, cls: ClassRef) -> bool {
    let mut c = Some(cls);
    while let Some(k) = c {
        if !vm.heap[k].props.borrow().is_empty() {
            return true;
        }
        c = vm.heap[k].superclass;
    }
    false
}

/// `Person#age expects Integer, got "x"`.
fn check_type(vm: &mut Vm, cls: ClassRef, p: &PropInfo, v: &Value) -> Result<(), Unwind> {
    if let Some(t) = p.ty
        && !vm.is_a(v, &t)
    {
        let (c, n, t) = (vm.heap[cls].name(), vm.sym_name(p.name).to_string(), vm.heap[t].name());
        let got = vm.inspect(v)?;
        return Err(vm.type_err(format!("{c}#{n} expects {t}, got {got}")));
    }
    Ok(())
}

/// The value a prop starts with on `obj`, if it has a default.
fn default_value(vm: &mut Vm, owner: ClassRef, p: &PropInfo, obj: &Value) -> Result<Option<Value>, Unwind> {
    let stored = vm.ivar_get(&Value::from(owner), p.default_key);
    Ok(match p.default {
        PropDefault::None | PropDefault::Computed => None,
        PropDefault::Value => Some(stored),
        PropDefault::Closure => {
            let call = ProcCall { self_: Some(*obj), target: Some(owner), method: None, lambda: true };
            Some(vm.call_proc(&stored, vec![], None, call)?)
        }
    })
}

/// Which class declared `p` (for its default), searching from `cls` up.
fn declaring_class(vm: &Vm, cls: ClassRef, name: Sym) -> ClassRef {
    let mut c = Some(cls);
    while let Some(k) = c {
        if vm.heap[k].props.borrow().iter().any(|p| p.name == name) {
            return k;
        }
        c = vm.heap[k].superclass;
    }
    cls
}

/// When an object of `cls` is made, before its init: the props' defaults.
pub fn apply_defaults(vm: &mut Vm, obj: &Value, cls: ClassRef) -> Result<(), Unwind> {
    if !has_props(vm, cls) {
        return Ok(());
    }
    for p in props_of(vm, cls) {
        let owner = declaring_class(vm, cls, p.name);
        match default_value(vm, owner, &p, obj)? {
            Some(v) => {
                check_type(vm, cls, &p, &v)?;
                vm.ivar_set(obj, p.ivar, v)?;
            }
            // in declaration order, so the object prints its props in order
            None if p.default == PropDefault::None => vm.ivar_set(obj, p.ivar, Value::NIL)?,
            None => {}
        }
    }
    Ok(())
}

/// The init of a class with props and no `fn init`: every prop by name
/// (through its writer, so a `set` runs), the ones without a default required.
fn prop_init(vm: &mut Vm, s: &Value, a: &[Value], _b: Option<&Value>) -> VmResult {
    let cls = vm.class_of(s);
    let name = vm.heap[cls].name();
    let given: Vec<(Value, Value)> = match a {
        [] => vec![],
        [h] => match vm.kind(*h) {
            V::Hash(h) if vm.heap[h].borrow().kw => vm.heap[h].borrow().to_vec(),
            _ => return Err(vm.arg_err(format!("{name}::new takes its props by name: {name}::new(prop: value, ...)"))),
        },
        _ => return Err(vm.arg_err(format!("{name}::new takes its props by name: {name}::new(prop: value, ...)"))),
    };
    let props = props_of(vm, cls);
    let mut set = vec![false; props.len()];
    vm.prop_initializing.push(*s);
    let r = set_given(vm, s, &name, &props, given, &mut set);
    vm.prop_initializing.pop();
    r?;
    let missing: Vec<String> = props
        .iter()
        .zip(&set)
        .filter(|(p, s)| !**s && p.default == PropDefault::None)
        .map(|(p, _)| vm.sym_name(p.name).to_string())
        .collect();
    if !missing.is_empty() {
        return Err(vm.arg_err(format!("{name}::new is missing {}", missing.join(", "))));
    }
    Ok(Value::NIL)
}

fn set_given(
    vm: &mut Vm,
    s: &Value,
    name: &str,
    props: &[PropInfo],
    given: Vec<(Value, Value)>,
    set: &mut [bool],
) -> Result<(), Unwind> {
    for (k, v) in given {
        let i = match vm.kind(k) {
            V::Sym(s) => props.iter().position(|p| p.name == s),
            _ => None,
        };
        let Some(i) = i.filter(|i| props[*i].default != PropDefault::Computed) else {
            let k = vm.inspect(&k)?;
            return Err(vm.arg_err(format!("{name} has no prop {k} to set")));
        };
        set[i] = true;
        let writer = vm.intern(&format!("{}=", vm.sym_name(props[i].name)));
        vm.call_method(s, writer, vec![v], None)?;
    }
    Ok(())
}

pub fn init(vm: &mut Vm) {
    let m = vm.c.module;
    // `prop`: (name, mut, type or none, default kind 0/1/2, default, line)
    vm.def(&m, "__prop", |vm, s, a, _| {
        let line = match vm.kind(arg(a, 5)) {
            V::Int(n) => n,
            _ => 0,
        };
        let (Some(cls), V::Sym(name)) = (class_of_value(vm, s), vm.kind(arg(a, 0))) else {
            return Err(vm.type_err(format!("line {line}: prop belongs in a class body")));
        };
        vm.check_frozen(s)?;
        let cname = vm.heap[cls].name();
        let pname = vm.sym_name(name).to_string();
        if vm.heap[cls].is_module {
            return Err(vm.type_err(format!("line {line}: {cname} is a module: props are for classes")));
        }
        if vm.heap[cls].props.borrow().iter().any(|p| p.name == name) {
            return Err(vm.type_err(format!("line {line}: {cname} declares prop {pname} twice")));
        }
        let mutable = arg(a, 1).truthy();
        let ty = match vm.kind(arg(a, 2)) {
            V::Nil => None,
            V::Class(t) => Some(t),
            _ => {
                let t = vm.inspect(&arg(a, 2))?;
                return Err(vm.type_err(format!("line {line}: prop {pname}: {t} is not a class or interface")));
            }
        };
        let default = match vm.kind(arg(a, 3)) {
            V::Int(1) => PropDefault::Value,
            V::Int(2) => PropDefault::Closure,
            V::Int(3) => PropDefault::Computed,
            _ => PropDefault::None,
        };
        let ivar = vm.intern(&format!("@{pname}"));
        let default_key = vm.intern(&format!("%default:{pname}"));
        if matches!(default, PropDefault::Value | PropDefault::Closure) {
            vm.ivar_set(s, default_key, arg(a, 4))?;
        }
        // a computed prop (`get` only) has its reader from the parser, no writer
        if default != PropDefault::Computed {
            let writer = vm.intern(&format!("{pname}="));
            // under `private`, a private prop
            let scope = match vm.scope_vis(cls) {
                Vis::Public | Vis::Protected => Vis::Public,
                _ => Vis::Private,
            };
            vm.heap[cls].methods.insert_vis(name, Method::AttrReader(ivar), scope);
            let vis = if mutable { scope } else { Vis::Private };
            vm.heap[cls].methods.insert_vis(writer, Method::AttrWriter(ivar), vis);
        }
        vm.heap[cls].props.borrow_mut().push(PropInfo { name, ivar, mutable, ty, default, default_key });
        // the class's init, unless it (or a superclass) has its own
        let own_init = vm.s.initialize;
        let has_init = match vm.find_method(&cls, own_init) {
            Some((Method::Native(f), owner)) => {
                ![vm.c.object, vm.c.basic_object, vm.c.kernel].contains(&owner) && f as *const () != prop_init as *const ()
            }
            Some(_) => true,
            None => false,
        };
        if !has_init {
            vm.heap[cls].methods.insert_vis(own_init, Method::Native(prop_init), Vis::Private);
        }
        Ok(Value::NIL)
    });
    // a typed prop's writer: (name, value)
    let o = vm.c.object;
    vm.def(&o, "__prop_check", |vm, s, a, _| {
        let V::Sym(name) = vm.kind(arg(a, 0)) else {
            return Ok(Value::NIL);
        };
        let cls = vm.class_of(s);
        if let Some(p) = props_of(vm, cls).into_iter().find(|p| p.name == name) {
            check_type(vm, cls, &p, &arg(a, 1))?;
        }
        Ok(Value::NIL)
    });
    // a setter with `changed`: whether to run it (not while init sets the props)
    vm.def(&o, "__prop_live", |vm, s, a, _| {
        let V::Sym(iv) = vm.kind(arg(a, 0)) else {
            return Ok(Value::FALSE);
        };
        let initializing = vm.prop_initializing.iter().any(|x| vm.heap.identical(x, s));
        let defined = match vm.kind(*s) {
            V::Object(o) => vm.heap[o].ivars.borrow().get(iv).is_some(),
            _ => true,
        };
        Ok(Value::bool(defined && !initializing))
    });
    vm.def(&m, "props", |vm, s, _, _| {
        let Some(cls) = class_of_value(vm, s) else {
            return Ok(vm.array(vec![]));
        };
        let v = props_of(vm, cls).iter().map(|p| Value::sym(p.name)).collect();
        Ok(vm.array(v))
    });
}
