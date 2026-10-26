//! Reflection and metaprogramming: singleton classes, `define_method`,
//! `instance_eval`/`class_eval`, `Class.new`, `alias_method`, class
//! variables, constants, and the hooks Ruby calls on include/extend.
//! mruby: `src/class.c`, `src/kernel.c`, and the `mruby-metaprog`,
//! `mruby-class-ext`, `mruby-proc-ext`, `mruby-object-ext` gems.

use super::*;
use crate::vm::{ProcCall, VmResult};

fn this_class(vm: &Vm, s: &Value) -> Result<ClassRef, Unwind> {
    match vm.kind(*s) {
        V::Class(c) => Ok(c),
        _ => Err(vm.type_err("expected a class or module")),
    }
}

fn module_arg(vm: &Vm, v: &Value) -> Result<ClassRef, Unwind> {
    match vm.kind(*v) {
        V::Class(m) if vm.heap[m].is_module => Ok(m),
        _ => Err(vm.type_err(format!(
            "wrong argument type {} (expected Module)",
            vm.class_name(v)
        ))),
    }
}

/// Calls a hook such as `included(base)` (Module defines no-op defaults).
fn hook(vm: &mut Vm, recv: &Value, name: &str, arg: Value) -> Result<(), Unwind> {
    vm.send(recv, name, vec![arg])?;
    Ok(())
}

fn anonymous(vm: &mut Vm, superclass: Option<ClassRef>, is_module: bool) -> ClassRef {
    let cls = vm.new_class("#<Class:?>", superclass, is_module);
    let kind = if is_module { "Module" } else { "Class" };
    vm.heap[cls].set_name(format!("#<{kind}:0x{:016x}>", cls.id()));
    cls
}

/// Runs `blk` with self = `recv` and methods going to `target` (`class_eval`, `instance_exec`).
fn eval_with(
    vm: &mut Vm,
    blk: &Value,
    recv: &Value,
    target: ClassRef,
    args: Vec<Value>,
) -> VmResult {
    let call = ProcCall {
        self_: Some(*recv),
        target: Some(target),
        method: None,
        lambda: false,
    };
    vm.call_proc(blk, args, None, call)
}

pub fn init(vm: &mut Vm) {
    init_kernel(vm);
    init_module(vm);
    init_proc(vm);
    init_pattern_support(vm);
    init_introspection(vm);
    init_method_objects(vm);
}

fn init_kernel(vm: &mut Vm) {
    let k = vm.c.kernel;
    vm.def(&k, "singleton_class", |vm, s, _, _| {
        Ok(Value::from(vm.singleton_class(s)?))
    });
    vm.def(&k, "instance_eval", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let target = vm.singleton_class(s).unwrap_or_else(|_| vm.class_of(s));
        eval_with(vm, &b, s, target, vec![*s])
    });
    vm.def(&k, "instance_exec", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let target = vm.singleton_class(s).unwrap_or_else(|_| vm.class_of(s));
        eval_with(vm, &b, s, target, a.to_vec())
    });
    vm.def(&k, "define_singleton_method", |vm, s, a, b| {
        let name = sym_of(vm, &arg(a, 0))?;
        let body = match b {
            Some(b) => *b,
            None => arg(a, 1),
        };
        let sc = vm.singleton_class(s)?;
        vm.heap[sc].methods.insert(name, Method::Proc(body));
        Ok(Value::sym(name))
    });
    vm.def(&k, "proc", |vm, _, _, b| need_block(vm, b));
    vm.def(&k, "lambda", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        Ok(vm.to_lambda(&b))
    });
    vm.def(&k, "instance_variable_defined?", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        Ok(Value::bool(vm.ivar_lookup(s, iv).is_some()))
    });
    vm.def(&k, "remove_instance_variable", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        match vm.ivar_remove(s, iv)? {
            Some(v) => Ok(v),
            None => Err(vm.name_err(format!("instance variable {} not defined", vm.sym_name(iv)))),
        }
    });
    vm.def(&k, "caller", |vm, _, _, _| Ok(vm.array(vec![])));
    vm.def(&k, "at_exit", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        vm.at_exit.push(b);
        Ok(b)
    });
    vm.def(&k, "Array", |vm, _, a, _| {
        let v = arg(a, 0);
        Ok({ let v = vm.splat_to_vec(&v)?; vm.array(v) })
    });
    vm.def(&k, "String", |vm, _, a, _| {
        Ok({ let v = vm.to_s(&arg(a, 0))?; vm.str(v) })
    });
}

