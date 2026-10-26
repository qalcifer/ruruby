//! Kernel/Object, Module/Class, Exception, Proc, nil/true/false, Symbol, Comparable.
//! mruby: `src/kernel.c`, `src/object.c`, `src/class.c`, `src/error.c`,
//! `src/proc.c`, `src/symbol.c`, `mrblib/kernel.rb`, `mrblib/compar.rb`.

use super::*;
use crate::vm::{VmResult, inspect_str};

pub fn init(vm: &mut Vm) {
    init_object(vm);
    init_module(vm);
    init_exception(vm);
    init_misc(vm);
    init_comparable(vm);
}

fn init_object(vm: &mut Vm) {
    let o = vm.c.kernel;
    vm.def(&o, "puts", puts);
    vm.def(&o, "print", |vm, _, a, _| {
        for v in a {
            let s = vm.to_s(v)?;
            vm.write(&s);
        }
        Ok(Value::NIL)
    });
    vm.def(&o, "p", |vm, _, a, _| {
        for v in a {
            let s = vm.inspect(v)?;
            vm.write(&s);
            vm.write("\n");
        }
        Ok(match a.len() {
            0 => Value::NIL,
            1 => a[0],
            _ => vm.array(a.to_vec()),
        })
    });
    vm.def(&o, "format", |vm, _, a, _| {
        let fmt = str_of(vm, &arg(a, 0))?;
        Ok({ let v = format(vm, &fmt, a.get(1..).unwrap_or(&[]))?; vm.str(v) })
    });
    vm.def(&o, "sprintf", |vm, _, a, _| {
        let fmt = str_of(vm, &arg(a, 0))?;
        Ok({ let v = format(vm, &fmt, a.get(1..).unwrap_or(&[]))?; vm.str(v) })
    });
    vm.def(&o, "gets", |vm, _, _, _| {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => Ok(Value::NIL),
            Ok(_) => Ok(vm.str(line)),
        }
    });
    vm.def(&o, "raise", raise);
    vm.def(&o, "loop", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        loop {
            vm.call_block(&b, vec![])?;
        }
    });
    vm.def(&o, "proc", |vm, _, _, b| need_block(vm, b));
    vm.def(&o, "lambda", |vm, _, _, b| need_block(vm, b));
    vm.def(&o, "Integer", |vm, _, a, _| match vm.kind(arg(a, 0)) {
        V::Str(s) => {
            let t = vm.heap[s].borrow().trim().replace('_', "");
            let parsed = t.parse::<i64>().map_err(|_| {
                vm.arg_err(format!(
                    "invalid value for Integer(): {}",
                    inspect_str(&vm.heap[s].borrow())
                ))
            })?;
            Ok(vm.int(parsed))
        }
        _ => {
            let n = int_of(vm, &arg(a, 0))?;
            Ok(vm.int(n))
        }
    });
    vm.def(&o, "Float", |vm, _, a, _| match vm.kind(arg(a, 0)) {
        V::Str(s) => vm.heap[s]
            .borrow()
            .trim()
            .parse::<f64>()
            .map(Value::float)
            .map_err(|_| {
                vm.arg_err(format!(
                    "invalid value for Float(): {}",
                    inspect_str(&vm.heap[s].borrow())
                ))
            }),
        _ => Ok(Value::float(float_of(vm, &arg(a, 0))?)),
    });
    vm.def(&o, "init", |_, _, _, _| Ok(Value::NIL));
    vm.def(&o, "class", |vm, s, _, _| Ok(Value::from(vm.class_of(s))));
    vm.def(&o, "to_s", |vm, s, _, _| {
        if vm.heap.identical(s, &vm.main_obj) {
            return Ok(vm.str("main"));
        }
        Ok(vm.str(format!("#<{}>", vm.class_name(s))))
    });
    vm.def(&o, "inspect", obj_inspect);
    vm.def(&o, "==", |vm, s, a, _| {
        Ok(Value::bool(vm.heap.identical(s, &arg(a, 0))))
    });
    vm.def(&o, "!=", |vm, s, a, _| {
        Ok(Value::bool(!vm.equal(s, &arg(a, 0))?))
    });
    vm.def(&o, "equal?", |vm, s, a, _| {
        Ok(Value::bool(vm.heap.identical(s, &arg(a, 0))))
    });
    vm.def(&o, "eql?", |vm, s, a, _| Ok(Value::bool(eql(&vm.heap, s, &arg(a, 0)))));
    // agrees with eql? (the hash a Hash key gets); `>> 1` keeps it positive
    vm.def(&o, "hash", |vm, s, _, _| {
        let h = (hash_key(&vm.heap, s) >> 1) as i64;
        Ok(vm.int(h))
    });
    vm.def(&o, "===", |vm, s, a, _| {
        Ok(Value::bool(vm.equal(s, &arg(a, 0))?))
    });
    // lang-fork: `x.none?` (Enumerable#none? comes first for collections)
    vm.def(&o, "none?", |_, _, _, _| Ok(Value::FALSE));
    vm.def(&o, "is_a?", is_a);
    vm.def(&o, "kind_of?", is_a);
    vm.def(&o, "instance_of?", |vm, s, a, _| match vm.kind(arg(a, 0)) {
        V::Class(c) => Ok(Value::bool(vm.class_of(s) == c)),
        _ => Err(vm.type_err("class or module required")),
    });
    vm.def(&o, "send", send);
    vm.def(&o, "__send__", send);
    vm.def(&o, "__id__", |vm, s, a, _| {
        check_argc(vm, a, 0, 0)?;
        vm.send(s, "object_id", vec![])
    });
    vm.def(&o, "object_id", |vm, s, _, _| {
        let id = match vm.kind(*s) {
            V::Nil => 8,
            V::Bool(true) => 20,
            V::Bool(false) => 0,
            V::Int(i) => i.wrapping_mul(2).wrapping_add(1),
            V::Sym(s) => s.0 as i64 * 16 + 12,
            V::Float(f) => f.to_bits() as i64,
            _ => s.heap_id() as i64 * 8,
        };
        Ok(vm.int(id))
    });
    vm.def(&o, "freeze", |vm, s, _, _| {
        vm.freeze(s);
        Ok(*s)
    });
    vm.def(&o, "frozen?", |vm, s, _, _| Ok(Value::bool(vm.is_frozen(s))));
    vm.def(&o, "dup", dup);
    // like dup, and frozen when the original is (unless `freeze: false`)
    vm.def(&o, "clone", |vm, s, a, b| {
        let copy = dup(vm, s, &[], b)?;
        let keep = match a.first().map(|h| vm.kind(*h)) {
            Some(V::Hash(h)) => {
                let k = Value::sym(vm.intern("freeze"));
                vm.heap[h].borrow().get(&vm.heap, &k).is_none_or(|v| v.truthy())
            }
            _ => true,
        };
        if keep && vm.is_frozen(s) && !vm.heap.identical(s, &copy) {
            vm.freeze(&copy);
        }
        Ok(copy)
    });
    vm.def(&o, "itself", |_, s, _, _| Ok(*s));
    vm.def(&o, "tap", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        vm.call_block(&b, vec![*s])?;
        Ok(*s)
    });
    vm.def(&o, "then", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        vm.call_block(&b, vec![*s])
    });
    vm.def(&o, "instance_variable_get", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        Ok(vm.ivar_get(s, iv))
    });
    vm.def(&o, "instance_variable_set", |vm, s, a, _| {
        let iv = sym_of(vm, &arg(a, 0))?;
        vm.ivar_set(s, iv, arg(a, 1))?;
        Ok(arg(a, 1))
    });
    vm.def(&o, "instance_variables", |vm, s, _, _| {
        let names = vm.ivar_names(s).into_iter().map(Value::sym).collect();
        Ok(vm.array(names))
    });
}

