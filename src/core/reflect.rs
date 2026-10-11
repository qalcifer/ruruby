//! Reflection and metaprogramming: singleton classes, `define_method`,
//! `instance_eval`/`class_eval`, `Class.new`, `alias_method`, class
//! variables, constants, and the hooks Ruby calls on include/extend.
//! mruby: `src/class.c`, `src/kernel.c`, and the `mruby-metaprog`,
//! `mruby-class-ext`, `mruby-proc-ext`, `mruby-object-ext` gems.

use super::*;
use crate::vm::{ProcCall, VmResult};

fn this_class(vm: &Vm, s: &Value) -> Result<Rc<RClass>, Unwind> {
    match s {
        Value::Class(c) => Ok(c.clone()),
        _ => Err(vm.type_err("expected a class or module")),
    }
}

fn module_arg(vm: &Vm, v: &Value) -> Result<Rc<RClass>, Unwind> {
    match v {
        Value::Class(m) if m.is_module => Ok(m.clone()),
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

fn anonymous(vm: &mut Vm, superclass: Option<Rc<RClass>>, is_module: bool) -> Rc<RClass> {
    let cls = RClass::new("#<Class:?>", superclass, is_module);
    let kind = if is_module { "Module" } else { "Class" };
    cls.set_name(format!("#<{kind}:0x{:016x}>", Rc::as_ptr(&cls) as usize));
    let _ = vm;
    cls
}

/// Runs `blk` with self = `recv` and methods going to `target` (`class_eval`, `instance_exec`).
fn eval_with(
    vm: &mut Vm,
    blk: &Value,
    recv: &Value,
    target: Rc<RClass>,
    args: Vec<Value>,
) -> VmResult {
    let call = ProcCall {
        self_: Some(recv.clone()),
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
}

fn init_kernel(vm: &mut Vm) {
    let k = vm.c.kernel.clone();
    vm.def(&k, "singleton_class", |vm, s, _, _| {
        Ok(Value::Class(vm.singleton_class(s)?))
    });
    vm.def(&k, "extend", |vm, s, a, _| {
        let sc = vm.singleton_class(s)?;
        for m in a.iter().rev() {
            let m = module_arg(vm, m)?;
            if !sc.includes.borrow().iter().any(|x| Rc::ptr_eq(x, &m)) {
                sc.includes.borrow_mut().push(m.clone());
            }
            hook(vm, &Value::Class(m), "extended", s.clone())?;
        }
        Ok(s.clone())
    });
    vm.def(&k, "instance_eval", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let target = vm.singleton_class(s).unwrap_or_else(|_| vm.class_of(s));
        eval_with(vm, &b, s, target, vec![s.clone()])
    });
    vm.def(&k, "instance_exec", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let target = vm.singleton_class(s).unwrap_or_else(|_| vm.class_of(s));
        eval_with(vm, &b, s, target, a.to_vec())
    });
    vm.def(&k, "define_singleton_method", |vm, s, a, b| {
        let name = sym_of(vm, &arg(a, 0))?;
        let body = match b {
            Some(b) => b.clone(),
            None => arg(a, 1),
        };
        vm.singleton_class(s)?
            .methods
            .borrow_mut()
            .insert(name, Method::Proc(body));
        Ok(Value::Sym(name))
    });
    vm.def(&k, "singleton_methods", |vm, s, _, _| {
        let names: Vec<Sym> = match s {
            Value::Class(c) => c
                .meta
                .borrow()
                .as_ref()
                .map(|m| m.methods.borrow().keys().copied().collect())
                .unwrap_or_default(),
            Value::Object(o) => o
                .singleton
                .borrow()
                .as_ref()
                .map(|m| m.methods.borrow().keys().copied().collect())
                .unwrap_or_default(),
            _ => vec![],
        };
        Ok(sorted_syms(vm, names))
    });
    vm.def(&k, "proc", |vm, _, _, b| need_block(vm, b));
    vm.def(&k, "lambda", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        Ok(vm.to_lambda(&b))
    });
    vm.def(&k, "instance_variable_defined?", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        Ok(Value::Bool(vm.ivar_lookup(s, iv).is_some()))
    });
    vm.def(&k, "remove_instance_variable", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        let table = match s {
            Value::Object(o) => &o.ivars,
            Value::Class(c) => &c.ivars,
            _ => return Err(vm.name_err("instance variable not defined")),
        };
        let mut t = table.borrow_mut();
        match t.items.iter().position(|(k, _)| *k == iv) {
            Some(i) => Ok(t.items.remove(i).1),
            None => Err(vm.name_err(format!("instance variable {} not defined", vm.sym_name(iv)))),
        }
    });
    vm.def(&k, "respond_to_missing?", |_, _, _, _| {
        Ok(Value::Bool(false))
    });
    vm.def(&k, "caller", |_, _, _, _| Ok(Value::array(vec![])));
    vm.def(&k, "at_exit", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        vm.at_exit.push(b.clone());
        Ok(b)
    });
    vm.def(&k, "Array", |vm, _, a, _| {
        let v = arg(a, 0);
        Ok(Value::array(vm.splat_to_vec(&v)?))
    });
    vm.def(&k, "String", |vm, _, a, _| {
        Ok(Value::str(vm.to_s(&arg(a, 0))?))
    });
}