fn sorted_syms(vm: &mut Vm, syms: Vec<Sym>) -> Value {
    let mut names: Vec<String> = syms.iter().map(|s| vm.sym_name(*s).to_string()).collect();
    names.sort();
    let syms = names.iter().map(|n| Value::sym(vm.intern(n))).collect();
    vm.array(syms)
}

fn init_module(vm: &mut Vm) {
    let m = vm.c.module;
    vm.def(&m, "name", |vm, s, _, _| {
        let c = this_class(vm, s)?;
        Ok(if vm.heap[c].is_anonymous() {
            Value::NIL
        } else {
            vm.str(vm.heap[c].name())
        })
    });
    let to_s: NativeFn = |vm, s, _, _| {
        let name = vm.heap[this_class(vm, s)?].name();
        Ok(vm.str(name))
    };
    vm.def(&m, "to_s", to_s);
    vm.def(&m, "inspect", to_s);
    for name in [
        "inherited",
        "method_added",
        "const_added",
    ] {
        vm.def(&m, name, |_, _, _, _| Ok(Value::NIL));
    }
    // lang-fork: no include/prepend/extend; interfaces are attached by `impl`
    vm.def(&m, "include?", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let module = module_arg(vm, &arg(a, 0))?;
        Ok(Value::bool(
            !(cls == module)
                && vm.ancestors(&cls).contains(&module),
        ))
    });
    vm.def(&m, "included_modules", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        Ok(vm.array(
            vm.ancestors(&cls)
                .into_iter()
                .filter(|c| vm.heap[c].is_module)
                .map(Value::from)
                .collect(),
        ))
    });
    vm.def(&m, "define_method", |vm, s, a, b| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        let name = sym_of(vm, &arg(a, 0))?;
        let body = match (b, a.get(1)) {
            (Some(b), _) => vm.to_lambda(b),
            (None, Some(p)) if matches!(vm.kind(*p), V::Proc(_)) => *p,
            _ => return Err(vm.arg_err("tried to create Proc object without a block")),
        };
        let vis = vm.scope_vis(cls);
        if vis == Vis::ModuleFunction {
            let meta = vm.singleton_class(s)?;
            vm.heap[meta].methods.insert(name, Method::Proc(body));
        }
        vm.heap[cls].methods.insert_vis(name, Method::Proc(body), vis);
        Ok(Value::sym(name))
    });
    vm.def(&m, "alias_method", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        let (new, old) = (sym_of(vm, &arg(a, 0))?, sym_of(vm, &arg(a, 1))?);
        vm.alias_method(&cls, new, old)?;
        Ok(Value::sym(new))
    });
    vm.def(&m, "remove_method", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        for n in a {
            let mid = sym_of(vm, n)?;
            if vm.heap[cls].methods.remove(&mid).is_none() {
                let msg = format!(
                    "method '{}' not defined in {}",
                    vm.sym_name(mid),
                    vm.heap[cls].name()
                );
                return Err(vm.name_err(msg));
            }
        }
        Ok(*s)
    });
    vm.def(&m, "undef_method", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        for n in a {
            let mid = sym_of(vm, n)?;
            vm.undef_method(&cls, mid)?;
        }
        Ok(*s)
    });
    let class_eval: NativeFn = |vm, s, _, b| {
        let cls = this_class(vm, s)?;
        let b = need_block(vm, b)?;
        eval_with(vm, &b, s, cls, vec![*s])
    };
    vm.def(&m, "class_eval", class_eval);
    vm.def(&m, "module_eval", class_eval);
    let class_exec: NativeFn = |vm, s, a, b| {
        let cls = this_class(vm, s)?;
        let b = need_block(vm, b)?;
        eval_with(vm, &b, s, cls, a.to_vec())
    };
    vm.def(&m, "class_exec", class_exec);
    vm.def(&m, "module_exec", class_exec);
    // with names; without, `private` & co. are compiled as a scope's
    // default (Op::DefaultVis), so a call that gets here changes nothing
    vm.def(&m, "private", |vm, s, a, _| set_vis(vm, s, a, Vis::Private));
    vm.def(&m, "public", |vm, s, a, _| set_vis(vm, s, a, Vis::Public));
    vm.def(&m, "protected", |vm, s, a, _| set_vis(vm, s, a, Vis::Protected));
    vm.def(&m, "private_class_method", |vm, s, a, _| {
        let meta = vm.singleton_class(s)?;
        set_vis(vm, &Value::from(meta), a, Vis::Private)
    });
    vm.def(&m, "public_class_method", |vm, s, a, _| {
        let meta = vm.singleton_class(s)?;
        set_vis(vm, &Value::from(meta), a, Vis::Public)
    });
    vm.def(&m, "class_variable_get", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        vm.ancestors(&cls)
            .iter()
            .find_map(|k| vm.heap[k].cvars.borrow().get(n))
            .ok_or_else(|| {
                vm.name_err(format!(
                    "uninitialized class variable {} in {}",
                    vm.sym_name(n),
                    vm.heap[cls].name()
                ))
            })
    });
    vm.def(&m, "class_variable_set", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        let holder = vm.ancestors(&cls)
            .into_iter()
            .find(|k| vm.heap[k].cvars.borrow().get(n).is_some())
            .unwrap_or(cls);
        vm.check_frozen(&Value::from(holder))?;
        vm.heap[holder].cvars.borrow_mut().set(n, arg(a, 1));
        Ok(arg(a, 1))
    });
    vm.def(&m, "class_variable_defined?", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        Ok(Value::bool(
            vm.ancestors(&cls)
                .iter()
                .any(|k| vm.heap[k].cvars.borrow().get(n).is_some()),
        ))
    });
    vm.def(&m, "class_variables", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        let mut syms = Vec::new();
        for k in vm.ancestors(&cls) {
            syms.extend(vm.heap[k].cvars.borrow().items.iter().map(|(s, _)| *s));
        }
        Ok(vm.array(syms.into_iter().map(Value::sym).collect()))
    });
    vm.def(&m, "const_get", |vm, s, a, _| {
        let mut cls = this_class(vm, s)?;
        let path = match vm.kind(arg(a, 0)) {
            V::Str(p) => vm.heap[p].borrow().clone(),
            _ => {
                let sym = sym_of(vm, &arg(a, 0))?;
                vm.sym_name(sym).to_string()
            }
        };
        let mut v = Value::NIL;
        for part in path.split("::") {
            if part.is_empty() {
                cls = vm.c.object;
                continue;
            }
            let n = vm.intern(part);
            v = vm.get_const_in(&cls, n).ok_or_else(|| {
                vm.name_err(format!("uninitialized constant {}::{}", vm.heap[cls].name(), part))
            })?;
            if let V::Class(c) = vm.kind(v) {
                cls = c;
            }
        }
        Ok(v)
    });
    vm.def(&m, "const_set", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        if !vm.sym_name(n).starts_with(char::is_uppercase) {
            let msg = format!("wrong constant name {}", vm.sym_name(n));
            return Err(vm.name_err(msg));
        }
        vm.store_const(&cls, n, arg(a, 1));
        Ok(arg(a, 1))
    });
    vm.def(&m, "const_defined?", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        Ok(Value::bool(vm.get_const_in(&cls, n).is_some()))
    });
    vm.def(&m, "constants", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        let syms: Vec<Value> = vm.heap[cls].consts.borrow().keys().map(|k| Value::sym(*k)).collect();
        let _ = vm;
        Ok(vm.array(syms))
    });
    vm.def(&m, "==", |vm, s, a, _| {
        Ok(Value::bool(vm.heap.identical(s, &arg(a, 0))))
    });
    vm.def(&m, "<", |vm, s, a, _| module_cmp(vm, s, &arg(a, 0), true));
    vm.def(&m, "<=", |vm, s, a, _| module_cmp(vm, s, &arg(a, 0), false));
    vm.def(&m, ">", |vm, s, a, _| module_cmp(vm, &arg(a, 0), s, true));
    vm.def(&m, ">=", |vm, s, a, _| module_cmp(vm, &arg(a, 0), s, false));
    vm.def(&m, "singleton_class?", |vm, s, _, _| {
        Ok(Value::bool(vm.heap[this_class(vm, s)?].is_singleton))
    });

    let c = vm.c.class;
    vm.def(&c, "allocate", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        Ok(vm.object(RObject::new(cls)))
    });
}