fn puts(vm: &mut Vm, _: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    fn put_one(vm: &mut Vm, v: &Value) -> Result<(), Unwind> {
        if let V::Array(arr) = vm.kind(*v) {
            let items = vm.heap[arr].borrow().clone();
            if items.is_empty() {
                vm.write("\n");
            }
            for x in &items {
                put_one(vm, x)?;
            }
            return Ok(());
        }
        let s = vm.to_s(v)?;
        vm.write(&s);
        if !s.ends_with('\n') {
            vm.write("\n");
        }
        Ok(())
    }
    if a.is_empty() {
        vm.write("\n");
    }
    for v in a {
        put_one(vm, v)?;
    }
    Ok(Value::NIL)
}

fn obj_inspect(vm: &mut Vm, s: &Value, _: &[Value], _: Option<&Value>) -> VmResult {
    let V::Object(o) = vm.kind(*s) else {
        return Ok(vm.str(format!("#<{}>", vm.class_name(s))));
    };
    if vm.heap.identical(s, &vm.main_obj) {
        return Ok(vm.str("main"));
    }
    let ivars = vm.heap[o].ivars.borrow().items.clone();
    let mut parts = Vec::new();
    for (k, v) in &ivars {
        let name = vm.sym_name(*k).to_string();
        parts.push(format!("{name}={}", vm.inspect(v)?));
    }
    let name = vm.heap[vm.heap[o].class].name();
    Ok(vm.str(if parts.is_empty() {
        format!("#<{name}>")
    } else {
        format!("#<{name} {}>", parts.join(", "))
    }))
}

