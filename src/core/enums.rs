//! Enums (lang-fork), like Rust's:
//!
//! ```text
//! enum Shape {
//!   Circle(r)          # a tuple variant: Shape::Circle(2)
//!   Rect { w, h }      # a struct variant: Shape::Rect(w: 2, h: 3)
//!   Empty              # a unit variant: the value Shape::Empty
//!   fn area() = ...    # methods, impl blocks, constants as in a class
//! }
//! ```
//!
//! The enum is a class without `new`. Each variant with fields is a
//! subclass of it (the constant `Shape::Circle`), created with
//! `Shape::Circle(...)` (a `Method::Variant` on the enum); a unit variant is
//! the one frozen instance of its own subclass (the constant `Shape::Empty`).
//! Values are frozen, compare by value, print like Rust's `Debug`
//! (`Circle(2)`, `Rect(w: 2, h: 3)`, `Empty`) and take part in pattern
//! matching (`in Circle(r)`, `in Rect(w:, h:)`, `in Empty`).
//!
//! The parser turns an `enum` into a class whose body starts with
//! `__enum(line)` and `__variant(:Name, [:field, ...], kind)` calls.

use super::arg;
use crate::gc::ClassRef;
use crate::rhash::RHash;
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{Unwind, Vm, VmResult};
use std::rc::Rc;

fn class_of_value(vm: &Vm, v: &Value) -> Option<ClassRef> {
    match vm.kind(*v) {
        V::Class(c) => Some(c),
        _ => None,
    }
}

/// The variant `v` is, with its class.
fn variant_of(vm: &Vm, v: &Value) -> Option<(ClassRef, Rc<VariantInfo>)> {
    let c = vm.class_of(v);
    let info = vm.heap[c].variant.borrow().clone()?;
    Some((c, info))
}

fn this_variant(vm: &Vm, v: &Value) -> Result<(ClassRef, Rc<VariantInfo>), Unwind> {
    variant_of(vm, v).ok_or_else(|| vm.type_err("not an enum variant"))
}

/// The enum a variant class belongs to.
fn enum_of(vm: &Vm, variant_class: ClassRef) -> Option<ClassRef> {
    vm.heap[variant_class].superclass
}

fn fields(vm: &Vm, v: &Value, info: &VariantInfo) -> Vec<Value> {
    info.ivars.iter().map(|iv| vm.ivar_get(v, *iv)).collect()
}

/// `Rect(w: 2, h: 3)`'s keyword arguments, in field order.
fn named_args(vm: &mut Vm, name: &str, info: &VariantInfo, a: &[Value]) -> Result<Vec<Value>, Unwind> {
    let given: Vec<(Value, Value)> = match a {
        [] => vec![],
        [h] => match vm.kind(*h) {
            V::Hash(h) if vm.heap[h].borrow().kw => vm.heap[h].borrow().to_vec(),
            _ => return Err(vm.arg_err(format!("{name} takes its fields by name: {name}(field: value, ...)"))),
        },
        _ => return Err(vm.arg_err(format!("{name} takes its fields by name: {name}(field: value, ...)"))),
    };
    let mut out = vec![Value::NIL; info.fields.len()];
    let mut set = vec![false; info.fields.len()];
    for (k, v) in given {
        let i = match vm.kind(k) {
            V::Sym(s) => info.fields.iter().position(|f| *f == s),
            _ => None,
        };
        let Some(i) = i else {
            let k = vm.inspect(&k)?;
            return Err(vm.arg_err(format!("{name} has no field {k}")));
        };
        out[i] = v;
        set[i] = true;
    }
    let missing: Vec<String> =
        info.fields.iter().zip(&set).filter(|(_, s)| !**s).map(|(f, _)| vm.sym_name(*f).to_string()).collect();
    if !missing.is_empty() {
        return Err(vm.arg_err(format!("{name} is missing {}", missing.join(", "))));
    }
    Ok(out)
}

fn init(vm: &mut Vm, s: &Value, a: &[Value], _b: Option<&Value>) -> VmResult {
    let (c, info) = this_variant(vm, s)?;
    let values = if info.named {
        let name = vm.heap[c].name();
        named_args(vm, &name, &info, a)?
    } else {
        if a.len() != info.fields.len() {
            let name = vm.heap[c].name();
            let n = info.fields.len();
            let fs = if n == 1 { "field" } else { "fields" };
            return Err(vm.arg_err(format!("{name} takes {n} {fs}, given {}", a.len())));
        }
        a.to_vec()
    };
    for (iv, v) in info.ivars.iter().zip(values) {
        vm.ivar_set(s, *iv, v)?;
    }
    vm.freeze(s);
    Ok(Value::NIL)
}