/// `A < B`: is A a proper descendant of B? nil when unrelated.
fn module_cmp(vm: &mut Vm, a: &Value, b: &Value, strict: bool) -> VmResult {
    let (a, b) = (this_class(vm, a)?, this_class(vm, b)?);
    if a == b {
        return Ok(Value::bool(!strict));
    }
    if vm.ancestors(&a).contains(&b) {
        return Ok(Value::TRUE);
    }
    if vm.ancestors(&b).contains(&a) {
        return Ok(Value::FALSE);
    }
    Ok(Value::NIL)
}

/// `Class.new(super) { body }` / `Module.new { body }`
pub fn new_class_or_module(
    vm: &mut Vm,
    cls: ClassRef,
    a: &[Value],
    b: Option<&Value>,
) -> VmResult {
    let is_module = cls == vm.c.module;
    let superclass = if is_module {
        None
    } else {
        match vm.kind(arg(a, 0)) {
            V::Nil => Some(vm.c.object),
            V::Class(s) if !vm.heap[s].is_module && !vm.heap[s].is_singleton => Some(s),
            _ => return Err(vm.type_err("superclass must be a Class")),
        }
    };
    let new = anonymous(vm, superclass, is_module);
    let v = Value::from(new);
    if let Some(sup) = superclass {
        hook(vm, &Value::from(sup), "inherited", v)?;
    }
    if let Some(b) = b {
        eval_with(vm, b, &v, new, vec![v])?;
    }
    Ok(v)
}