fn is_a(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    match vm.kind(arg(a, 0)) {
        V::Class(c) => Ok(Value::bool(vm.is_a(s, &c))),
        _ => Err(vm.type_err("class or module required")),
    }
}

fn send(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>) -> VmResult {
    let Some((name, rest)) = a.split_first() else {
        return Err(vm.arg_err("no method name given"));
    };
    let mid = sym_of(vm, name)?;
    vm.call_with_block(s, mid, rest.to_vec(), b.cloned())
}

/// `dup`/`clone`: a copy with the same class and instance variables.
fn dup(vm: &mut Vm, s: &Value, _: &[Value], _: Option<&Value>) -> VmResult {
    let copy = match vm.kind(*s) {
        V::Str(x) => { let v = vm.heap[x].borrow().clone(); vm.str(v) },
        V::Array(x) => { let v = vm.heap[x].borrow().clone(); vm.array(v) },
        V::Hash(x) => { let v = vm.heap[x].borrow().clone(); vm.hash(v) },
        _ => return dup_other(vm, s),
    };
    copy_extra(vm, s, &copy);
    Ok(copy)
}

/// Gives the String/Array/Hash `to` the class and instance variables of
/// `from` (a subclass instance's; see gc::Extra), as `dup` does.
pub fn copy_extra(vm: &mut Vm, from: &Value, to: &Value) {
    if let Some(x) = vm.heap.extra_of(from) {
        let (class, ivars) = (x.class, x.ivars.items.clone());
        if let Some(e) = vm.heap.extra_of_mut(to) {
            e.class = class;
            e.ivars.items = ivars;
        }
    }
}

fn dup_other(vm: &mut Vm, s: &Value) -> VmResult {
    Ok(match vm.kind(*s) {
        V::Object(o) => {
            let ivars = vm.heap[o].ivars.borrow().items.clone();
            let copy = RObject::new(vm.heap[o].class);
            *copy.ivars.borrow_mut() = IvTable { items: ivars };
            vm.object(copy)
        }
        _ => *s,
    })
}