fn sorted_syms(vm: &mut Vm, syms: Vec<Sym>) -> Value {
    let mut names: Vec<String> = syms.iter().map(|s| vm.sym_name(*s).to_string()).collect();
    names.sort();
    Value::array(names.iter().map(|n| Value::Sym(vm.intern(n))).collect())
}

fn init_module(vm: &mut Vm) {
    let m = vm.c.module.clone();
    vm.def(&m, "name", |vm, s, _, _| {
        let c = this_class(vm, s)?;
        Ok(if c.is_anonymous() {
            Value::Nil
        } else {
            Value::str(c.name())
        })
    });
    let to_s: NativeFn = |vm, s, _, _| Ok(Value::str(this_class(vm, s)?.name()));
    vm.def(&m, "to_s", to_s);
    vm.def(&m, "inspect", to_s);
    for name in [
        "included",
        "extended",
        "prepended",
        "inherited",
        "method_added",
        "const_added",
    ] {
        vm.def(&m, name, |_, _, _, _| Ok(Value::Nil));
    }
    vm.def(&m, "include", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        for v in a.iter().rev() {
            let module = module_arg(vm, v)?;
            if Vm::ancestors(&cls).iter().any(|x| Rc::ptr_eq(x, &module)) {
                continue;
            }
            cls.includes.borrow_mut().push(module.clone());
            hook(vm, v, "included", s.clone())?;
        }
        Ok(s.clone())
    });
    vm.def(&m, "prepend", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        for v in a.iter().rev() {
            let module = module_arg(vm, v)?;
            if cls.prepends.borrow().iter().any(|x| Rc::ptr_eq(x, &module)) {
                continue;
            }
            cls.prepends.borrow_mut().push(module.clone());
            hook(vm, v, "prepended", s.clone())?;
        }
        Ok(s.clone())
    });
    vm.def(&m, "include?", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let module = module_arg(vm, &arg(a, 0))?;
        Ok(Value::Bool(
            !Rc::ptr_eq(&cls, &module)
                && Vm::ancestors(&cls).iter().any(|x| Rc::ptr_eq(x, &module)),
        ))
    });
    vm.def(&m, "included_modules", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        Ok(Value::array(
            Vm::ancestors(&cls)
                .into_iter()
                .filter(|c| c.is_module)
                .map(Value::Class)
                .collect(),
        ))
    });
    vm.def(&m, "define_method", |vm, s, a, b| {
        let cls = this_class(vm, s)?;
        let name = sym_of(vm, &arg(a, 0))?;
        let body = match (b, a.get(1)) {
            (Some(b), _) => vm.to_lambda(b),
            (None, Some(p @ Value::Proc(_))) => p.clone(),
            _ => return Err(vm.arg_err("tried to create Proc object without a block")),
        };
        cls.methods.borrow_mut().insert(name, Method::Proc(body));
        Ok(Value::Sym(name))
    });
    vm.def(&m, "alias_method", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let (new, old) = (sym_of(vm, &arg(a, 0))?, sym_of(vm, &arg(a, 1))?);
        vm.alias_method(&cls, new, old)?;
        Ok(Value::Sym(new))
    });
    vm.def(&m, "remove_method", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        for n in a {
            let mid = sym_of(vm, n)?;
            if cls.methods.borrow_mut().remove(&mid).is_none() {
                let msg = format!(
                    "method '{}' not defined in {}",
                    vm.sym_name(mid),
                    cls.name()
                );
                return Err(vm.name_err(msg));
            }
        }
        Ok(s.clone())
    });
    vm.def(&m, "undef_method", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        for n in a {
            let mid = sym_of(vm, n)?;
            vm.undef_method(&cls, mid)?;
        }
        Ok(s.clone())
    });
    let class_eval: NativeFn = |vm, s, _, b| {
        let cls = this_class(vm, s)?;
        let b = need_block(vm, b)?;
        eval_with(vm, &b, s, cls, vec![s.clone()])
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
    vm.def(&m, "instance_methods", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let own_only = a.first().is_some_and(|v| !v.truthy());
        let classes = if own_only {
            vec![cls]
        } else {
            Vm::ancestors(&cls)
        };
        let mut syms = Vec::new();
        for c in classes {
            for (k, m) in c.methods.borrow().iter() {
                if !matches!(m, Method::Undefined) && !syms.contains(k) {
                    syms.push(*k);
                }
            }
        }
        Ok(sorted_syms(vm, syms))
    });
    let method_defined: NativeFn = |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let mid = sym_of(vm, &arg(a, 0))?;
        Ok(Value::Bool(Vm::find_method(&cls, mid).is_some()))
    };
    vm.def(&m, "method_defined?", method_defined);
    vm.def(&m, "public_method_defined?", method_defined);
    vm.def(&m, "private_method_defined?", |_, _, _, _| {
        Ok(Value::Bool(false))
    });
    vm.def(&m, "protected_method_defined?", |_, _, _, _| {
        Ok(Value::Bool(false))
    });
    vm.def(&m, "instance_method", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let mid = sym_of(vm, &arg(a, 0))?;
        if Vm::find_method(&cls, mid).is_none() {
            let msg = format!(
                "undefined method '{}' for class '{}'",
                vm.sym_name(mid),
                cls.name()
            );
            return Err(vm.name_err(msg));
        }
        Ok(Value::Sym(mid))
    });
    // visibility is not enforced yet; these return their arguments so `private def x` works
    for name in [
        "private",
        "public",
        "protected",
        "private_class_method",
        "public_class_method",
        "private_constant",
    ] {
        vm.def(&m, name, |_, _, a, _| {
            Ok(match a.len() {
                0 => Value::Nil,
                1 => a[0].clone(),
                _ => Value::array(a.to_vec()),
            })
        });
    }
    vm.def(&m, "module_function", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let sc = vm.class_singleton(&cls);
        for n in a {
            let mid = sym_of(vm, n)?;
            if let Some((meth, _)) = Vm::find_method(&cls, mid) {
                sc.methods.borrow_mut().insert(mid, meth);
            }
        }
        Ok(Value::Nil)
    });
    vm.def(&m, "attr", |vm, s, a, _| {
        let attr_reader = vm.intern("attr_reader");
        vm.call_method(s, attr_reader, a.to_vec(), None)
    });
    vm.def(&m, "class_variable_get", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        Vm::ancestors(&cls)
            .iter()
            .find_map(|k| k.cvars.borrow().get(n))
            .ok_or_else(|| {
                vm.name_err(format!(
                    "uninitialized class variable {} in {}",
                    vm.sym_name(n),
                    cls.name()
                ))
            })
    });
    vm.def(&m, "class_variable_set", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        let holder = Vm::ancestors(&cls)
            .into_iter()
            .find(|k| k.cvars.borrow().get(n).is_some())
            .unwrap_or(cls);
        holder.cvars.borrow_mut().set(n, arg(a, 1));
        Ok(arg(a, 1))
    });
    vm.def(&m, "class_variable_defined?", |vm, s, a, _| {
        let cls = this_class(vm, s)?;
        let n = sym_of(vm, &arg(a, 0))?;
        Ok(Value::Bool(
            Vm::ancestors(&cls)
                .iter()
                .any(|k| k.cvars.borrow().get(n).is_some()),
        ))
    });
    vm.def(&m, "class_variables", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        let mut syms = Vec::new();
        for k in Vm::ancestors(&cls) {
            syms.extend(k.cvars.borrow().items.iter().map(|(s, _)| *s));
        }
        Ok(Value::array(syms.into_iter().map(Value::Sym).collect()))
    });
    vm.def(&m, "const_get", |vm, s, a, _| {
        let mut cls = this_class(vm, s)?;
        let path = match arg(a, 0) {
            Value::Str(p) => p.borrow().clone(),
            v => {
                let sym = sym_of(vm, &v)?;
                vm.sym_name(sym).to_string()
            }
        };
        let mut v = Value::Nil;
        for part in path.split("::") {
            if part.is_empty() {
                cls = vm.c.object.clone();
                continue;
            }
            let n = vm.intern(part);
            v = vm.get_const_in(&cls, n).ok_or_else(|| {
                vm.name_err(format!("uninitialized constant {}::{}", cls.name(), part))
            })?;
            if let Value::Class(c) = &v {
                cls = c.clone();
            }
        }
        Ok(v)
    });
    vm.def(&m, "const_set", |vm, s, a, _| {
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
        Ok(Value::Bool(vm.get_const_in(&cls, n).is_some()))
    });
    vm.def(&m, "constants", |vm, s, _, _| {
        let cls = this_class(vm, s)?;
        let syms: Vec<Value> = cls.consts.borrow().keys().map(|k| Value::Sym(*k)).collect();
        let _ = vm;
        Ok(Value::array(syms))
    });
    vm.def(&m, "freeze", |_, s, _, _| Ok(s.clone()));
    vm.def(&m, "==", |_, s, a, _| {
        Ok(Value::Bool(s.identical(&arg(a, 0))))
    });
    vm.def(&m, "<", |vm, s, a, _| module_cmp(vm, s, &arg(a, 0), true));
    vm.def(&m, "<=", |vm, s, a, _| module_cmp(vm, s, &arg(a, 0), false));
    vm.def(&m, ">", |vm, s, a, _| module_cmp(vm, &arg(a, 0), s, true));
    vm.def(&m, ">=", |vm, s, a, _| module_cmp(vm, &arg(a, 0), s, false));
    vm.def(&m, "singleton_class?", |vm, s, _, _| {
        Ok(Value::Bool(this_class(vm, s)?.is_singleton))
    });

    let c = vm.c.class.clone();
    vm.def(&c, "allocate", |vm, s, _, _| {
        Ok(Value::Object(Rc::new(RObject::new(this_class(vm, s)?))))
    });
}