fn init_proc(vm: &mut Vm) {
    let p = vm.c.proc_;
    vm.def_singleton(&p, "new", |vm, _, _, b| {
        b.cloned()
            .ok_or_else(|| vm.arg_err("tried to create Proc object without a block"))
    });
    vm.def(&p, "lambda?", |vm, s, _, _| {
        Ok(Value::bool(matches!(vm.kind(*s), V::Proc(p) if vm.heap[p].lambda)))
    });
    vm.def(&p, "inspect", |vm, s, _, _| {
        let V::Proc(p) = vm.kind(*s) else { unreachable!() };
        let lambda = if vm.heap[p].lambda { " (lambda)" } else { "" };
        Ok(vm.str(format!("#<Proc:0x{:016x}{lambda}>", p.id())))
    });
    vm.def(&p, "to_s", |vm, s, _, _| vm.send(s, "inspect", vec![]));
    vm.def(&p, "==", |vm, s, a, _| {
        Ok(Value::bool(vm.heap.identical(s, &arg(a, 0))))
    });
}

/// Protocol methods used by pattern matching (`deconstruct`, `deconstruct_keys`).
fn init_pattern_support(vm: &mut Vm) {
    let k = vm.c.kernel;
    // used by desugared patterns: nil when the subject has no hook
    vm.def(&k, "__pm_deconstruct", |vm, _, a, _| {
        let v = arg(a, 0);
        let hook = vm.intern("deconstruct");
        // through Ruby's respond_to?, which a class may override
        if !vm.send(&v, "respond_to?", vec![Value::sym(hook)])?.truthy() {
            return Ok(Value::NIL);
        }
        let r = vm.call_method(&v, hook, vec![], None)?;
        match vm.kind(r) {
            V::Array(_) => Ok(r),
            _ => Err(vm.type_err("deconstruct must return Array")),
        }
    });
    vm.def(&k, "__pm_deconstruct_keys", |vm, _, a, _| {
        let v = arg(a, 0);
        let hook = vm.intern("deconstruct_keys");
        if !vm.send(&v, "respond_to?", vec![Value::sym(hook)])?.truthy() {
            return Ok(Value::NIL);
        }
        let r = vm.call_method(&v, hook, vec![arg(a, 1)], None)?;
        match vm.kind(r) {
            V::Hash(_) => Ok(r),
            _ => Err(vm.type_err("deconstruct_keys must return Hash")),
        }
    });
    vm.def(&k, "`", |vm, _, _, _| {
        Err(vm.err(&vm.c.not_implemented_error, "backquotes not implemented"))
    });
    let a = vm.c.array;
    vm.def(&a, "deconstruct", |_, s, _, _| Ok(*s));
    let h = vm.c.hash;
    vm.def(&h, "deconstruct_keys", |_, s, _, _| Ok(*s));
    vm.def(&h, "except", |vm, s, a, _| {
        let mut out = vm.heap[this_hash(vm, s)?].borrow().clone();
        out.kw = false;
        for k in a {
            out.remove(&vm.heap, k);
        }
        Ok(vm.hash(out))
    });
}