fn raise(vm: &mut Vm, _: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    let exc = match a {
        [] => {
            let c = vm.c.runtime_error;
            vm.make_exception(&c, "unhandled exception")
        }
        [m] if let Some(m) = vm.string(m) => {
            let c = vm.c.runtime_error;
            vm.make_exception(&c, m)
        }
        [cls] if matches!(vm.kind(*cls), V::Class(_)) => vm.send(cls, "new", vec![])?,
        [cls, msg] if matches!(vm.kind(*cls), V::Class(_)) => vm.send(cls, "new", vec![*msg])?,
        [e] if matches!(vm.kind(*e), V::Object(_)) => *e,
        _ => return Err(vm.type_err("exception class/object expected")),
    };
    if !vm.is_a(&exc, &vm.c.exception.clone()) {
        return Err(vm.type_err("exception class/object expected"));
    }
    Err(Unwind::Error(exc))
}

/// The contents `Array.new(size = 0, default = nil) { |i| }` or
/// `Array.new(array)` makes.
pub fn array_new_items(vm: &mut Vm, a: &[Value], b: Option<&Value>) -> Result<Vec<Value>, Unwind> {
    check_argc(vm, a, 0, 2)?;
    if let [x] = a
        && let V::Array(src) = vm.kind(*x)
    {
        return Ok(vm.heap[src].borrow().clone());
    }
    let n = if a.is_empty() { 0 } else { int_of(vm, &a[0])? };
    if n < 0 {
        return Err(vm.arg_err("negative array size"));
    }
    let mut v = Vec::with_capacity(n as usize);
    for i in 0..n {
        v.push(match b {
            Some(b) => vm.call_block(b, vec![Value::small_int(i)])?,
            None => arg(a, 1),
        });
    }
    Ok(v)
}

fn class_new(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>) -> VmResult {
    let V::Class(cls) = vm.kind(*s) else { unreachable!() };
    if vm.heap[cls].is_abstract.get() {
        let n = vm.heap[cls].name();
        return Err(vm.type_err(format!("{n} is abstract: create one of its subclasses")));
    }
    if cls == vm.c.array {
        let v = array_new_items(vm, a, b)?;
        return Ok(vm.array(v));
    }
    // a subclass of Array, Hash or String: a value of that type with the
    // class, which `initialize` (a native one, or the class's calling
    // `super`) fills in
    if let Some(base) = vm.builtin_base(cls)
        && base != cls
    {
        let v = if base == vm.c.array {
            vm.array(vec![])
        } else if base == vm.c.hash {
            vm.hash(RHash::new())
        } else {
            vm.str(String::new())
        };
        vm.set_builtin_class(&v, cls);
        vm.call_with_block(&v, vm.s.initialize, a.to_vec(), b.cloned())?;
        return Ok(v);
    }
    if (cls == vm.c.class) || (cls == vm.c.module) {
        return super::reflect::new_class_or_module(vm, cls, a, b);
    }
    let c = &vm.c;
    if cls == c.hash {
        let mut h = RHash::new();
        h.default = arg(a, 0);
        h.default_proc = b.cloned();
        return Ok(vm.hash(h));
    }
    if cls == c.string {
        return Ok(vm.str(if a.is_empty() {
            String::new()
        } else {
            str_of(vm, &a[0])?
        }));
    }
    let builtin = [
        c.integer, c.float, c.symbol, c.nil, c.true_, c.false_, c.proc_, c.range, c.class, c.module,
    ];
    if vm.heap[cls].is_module || builtin.contains(&cls) {
        let msg = format!(
            "undefined method 'new' for {} {}",
            if vm.heap[cls].is_module { "module" } else { "class" },
            vm.heap[cls].name()
        );
        return Err(vm.err(&vm.c.no_method_error, msg));
    }
    if vm.heap[cls].is_singleton {
        return Err(vm.type_err("can't create instance of singleton class"));
    }
    let obj = vm.object(RObject::new(cls));
    vm.call_with_block(&obj, vm.s.initialize, a.to_vec(), b.cloned())?;
    Ok(obj)
}