/// `A < B`: is A a proper descendant of B? nil when unrelated.
fn module_cmp(vm: &mut Vm, a: &Value, b: &Value, strict: bool) -> VmResult {
    let (a, b) = (this_class(vm, a)?, this_class(vm, b)?);
    if Rc::ptr_eq(&a, &b) {
        return Ok(Value::Bool(!strict));
    }
    if Vm::ancestors(&a).iter().any(|x| Rc::ptr_eq(x, &b)) {
        return Ok(Value::Bool(true));
    }
    if Vm::ancestors(&b).iter().any(|x| Rc::ptr_eq(x, &a)) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Nil)
}

/// `Class.new(super) { body }` / `Module.new { body }`
pub fn new_class_or_module(
    vm: &mut Vm,
    cls: &Rc<RClass>,
    a: &[Value],
    b: Option<&Value>,
) -> VmResult {
    let is_module = Rc::ptr_eq(cls, &vm.c.module);
    let superclass = if is_module {
        None
    } else {
        match arg(a, 0) {
            Value::Nil => Some(vm.c.object.clone()),
            Value::Class(s) if !s.is_module && !s.is_singleton => Some(s),
            _ => return Err(vm.type_err("superclass must be a Class")),
        }
    };
    let new = anonymous(vm, superclass.clone(), is_module);
    let v = Value::Class(new.clone());
    if let Some(sup) = superclass {
        hook(vm, &Value::Class(sup), "inherited", v.clone())?;
    }
    if let Some(b) = b {
        eval_with(vm, b, &v, new, vec![v.clone()])?;
    }
    Ok(v)
}