/// The method names given to `private` & co.: Symbols, Strings, or Arrays
/// of them (`private attr_accessor :a, :b`).
fn method_names(vm: &mut Vm, a: &[Value]) -> Result<Vec<Sym>, Unwind> {
    let mut names = Vec::new();
    for v in a {
        match vm.kind(*v) {
            V::Array(items) => {
                let items = vm.heap[items].borrow().clone();
                for x in items {
                    names.push(sym_of(vm, &x)?);
                }
            }
            _ => names.push(sym_of(vm, v)?),
        }
    }
    Ok(names)
}

/// `private :a, :b` (and public/protected): changes the methods'
/// visibility in `s`. An inherited method is copied into `s` with the new
/// visibility, as in mruby. Returns the arguments.
fn set_vis(vm: &mut Vm, s: &Value, a: &[Value], vis: Vis) -> VmResult {
    vm.check_frozen(s)?;
    let cls = this_class(vm, s)?;
    for name in method_names(vm, a)? {
        let own = matches!(
            vm.heap[cls].methods.borrow().get(&name),
            Some(m) if !matches!(m, Method::Undefined)
        );
        // a module's functions (`fn f` in its body) are on its singleton class
        let meta = vm.heap[cls].meta.get();
        let own_fn = vm.heap[cls].is_module
            && meta.is_some_and(|m| matches!(vm.heap[m].methods.borrow().get(&name), Some(m) if !matches!(m, Method::Undefined)));
        if own {
            vm.heap[cls].methods.set_vis(name, vis);
        } else if own_fn {
            vm.heap[meta.unwrap()].methods.set_vis(name, vis);
        } else if let Some((m, _)) = vm.find_method(&cls, name) {
            vm.heap[cls].methods.insert_vis(name, m, vis);
        } else {
            let kind = if vm.heap[cls].is_module { "module" } else { "class" };
            let msg = format!(
                "undefined method '{}' for {kind} '{}'",
                vm.sym_name(name),
                vm.heap[cls].name()
            );
            return Err(vm.name_err(msg));
        }
    }
    Ok(match a.len() {
        0 => Value::NIL,
        1 => a[0],
        _ => vm.array(a.to_vec()),
    })
}

/// Which visibilities a listing or a `*_defined?` query reports.
#[derive(Clone, Copy)]
enum Shown {
    /// `instance_methods`, `methods`, `method_defined?`: what a call with
    /// a receiver may reach from somewhere (public and protected)
    Callable,
    Only(Vis),
}

impl Shown {
    fn matches(self, vis: Vis) -> bool {
        match self {
            Shown::Callable => matches!(vis, Vis::Public | Vis::Protected),
            Shown::Only(v) => v == vis,
        }
    }
}

/// The methods `cls` has (with `inherit`, also from its ancestors) whose
/// visibility `shown` takes, sorted. For each name the first module of the
/// lookup order decides; an undef hides the name.
fn listed(vm: &mut Vm, cls: ClassRef, inherit: bool, shown: Shown) -> Value {
    let classes = if inherit { vm.ancestors(&cls) } else { vec![cls] };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in classes {
        let table = &vm.heap[c].methods;
        for (k, m) in table.borrow().iter() {
            if seen.insert(*k) && !matches!(m, Method::Undefined) && shown.matches(table.vis(*k)) {
                out.push(*k);
            }
        }
    }
    sorted_syms(vm, out)
}

/// Whether `cls` (only itself without `inherit`) has `name` with a
/// visibility `shown` takes.
fn defined_as(vm: &Vm, cls: ClassRef, name: Sym, inherit: bool, shown: Shown) -> bool {
    if inherit {
        return vm.find_method_vis(&cls, name).is_some_and(|(_, _, vis)| shown.matches(vis));
    }
    let table = &vm.heap[cls].methods;
    matches!(table.borrow().get(&name), Some(m) if !matches!(m, Method::Undefined))
        && shown.matches(table.vis(name))
}