/// `attr_reader` & co.; returns the names of the methods defined (so
/// `private attr_accessor :a` works).
fn attr(vm: &mut Vm, s: &Value, a: &[Value], reader: bool, writer: bool) -> VmResult {
    vm.check_frozen(s)?;
    let V::Class(cls) = vm.kind(*s) else { unreachable!() };
    // under module_function: private, and no module method is made
    let vis = match vm.scope_vis(cls) {
        Vis::ModuleFunction => Vis::Private,
        v => v,
    };
    let mut defined = Vec::new();
    for name in a {
        let n = sym_of(vm, name)?;
        let n = vm.sym_name(n).to_string();
        let iv = vm.intern(&format!("@{n}"));
        if reader {
            let m = vm.intern(&n);
            vm.heap[cls].methods.insert_vis(m, Method::AttrReader(iv), vis);
            defined.push(Value::sym(m));
        }
        if writer {
            let m = vm.intern(&format!("{n}="));
            vm.heap[cls].methods.insert_vis(m, Method::AttrWriter(iv), vis);
            defined.push(Value::sym(m));
        }
    }
    Ok(vm.array(defined))
}

fn init_module(vm: &mut Vm) {
    let m = vm.c.module;
    let name: NativeFn = |vm, s, _, _| match vm.kind(*s) {
        V::Class(c) => Ok(vm.str(vm.heap[c].name())),
        _ => Ok(Value::NIL),
    };
    vm.def(&m, "name", name);
    vm.def(&m, "to_s", name);
    vm.def(&m, "inspect", name);
    vm.def(&m, "attr_accessor", |vm, s, a, _| {
        attr(vm, s, a, true, true)
    });
    vm.def(&m, "attr_reader", |vm, s, a, _| attr(vm, s, a, true, false));
    vm.def(&m, "attr_writer", |vm, s, a, _| attr(vm, s, a, false, true));
    vm.def(&m, "ancestors", |vm, s, _, _| {
        let V::Class(cls) = vm.kind(*s) else { unreachable!() };
        Ok(vm.array(
            vm.ancestors(&cls).into_iter().map(Value::from).collect(),
        ))
    });
    vm.def(&m, "===", |vm, s, a, _| {
        let V::Class(cls) = vm.kind(*s) else { unreachable!() };
        Ok(Value::bool(vm.is_a(&arg(a, 0), &cls)))
    });
    vm.def(&m, "const_get", |vm, s, a, _| {
        let V::Class(cls) = vm.kind(*s) else { unreachable!() };
        let n = sym_of(vm, &arg(a, 0))?;
        let v = vm.heap[cls].consts.borrow().get(&n).cloned();
        v.ok_or_else(|| {
            vm.err(
                &vm.c.name_error,
                format!("uninitialized constant {}", vm.sym_name(n)),
            )
        })
    });
    // constants have no visibility
    vm.def(&m, "private_constant", |_, _, _, _| Ok(Value::NIL));

    let c = vm.c.class;
    vm.def(&c, "new", class_new);
    vm.def(&c, "superclass", |vm, s, _, _| match vm.kind(*s) {
        V::Class(c) => Ok(vm.heap[c].superclass.map(Value::from).unwrap_or(Value::NIL)),
        _ => Ok(Value::NIL),
    });
}