fn init_proc(vm: &mut Vm) {
    let p = vm.c.proc_.clone();
    vm.def_singleton(&p, "new", |vm, _, _, b| {
        b.cloned()
            .ok_or_else(|| vm.arg_err("tried to create Proc object without a block"))
    });
    vm.def(&p, "lambda?", |_, s, _, _| {
        Ok(Value::Bool(matches!(s, Value::Proc(p) if p.lambda)))
    });
    vm.def(&p, "inspect", |_, s, _, _| {
        let Value::Proc(p) = s else { unreachable!() };
        let lambda = if p.lambda { " (lambda)" } else { "" };
        Ok(Value::str(format!(
            "#<Proc:0x{:016x}{lambda}>",
            Rc::as_ptr(p) as usize
        )))
    });
    vm.def(&p, "to_s", |vm, s, _, _| vm.send(s, "inspect", vec![]));
    vm.def(&p, "==", |_, s, a, _| {
        Ok(Value::Bool(s.identical(&arg(a, 0))))
    });
}

/// Protocol methods used by pattern matching (`deconstruct`, `deconstruct_keys`).
fn init_pattern_support(vm: &mut Vm) {
    let k = vm.c.kernel.clone();
    // used by desugared patterns: nil when the subject has no hook
    vm.def(&k, "__pm_deconstruct", |vm, _, a, _| {
        let v = arg(a, 0);
        let hook = vm.intern("deconstruct");
        // through Ruby's respond_to?, which a class may override
        if !vm.send(&v, "respond_to?", vec![Value::Sym(hook)])?.truthy() {
            return Ok(Value::Nil);
        }
        match vm.call_method(&v, hook, vec![], None)? {
            r @ Value::Array(_) => Ok(r),
            _ => Err(vm.type_err("deconstruct must return Array")),
        }
    });
    vm.def(&k, "__pm_deconstruct_keys", |vm, _, a, _| {
        let v = arg(a, 0);
        let hook = vm.intern("deconstruct_keys");
        if !vm.send(&v, "respond_to?", vec![Value::Sym(hook)])?.truthy() {
            return Ok(Value::Nil);
        }
        match vm.call_method(&v, hook, vec![arg(a, 1)], None)? {
            r @ Value::Hash(_) => Ok(r),
            _ => Err(vm.type_err("deconstruct_keys must return Hash")),
        }
    });
    vm.def(&k, "`", |vm, _, _, _| {
        Err(vm.err(&vm.c.not_implemented_error, "backquotes not implemented"))
    });
    let a = vm.c.array.clone();
    vm.def(&a, "deconstruct", |_, s, _, _| Ok(s.clone()));
    let h = vm.c.hash.clone();
    vm.def(&h, "deconstruct_keys", |_, s, _, _| Ok(s.clone()));
    vm.def(&h, "except", |vm, s, a, _| {
        let mut out = this_hash(vm, s)?.borrow().clone();
        out.kw = false;
        for k in a {
            out.remove(k);
        }
        Ok(Value::hash(out))
    });
}