fn init_introspection(vm: &mut Vm) {
    let k = vm.c.kernel;
    // respond_to?(name, include_all = false): a method a call with a
    // receiver can reach, any method with include_all; else what
    // respond_to_missing? says (when it is defined at all)
    vm.def(&k, "respond_to?", |vm, s, a, _| {
        check_argc(vm, a, 1, 2)?;
        let mid = sym_of(vm, &arg(a, 0))?;
        let include_all = a.get(1).copied().unwrap_or(Value::FALSE);
        let cls = vm.dispatch_class(s);
        match vm.find_method_vis(&cls, mid) {
            Some((_, _, Vis::Public)) => return Ok(Value::TRUE),
            Some(_) if include_all.truthy() => return Ok(Value::TRUE),
            _ => {}
        }
        let rtm = vm.s.respond_to_missing;
        if vm.find_method(&cls, rtm).is_none() {
            return Ok(Value::FALSE);
        }
        let r = vm.call_method(s, rtm, vec![Value::sym(mid), include_all], None)?;
        Ok(Value::bool(r.truthy()))
    });
    vm.def(&k, "respond_to_missing?", |_, _, _, _| Ok(Value::FALSE));
    // send, but only to public methods
    vm.def(&k, "public_send", |vm, s, a, b| {
        let Some((name, rest)) = a.split_first() else {
            return Err(vm.arg_err("no method name given"));
        };
        let mid = sym_of(vm, name)?;
        let cls = vm.dispatch_class(s);
        if let Some((_, _, vis)) = vm.find_method_vis(&cls, mid)
            && vis != Vis::Public
        {
            return Err(vm.not_visible(vis, s, mid));
        }
        vm.call_with_block(s, mid, rest.to_vec(), b.cloned())
    });
    fn of_object(vm: &mut Vm, s: &Value, shown: Shown) -> VmResult {
        let c = vm.dispatch_class(s);
        Ok(listed(vm, c, true, shown))
    }
    vm.def(&k, "methods", |vm, s, _, _| of_object(vm, s, Shown::Callable));
    vm.def(&k, "public_methods", |vm, s, _, _| of_object(vm, s, Shown::Only(Vis::Public)));
    vm.def(&k, "private_methods", |vm, s, _, _| of_object(vm, s, Shown::Only(Vis::Private)));
    vm.def(&k, "protected_methods", |vm, s, _, _| {
        of_object(vm, s, Shown::Only(Vis::Protected))
    });
    // the methods of the object's own singleton class
    vm.def(&k, "singleton_methods", |vm, s, _, _| {
        let sc = match vm.kind(*s) {
            V::Class(c) => vm.heap[c].meta.get(),
            V::Object(o) => vm.heap[o].singleton.get(),
            _ => vm.heap.extra_of(s).and_then(|x| x.singleton),
        };
        match sc {
            Some(sc) => Ok(listed(vm, sc, false, Shown::Callable)),
            None => Ok(vm.array(vec![])),
        }
    });

    let m = vm.c.module;
    // instance_methods(inherit = true) & co.
    vm.def(&m, "instance_methods", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        Ok(listed(vm, cls, inherit(a, 0), Shown::Callable))
    });
    vm.def(&m, "public_instance_methods", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        Ok(listed(vm, cls, inherit(a, 0), Shown::Only(Vis::Public)))
    });
    vm.def(&m, "private_instance_methods", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        Ok(listed(vm, cls, inherit(a, 0), Shown::Only(Vis::Private)))
    });
    vm.def(&m, "protected_instance_methods", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        Ok(listed(vm, cls, inherit(a, 0), Shown::Only(Vis::Protected)))
    });
    // method_defined?(name, inherit = true) & co.
    vm.def(&m, "method_defined?", |vm, s, a, _| {
        let (cls, mid) = (this_class(vm, s)?, sym_of(vm, &arg(a, 0))?);
        Ok(Value::bool(defined_as(vm, cls, mid, inherit(a, 1), Shown::Callable)))
    });
    vm.def(&m, "public_method_defined?", |vm, s, a, _| {
        let (cls, mid) = (this_class(vm, s)?, sym_of(vm, &arg(a, 0))?);
        Ok(Value::bool(defined_as(vm, cls, mid, inherit(a, 1), Shown::Only(Vis::Public))))
    });
    vm.def(&m, "private_method_defined?", |vm, s, a, _| {
        let (cls, mid) = (this_class(vm, s)?, sym_of(vm, &arg(a, 0))?);
        Ok(Value::bool(defined_as(vm, cls, mid, inherit(a, 1), Shown::Only(Vis::Private))))
    });
    vm.def(&m, "protected_method_defined?", |vm, s, a, _| {
        let (cls, mid) = (this_class(vm, s)?, sym_of(vm, &arg(a, 0))?);
        Ok(Value::bool(defined_as(vm, cls, mid, inherit(a, 1), Shown::Only(Vis::Protected))))
    });
}

/// The `inherit` argument at `i`: true when missing, read for truth.
fn inherit(a: &[Value], i: usize) -> bool {
    a.get(i).is_none_or(|v| v.truthy())
}