fn init_exception(vm: &mut Vm) {
    let e = vm.c.exception;
    vm.def(&e, "init", |vm, s, a, _| {
        let msg = match vm.kind(arg(a, 0)) {
            V::Nil => vm.str(vm.class_name(s)),
            _ => arg(a, 0),
        };
        vm.ivar_set(s, vm.s.message, msg)?;
        Ok(Value::NIL)
    });
    // NameError.new(msg = nil, name = nil), NoMethodError.new(msg, name, args = [])
    let ne = vm.c.name_error;
    vm.def(&ne, "init", |vm, s, a, _| {
        let msg = match vm.kind(arg(a, 0)) {
            V::Nil => vm.str(vm.class_name(s)),
            _ => arg(a, 0),
        };
        vm.ivar_set(s, vm.s.message, msg)?;
        vm.ivar_set(s, vm.s.e_name, arg(a, 1))?;
        if a.len() > 2 {
            vm.ivar_set(s, vm.s.e_args, a[2])?;
        }
        Ok(*s)
    });
    // the name given, or the one the VM's message names
    vm.def(&ne, "name", |vm, s, _, _| {
        if let Some(n) = vm.ivar_lookup(s, vm.s.e_name) {
            return Ok(n);
        }
        let m = vm.ivar_get(s, vm.s.message);
        let msg = vm.to_s(&m)?;
        let quoted = msg.split('\'').nth(1).filter(|_| msg.matches('\'').count() >= 2);
        let name = quoted.or_else(|| msg.strip_prefix("uninitialized constant ").map(|c| c.rsplit("::").next().unwrap_or(c)));
        Ok(match name {
            Some(n) => Value::sym(vm.intern(n)),
            None => Value::NIL,
        })
    });
    let nme = vm.c.no_method_error;
    vm.def(&nme, "args", |vm, s, _, _| match vm.ivar_lookup(s, vm.s.e_args) {
        Some(a) => Ok(a),
        None => Ok(vm.array(vec![])),
    });
    let message: NativeFn = |vm, s, _, _| {
        let m = vm.ivar_get(s, vm.s.message);
        Ok({ let v = vm.to_s(&m)?; vm.str(v) })
    };
    vm.def(&e, "message", message);
    vm.def(&e, "to_s", message);
    vm.def(&e, "inspect", |vm, s, _, _| {
        let m = vm.exc_message(s);
        Ok(vm.str(format!("#<{}: {}>", vm.class_name(s), m)))
    });
    vm.def(&e, "full_message", |vm, s, _, _| {
        Ok(vm.str(format!(
            "{} ({})",
            vm.exc_message(s),
            vm.class_name(s)
        )))
    });
    vm.def(&e, "backtrace", |_, _, _, _| Ok(Value::NIL));
    vm.def_singleton(&e, "exception", |vm, s, a, b| {
        let new = vm.intern("new");
        vm.call_with_block(s, new, a.to_vec(), b.cloned())
    });
    vm.def(&e, "exception", |vm, s, a, _| {
        if a.is_empty() {
            return Ok(*s);
        }
        let copy = dup(vm, s, &[], None)?;
        vm.ivar_set(&copy, vm.s.message, arg(a, 0))?;
        Ok(copy)
    });
    vm.def(&e, "==", |vm, s, a, _| {
        let o = arg(a, 0);
        Ok(Value::bool(
            vm.heap.identical(s, &o)
                || (vm.class_of(s) == vm.class_of(&o)
                    && vm.exc_message(s) == vm.exc_message(&o)),
        ))
    });
}