fn inspect(vm: &mut Vm, s: &Value, _a: &[Value], _b: Option<&Value>) -> VmResult {
    let Some((_, info)) = variant_of(vm, s) else {
        return Ok(vm.str("#<enum>"));
    };
    let mut out = vm.sym_name(info.name).to_string();
    if !info.unit {
        let mut parts = vec![];
        for (f, v) in info.fields.iter().zip(fields(vm, s, &info)) {
            let shown = vm.inspect(&v)?;
            parts.push(if info.named { format!("{}: {shown}", vm.sym_name(*f)) } else { shown });
        }
        out = format!("{out}({})", parts.join(", "));
    }
    Ok(vm.str(out))
}

fn same_variant(vm: &Vm, s: &Value, o: &Value) -> Option<Rc<VariantInfo>> {
    let (c, info) = variant_of(vm, s)?;
    let (d, _) = variant_of(vm, o)?;
    (c == d).then_some(info)
}

fn eq(vm: &mut Vm, s: &Value, a: &[Value], _b: Option<&Value>) -> VmResult {
    let o = arg(a, 0);
    let Some(info) = same_variant(vm, s, &o) else {
        return Ok(Value::FALSE);
    };
    for (x, y) in fields(vm, s, &info).into_iter().zip(fields(vm, &o, &info)) {
        if !vm.equal(&x, &y)? {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

/// `[:Name, fields...]`, for hash and eql?.
fn key_array(vm: &mut Vm, s: &Value, info: &VariantInfo) -> Value {
    let mut v = vec![Value::sym(info.name)];
    v.extend(fields(vm, s, info));
    vm.array(v)
}

pub fn init_enums(vm: &mut Vm) {
    let m = vm.c.module;
    // `enum E {`: self is the new class
    vm.def(&m, "__enum", |vm, s, a, _| {
        let line = match vm.kind(arg(a, 0)) {
            V::Int(n) => n,
            _ => 0,
        };
        let Some(e) = class_of_value(vm, s) else {
            return Err(vm.type_err("__enum outside an enum"));
        };
        let n = vm.heap[e].name();
        if vm.heap[e].enum_variants.borrow().is_some() {
            return Err(vm.type_err(format!("line {line}: enum {n} is already defined")));
        }
        let k = &vm.heap[e];
        if k.bodies.get() > 1
            || !k.methods.borrow().is_empty()
            || !k.consts.borrow().is_empty()
            || k.superclass != Some(vm.c.object)
        {
            return Err(vm.type_err(format!("line {line}: {n} is already a class")));
        }
        *vm.heap[e].enum_variants.borrow_mut() = Some(vec![]);
        // ordered by variant (`<=>` below): <, max, sort, clamp...
        let cmp = vm.c.comparable;
        vm.heap[e].includes.push(cmp);
        // the values' methods; the enum body may override them
        vm.def(&e, "init", init);
        vm.def(&e, "inspect", inspect);
        vm.def(&e, "to_s", inspect);
        vm.def(&e, "==", eq);
        vm.def(&e, "eql?", |vm, s, a, _| {
            let o = arg(a, 0);
            let Some(info) = same_variant(vm, s, &o) else {
                return Ok(Value::FALSE);
            };
            let (x, y) = (key_array(vm, s, &info), key_array(vm, &o, &info));
            vm.send(&x, "eql?", vec![y])
        });
        vm.def(&e, "hash", |vm, s, _, _| {
            let (_, info) = this_variant(vm, s)?;
            let x = key_array(vm, s, &info);
            vm.send(&x, "hash", vec![])
        });
        vm.def(&e, "deconstruct", |vm, s, _, _| {
            let (_, info) = this_variant(vm, s)?;
            let v = fields(vm, s, &info);
            Ok(vm.array(v))
        });
        vm.def(&e, "deconstruct_keys", |vm, s, _, _| {
            let (_, info) = this_variant(vm, s)?;
            let mut h = RHash::new();
            for (f, v) in info.fields.iter().zip(fields(vm, s, &info)) {
                h.set(&vm.heap, Value::sym(*f), v);
            }
            Ok(vm.hash(h))
        });
        vm.def(&e, "variant", |vm, s, _, _| Ok(Value::sym(this_variant(vm, s)?.1.name)));
        vm.def(&e, "ordinal", |vm, s, _, _| Ok(Value::small_int(this_variant(vm, s)?.1.ordinal as i64)));
        // by variant order, then by fields; none across enums
        vm.def(&e, "<=>", |vm, s, a, _| {
            let o = arg(a, 0);
            let (Some((c, x)), Some((d, y))) = (variant_of(vm, s), variant_of(vm, &o)) else {
                return Ok(Value::NIL);
            };
            if enum_of(vm, c) != enum_of(vm, d) {
                return Ok(Value::NIL);
            }
            if x.ordinal != y.ordinal {
                return Ok(Value::small_int(if x.ordinal < y.ordinal { -1 } else { 1 }));
            }
            let (l, r) = (fields(vm, s, &x), fields(vm, &o, &y));
            let (l, r) = (vm.array(l), vm.array(r));
            vm.send(&l, "<=>", vec![r])
        });
        // the enum's own functions: `Shape::variants`, `Align::from(:center)`
        let meta = vm.singleton_class(s)?;
        vm.def(&meta, "variants", |vm, s, _, _| {
            let e = class_of_value(vm, s).unwrap();
            let names = vm.heap[e].enum_variants.borrow().clone().unwrap_or_default();
            let v = names.iter().filter_map(|n| vm.heap[e].consts.borrow().get(n).copied()).collect();
            Ok(vm.array(v))
        });
        vm.def(&meta, "from", |vm, s, a, _| {
            let e = class_of_value(vm, s).unwrap();
            let want = match vm.kind(arg(a, 0)) {
                V::Sym(x) => vm.sym_name(x).to_string(),
                V::Str(x) => vm.heap[x].borrow().clone(),
                _ => String::new(),
            };
            let norm = |s: &str| s.replace('_', "").to_lowercase();
            let names = vm.heap[e].enum_variants.borrow().clone().unwrap_or_default();
            let found = names.iter().find(|n| norm(vm.sym_name(**n)) == norm(&want)).copied();
            let value = found.and_then(|n| vm.heap[e].consts.borrow().get(&n).copied());
            let unit = value.is_some_and(|v| variant_of(vm, &v).is_some_and(|(_, i)| i.unit));
            let r = match value {
                Some(v) if unit => Ok(v),
                _ => {
                    let what = vm.inspect(&arg(a, 0))?;
                    let n = vm.heap[e].name();
                    Err(vm.str(format!("{n} has no variant {what} without fields")))
                }
            };
            let (ok, err) = match vm.results {
                Some((ok, err, _)) => (ok, err),
                None => return Err(vm.type_err("from before result.rb")),
            };
            let (class, v) = match r {
                Ok(v) => (ok, v),
                Err(e) => (err, e),
            };
            vm.send(&Value::from(class), "new", vec![v])
        });
        Ok(Value::NIL)
    });
    // a variant: (name, [fields], kind: 0 unit, 1 tuple, 2 struct)
    vm.def(&m, "__variant", |vm, s, a, _| {
        let Some(e) = class_of_value(vm, s) else {
            return Err(vm.type_err("__variant outside an enum"));
        };
        let V::Sym(name) = vm.kind(arg(a, 0)) else {
            return Err(vm.type_err("__variant: bad name"));
        };
        let field_names: Vec<Sym> = match vm.kind(arg(a, 1)) {
            V::Array(x) => vm.heap[x]
                .borrow()
                .iter()
                .filter_map(|v| match vm.heap.kind(*v) {
                    V::Sym(f) => Some(f),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        let kind = match vm.kind(arg(a, 2)) {
            V::Int(k) => k,
            _ => 0,
        };
        let ename = vm.heap[e].name();
        let vname = vm.sym_name(name).to_string();
        let ordinal = {
            let mut vs = vm.heap[e].enum_variants.borrow_mut();
            let Some(vs) = vs.as_mut() else {
                return Err(vm.type_err("__variant outside an enum"));
            };
            if vs.contains(&name) {
                return Err(vm.type_err(format!("enum {ename}: variant {vname} is defined twice")));
            }
            vs.push(name);
            vs.len() - 1
        };
        for (i, f) in field_names.iter().enumerate() {
            if field_names[..i].contains(f) {
                let f = vm.sym_name(*f).to_string();
                return Err(vm.type_err(format!("enum {ename}: {vname} has the field {f} twice")));
            }
        }
        let c = vm.new_class(&format!("{ename}::{vname}"), Some(e), false);
        vm.heap[c].outer.set(Some(e));
        let ivars: Vec<Sym> = field_names.iter().map(|f| vm.intern(&format!("@{}", vm.sym_name(*f)))).collect();
        for (f, iv) in field_names.iter().zip(&ivars) {
            vm.heap[c].methods.insert(*f, Method::AttrReader(*iv));
        }
        let unit = kind == 0;
        *vm.heap[c].variant.borrow_mut() =
            Some(Rc::new(VariantInfo { name, fields: field_names, ivars, named: kind == 2, unit, ordinal }));
        if unit {
            // the one value
            let v = vm.object(RObject::new(c));
            vm.freeze(&v);
            vm.store_const(&e, name, v);
        } else {
            vm.store_const(&e, name, Value::from(c));
            let meta = vm.singleton_class(s)?;
            vm.heap[meta].methods.insert(name, Method::Variant(c));
            // `Circle(2)` inside the enum's own methods, where self is a value
            vm.heap[e].methods.insert_vis(name, Method::Variant(c), Vis::Private);
        }
        Ok(Value::NIL)
    });
}