// ---- Method and UnboundMethod ---------------------------------------
//
// Objects of class Method / UnboundMethod with hidden ivars (names without
// `@`, so `instance_variables` doesn't show them): `__recv` (Method only),
// `__owner` (the class or module defining the method) and `__name`.
// Calling looks the method up from the owner (Vm::call_resolved), so a
// method bound to an instance of a subclass is still the owner's.

fn method_object(vm: &mut Vm, cls: ClassRef, recv: Option<Value>, owner: ClassRef, name: Sym) -> VmResult {
    let obj = vm.object(RObject::new(cls));
    let (r, o, n) = (vm.s.m_recv, vm.s.m_owner, vm.s.m_name);
    if let Some(recv) = recv {
        vm.ivar_set(&obj, r, recv)?;
    }
    vm.ivar_set(&obj, o, Value::from(owner))?;
    vm.ivar_set(&obj, n, Value::sym(name))?;
    Ok(obj)
}

/// The (owner, name) of a Method or UnboundMethod.
fn method_of(vm: &Vm, m: &Value) -> Result<(ClassRef, Sym), Unwind> {
    let owner = vm.ivar_lookup(m, vm.s.m_owner).map(|o| vm.kind(o));
    let name = vm.ivar_lookup(m, vm.s.m_name).map(|n| vm.kind(n));
    match (owner, name) {
        (Some(V::Class(o)), Some(V::Sym(n))) => Ok((o, n)),
        _ => Err(vm.type_err("not a method")),
    }
}

/// `obj.method(:name)`: NameError when there is no such method.
fn find_for_object(vm: &mut Vm, s: &Value, a: &[Value], public_only: bool) -> VmResult {
    let name = sym_of(vm, &arg(a, 0))?;
    let cls = vm.dispatch_class(s);
    match vm.find_method_vis(&cls, name) {
        Some((_, _, vis)) if public_only && vis != Vis::Public => Err(vm.not_visible(vis, s, name)),
        Some((_, owner, _)) => {
            let c = vm.c.method_;
            method_object(vm, c, Some(*s), owner, name)
        }
        None => {
            let msg = format!("undefined method '{}' for {}", vm.sym_name(name), vm.recv_desc(s));
            Err(vm.name_err(msg))
        }
    }
}

/// `Mod.instance_method(:name)`.
fn find_for_module(vm: &mut Vm, s: &Value, a: &[Value], public_only: bool) -> VmResult {
    let cls = this_class(vm, s)?;
    let name = sym_of(vm, &arg(a, 0))?;
    match vm.find_method_vis(&cls, name) {
        Some((_, _, vis)) if public_only && vis != Vis::Public => {
            let kind = if vm.heap[cls].is_module { "module" } else { "class" };
            let vis = if vis == Vis::Protected { "protected" } else { "private" };
            let msg = format!(
                "method '{}' for {kind} '{}' is {vis}",
                vm.sym_name(name),
                vm.heap[cls].name()
            );
            Err(vm.name_err(msg))
        }
        Some((_, owner, _)) => {
            let c = vm.c.unbound_method;
            method_object(vm, c, None, owner, name)
        }
        None => {
            let kind = if vm.heap[cls].is_module { "module" } else { "class" };
            let msg = format!("undefined method '{}' for {kind} '{}'", vm.sym_name(name), vm.heap[cls].name());
            Err(vm.name_err(msg))
        }
    }
}

/// Like `Proc#arity` for a lambda: required count, or -(required + 1)
/// with optional or rest parameters; natives say -1.
pub fn method_arity(vm: &Vm, owner: ClassRef, name: Sym) -> i64 {
    match vm.find_method(&owner, name).map(|(m, _)| m) {
        Some(Method::Ruby(irep)) => irep.spec.arity(),
        Some(Method::AttrReader(_)) => 0,
        Some(Method::AttrWriter(_)) => 1,
        Some(Method::Proc(p)) => match vm.kind(p) {
            V::Proc(p) => match &vm.heap[p].body {
                ProcBody::Ruby { irep, .. } => irep.spec.arity(),
                _ => -1,
            },
            _ => -1,
        },
        _ => -1,
    }
}