fn init_misc(vm: &mut Vm) {
    let p = vm.c.proc_;
    for name in ["call", "()", "yield", "[]"] {
        let s = vm.intern(name);
        vm.heap[p].methods.insert(s, Method::ProcCall);
    }
    vm.def(&p, "to_proc", |_, s, _, _| Ok(*s));
    vm.def(&p, "arity", |vm, s, _, _| match vm.kind(*s) {
        V::Proc(p) => Ok(vm.int(match &vm.heap[p].body {
            ProcBody::Ruby { irep, .. } if vm.heap[p].lambda => irep.spec.arity(),
            ProcBody::Ruby { irep, .. } => {
                // a proc's optional parameters don't count unless it also has a rest
                let s = &irep.spec;
                let m = (s.mandatory() + s.kw.iter().any(|(_, r)| *r) as usize) as i64;
                if s.rest { -(m + 1) } else { m }
            }
            _ => -1,
        })),
        _ => Ok(Value::small_int(-1)),
    });

    let n = vm.c.nil;
    vm.def(&n, "to_s", |vm, _, _, _| Ok(vm.str("")));
    vm.def(&n, "to_a", |vm, _, _, _| Ok(vm.array(vec![])));
    vm.def(&n, "to_i", |_, _, _, _| Ok(Value::small_int(0)));
    vm.def(&n, "inspect", |vm, _, _, _| Ok(vm.str("none")));
    vm.def(&n, "none?", |_, _, _, _| Ok(Value::TRUE));

    for cls in [vm.c.true_, vm.c.false_] {
        let to_s: NativeFn = |vm, s, _, _| Ok(vm.str(if s.truthy() { "true" } else { "false" }));
        vm.def(&cls, "to_s", to_s);
        vm.def(&cls, "inspect", to_s);
        vm.def(&cls, "&", |_, s, a, _| {
            Ok(Value::bool(s.truthy() && arg(a, 0).truthy()))
        });
        vm.def(&cls, "|", |_, s, a, _| {
            Ok(Value::bool(s.truthy() || arg(a, 0).truthy()))
        });
        vm.def(&cls, "^", |_, s, a, _| {
            Ok(Value::bool(s.truthy() != arg(a, 0).truthy()))
        });
    }

    let sy = vm.c.symbol;
    let to_s: NativeFn = |vm, s, _, _| Ok({ let v = vm.to_s(s)?; vm.str(v) });
    vm.def(&sy, "to_s", to_s);
    vm.def(&sy, "name", to_s);
    vm.def(&sy, "id2name", to_s);
    vm.def(&sy, "to_sym", |_, s, _, _| Ok(*s));
    vm.def(&sy, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&sy, "to_proc", |vm, s, _, _| match vm.kind(*s) {
        V::Sym(m) => Ok(vm.proc_value(ProcBody::SymCall(m))),
        _ => unreachable!(),
    });
    vm.def(&sy, "length", |vm, s, _, _| {
        Ok(Value::small_int(vm.to_s(s)?.chars().count() as i64))
    });
    vm.def(&sy, "size", |vm, s, _, _| {
        Ok(Value::small_int(vm.to_s(s)?.chars().count() as i64))
    });
    vm.def(&sy, "<=>", |vm, s, a, _| match vm.kind(arg(a, 0)) {
        V::Sym(o) => {
            let (x, y) = (vm.to_s(s)?, vm.sym_name(o).to_string());
            Ok(vm.int(x.cmp(&y) as i64))
        }
        _ => Ok(Value::NIL),
    });
}

fn init_comparable(vm: &mut Vm) {
    let c = vm.c.comparable;
    fn cmp(vm: &mut Vm, s: &Value, a: &[Value]) -> Result<Ordering, Unwind> {
        compare(vm, s, &arg(a, 0))
    }
    vm.def(&c, "<", |vm, s, a, _| {
        Ok(Value::bool(cmp(vm, s, a)?.is_lt()))
    });
    vm.def(&c, "<=", |vm, s, a, _| {
        Ok(Value::bool(cmp(vm, s, a)?.is_le()))
    });
    vm.def(&c, ">", |vm, s, a, _| {
        Ok(Value::bool(cmp(vm, s, a)?.is_gt()))
    });
    vm.def(&c, ">=", |vm, s, a, _| {
        Ok(Value::bool(cmp(vm, s, a)?.is_ge()))
    });
    vm.def(&c, "==", |vm, s, a, _| {
        if vm.heap.identical(s, &arg(a, 0)) {
            return Ok(Value::TRUE);
        }
        let r = vm.call_method(s, vm.s.cmp, vec![arg(a, 0)], None)?;
        Ok(Value::bool(matches!(vm.kind(r), V::Int(0))))
    });
    vm.def(&c, "between?", |vm, s, a, _| {
        Ok(Value::bool(
            compare(vm, s, &arg(a, 0))?.is_ge() && compare(vm, s, &arg(a, 1))?.is_le(),
        ))
    });
    vm.def(&c, "clamp", |vm, s, a, _| {
        let (lo, hi) = (arg(a, 0), arg(a, 1));
        if compare(vm, s, &lo)?.is_lt() {
            return Ok(lo);
        }
        if compare(vm, s, &hi)?.is_gt() {
            return Ok(hi);
        }
        Ok(*s)
    });
}

/// `format` / `sprintf` / `String#%` (mruby-sprintf).
pub fn format(vm: &mut Vm, fmt: &str, args: &[Value]) -> Result<String, Unwind> {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    let mut ai = 0;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        let (mut left, mut zero, mut plus, mut space) = (false, false, false, false);
        while let Some(&f) = chars.peek() {
            match f {
                '-' => left = true,
                '0' => zero = true,
                '+' => plus = true,
                ' ' => space = true,
                _ => break,
            }
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
            width = width * 10 + d as usize;
            chars.next();
        }
        let mut prec: Option<usize> = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = 0;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                p = p * 10 + d as usize;
                chars.next();
            }
            prec = Some(p);
        }
        let conv = chars
            .next()
            .ok_or_else(|| vm.arg_err("malformed format string - %"))?;
        let v = args
            .get(ai)
            .cloned()
            .ok_or_else(|| vm.arg_err("too few arguments"))?;
        ai += 1;
        let sign = |neg: bool| {
            if neg {
                "-"
            } else if plus {
                "+"
            } else if space {
                " "
            } else {
                ""
            }
        };
        let (prefix, body, numeric) = match conv {
            'd' | 'i' | 'u' => {
                let n = match vm.kind(v) {
                    V::Float(f) => f.floor() as i64,
                    V::Str(s) => vm.heap[s].borrow().trim().parse().unwrap_or(0),
                    _ => int_of(vm, &v)?,
                };
                (sign(n < 0), n.unsigned_abs().to_string(), true)
            }
            'f' | 'e' | 'g' => {
                let x = float_of(vm, &v)?;
                let p = prec.unwrap_or(6);
                let body = match conv {
                    'f' => format!("{:.*}", p, x.abs()),
                    'e' => {
                        let s = format!("{:.*e}", p, x.abs());
                        let (m, e) = s.split_once('e').unwrap();
                        let (es, ed) = e.strip_prefix('-').map(|d| ('-', d)).unwrap_or(('+', e));
                        format!("{m}e{es}{ed:0>2}")
                    }
                    _ => fmt_float(x.abs()),
                };
                (sign(x < 0.0), body, true)
            }
            'x' | 'X' | 'o' | 'b' => {
                let n = int_of(vm, &v)?;
                let a = n.unsigned_abs();
                let body = match conv {
                    'x' => format!("{a:x}"),
                    'X' => format!("{a:X}"),
                    'o' => format!("{a:o}"),
                    _ => format!("{a:b}"),
                };
                (sign(n < 0), body, true)
            }
            'c' => (
                "",
                match vm.kind(v) {
                    V::Str(s) => vm.heap[s]
                        .borrow()
                        .chars()
                        .next()
                        .map(String::from)
                        .unwrap_or_default(),
                    _ => char::from_u32(int_of(vm, &v)? as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                },
                false,
            ),
            's' => {
                let mut s = vm.to_s(&v)?;
                if let Some(p) = prec {
                    s = s.chars().take(p).collect();
                }
                ("", s, false)
            }
            'p' => ("", vm.inspect(&v)?, false),
            other => return Err(vm.arg_err(format!("malformed format string - %{other}"))),
        };
        let len = prefix.chars().count() + body.chars().count();
        let pad = width.saturating_sub(len);
        if left {
            out.push_str(prefix);
            out.push_str(&body);
            out.push_str(&" ".repeat(pad));
        } else if zero && numeric {
            out.push_str(prefix);
            out.push_str(&"0".repeat(pad));
            out.push_str(&body);
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(prefix);
            out.push_str(&body);
        }
    }
    Ok(out)
}