fn method_inspect(vm: &mut Vm, s: &Value, kind: &str) -> VmResult {
    let (owner, name) = method_of(vm, s)?;
    let sep = if vm.heap[owner].is_singleton { "." } else { "#" };
    let owner_name = match vm.heap[owner].attached.get() {
        Some(c) if vm.heap[owner].is_singleton => vm.heap[c].name(),
        _ => vm.heap[owner].name(),
    };
    Ok(vm.str(format!("#<{kind}: {owner_name}{sep}{}>", vm.sym_name(name))))
}

fn init_method_objects(vm: &mut Vm) {
    let k = vm.c.kernel;
    vm.def(&k, "method", |vm, s, a, _| find_for_object(vm, s, a, false));
    vm.def(&k, "public_method", |vm, s, a, _| find_for_object(vm, s, a, true));
    let m = vm.c.module;
    vm.def(&m, "instance_method", |vm, s, a, _| find_for_module(vm, s, a, false));
    vm.def(&m, "public_instance_method", |vm, s, a, _| find_for_module(vm, s, a, true));

    let me = vm.c.method_;
    let call: NativeFn = |vm, s, a, b| {
        let (recv, owner, name) = vm.method_parts(s)?;
        vm.call_resolved(recv, owner, name, a.to_vec(), b.cloned())
    };
    for name in ["call", "()", "[]", "==="] {
        vm.def(&me, name, call);
    }
    vm.def(&me, "to_proc", |vm, s, _, _| {
        let p = vm.proc_value(ProcBody::Method(*s));
        Ok(vm.to_lambda(&p))
    });
    vm.def(&me, "receiver", |vm, s, _, _| Ok(vm.method_parts(s)?.0));
    vm.def(&me, "owner", |vm, s, _, _| Ok(Value::from(method_of(vm, s)?.0)));
    vm.def(&me, "name", |vm, s, _, _| Ok(Value::sym(method_of(vm, s)?.1)));
    vm.def(&me, "arity", |vm, s, _, _| {
        let (owner, name) = method_of(vm, s)?;
        Ok(vm.int(method_arity(vm, owner, name)))
    });
    vm.def(&me, "unbind", |vm, s, _, _| {
        let (owner, name) = method_of(vm, s)?;
        let c = vm.c.unbound_method;
        method_object(vm, c, None, owner, name)
    });
    vm.def(&me, "inspect", |vm, s, _, _| method_inspect(vm, s, "Method"));
    vm.def(&me, "to_s", |vm, s, _, _| method_inspect(vm, s, "Method"));
    vm.def(&me, "==", |vm, s, a, _| {
        let o = arg(a, 0);
        if vm.class_of(&o) != vm.c.method_ {
            return Ok(Value::FALSE);
        }
        let (x, y) = (vm.method_parts(s)?, vm.method_parts(&o)?);
        Ok(Value::bool(vm.heap.identical(&x.0, &y.0) && x.1 == y.1 && x.2 == y.2))
    });

    let um = vm.c.unbound_method;
    // bind(obj): a Method of obj, which must be an instance of the owner
    vm.def(&um, "bind", |vm, s, a, _| {
        let (owner, name) = method_of(vm, s)?;
        let recv = arg(a, 0);
        if !vm.is_a(&recv, &owner) {
            let msg = format!("bind argument must be an instance of {}", vm.heap[owner].name());
            return Err(vm.type_err(msg));
        }
        let c = vm.c.method_;
        method_object(vm, c, Some(recv), owner, name)
    });
    vm.def(&um, "bind_call", |vm, s, a, b| {
        let (owner, name) = method_of(vm, s)?;
        let Some((recv, args)) = a.split_first() else {
            return Err(vm.arg_err("wrong number of arguments (given 0, expected 1+)"));
        };
        if !vm.is_a(recv, &owner) {
            let msg = format!("bind argument must be an instance of {}", vm.heap[owner].name());
            return Err(vm.type_err(msg));
        }
        vm.call_resolved(*recv, owner, name, args.to_vec(), b.cloned())
    });
    vm.def(&um, "owner", |vm, s, _, _| Ok(Value::from(method_of(vm, s)?.0)));
    vm.def(&um, "name", |vm, s, _, _| Ok(Value::sym(method_of(vm, s)?.1)));
    vm.def(&um, "arity", |vm, s, _, _| {
        let (owner, name) = method_of(vm, s)?;
        Ok(vm.int(method_arity(vm, owner, name)))
    });
    vm.def(&um, "inspect", |vm, s, _, _| method_inspect(vm, s, "UnboundMethod"));
    vm.def(&um, "to_s", |vm, s, _, _| method_inspect(vm, s, "UnboundMethod"));
}

