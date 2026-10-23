//! Hash. mruby: `src/hash.c`, `mrblib/hash.rb`, `mruby-hash-ext`.

use super::*;
use crate::vm::VmResult;

fn pairs(vm: &Vm, s: &Value) -> Result<Vec<(Value, Value)>, Unwind> {
    Ok(vm.heap[this_hash(vm, s)?].borrow().to_vec())
}

fn pair(vm: &mut Vm, k: &Value, v: &Value) -> Value {
    vm.array(vec![*k, *v])
}

/// A new empty Hash: the value and its handle.
fn new_hash(vm: &mut Vm) -> (Value, HashRef) {
    let v = vm.hash(RHash::new());
    let V::Hash(h) = vm.kind(v) else { unreachable!() };
    (v, h)
}

pub fn init(vm: &mut Vm) {
    let c = vm.c.hash;
    vm.heap[c].includes.push(vm.c.enumerable);
    // Hash.new(default) { |h, k| }, and a subclass's `super` in initialize
    vm.def(&c, "initialize", |vm, s, a, b| {
        check_argc(vm, a, 0, 1)?;
        if b.is_some() && !a.is_empty() {
            return Err(vm.arg_err("wrong number of arguments (given 1, expected 0)"));
        }
        let mut h = vm.heap[this_hash(vm, s)?].borrow_mut();
        h.default = arg(a, 0);
        h.default_proc = b.cloned();
        Ok(Value::NIL)
    });
    // Hash[hash], Hash[[[k, v], ...]], Hash[k, v, ...]: an instance of the
    // receiver (a subclass too)
    vm.def_singleton(&c, "[]", |vm, s, a, _| {
        let pairs: Vec<(Value, Value)> = match a {
            [x] if matches!(vm.kind(*x), V::Hash(_)) => pairs(vm, x)?,
            [x] if let V::Array(items) = vm.kind(*x) => {
                let items = vm.heap[items].borrow().clone();
                let mut out = Vec::new();
                for item in items {
                    let V::Array(p) = vm.kind(item) else {
                        return Err(vm.arg_err("wrong element type (expected array)"));
                    };
                    let p = vm.heap[p].borrow();
                    match p.len() {
                        1 | 2 => out.push((p[0], p.get(1).copied().unwrap_or(Value::NIL))),
                        n => {
                            drop(p);
                            return Err(vm.arg_err(format!(
                                "invalid number of elements ({n} for 1..2)"
                            )));
                        }
                    }
                }
                out
            }
            _ if a.len() % 2 == 1 => return Err(vm.arg_err("odd number of arguments for Hash")),
            _ => a.chunks(2).map(|kv| (kv[0], kv[1])).collect(),
        };
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs {
            vm.hash_set(h, k, v)?;
        }
        if let V::Class(cls) = vm.kind(*s) {
            vm.set_builtin_class(&hv, cls);
        }
        Ok(hv)
    });
    vm.def(&c, "[]", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let key = arg(a, 0);
        if let Some(v) = vm.hash_get(h, &key)? {
            return Ok(v);
        }
        let (default, default_proc) = {
            let h = vm.heap[h].borrow();
            (h.default, h.default_proc)
        };
        match default_proc {
            Some(p) => vm.call_block(&p, vec![*s, key]),
            None => Ok(default),
        }
    });
    let store: NativeFn = |vm, s, a, _| {
        check_argc(vm, a, 2, 2)?;
        let h = this_hash(vm, s)?;
        vm.hash_set(h, a[0], a[1])?;
        Ok(a[1])
    };
    vm.def(&c, "[]=", store);
    vm.def(&c, "store", store);
    vm.def(&c, "fetch", |vm, s, a, b| {
        let key = arg(a, 0);
        let h = this_hash(vm, s)?;
        if let Some(v) = vm.hash_get(h, &key)? {
            return Ok(v);
        }
        if let Some(b) = b {
            return vm.call_block(b, vec![key]);
        }
        if a.len() > 1 {
            return Ok(a[1]);
        }
        let k = vm.inspect(&key)?;
        Err(vm.err(&vm.c.key_error, format!("key not found: {k}")))
    });
    vm.def(&c, "dig", |vm, s, a, _| {
        let mut cur = *s;
        for k in a {
            cur = match vm.kind(cur) {
                V::Nil => return Ok(Value::NIL),
                _ => vm.send(&cur, "[]", vec![*k])?,
            };
        }
        Ok(cur)
    });
    let has_key: NativeFn = |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        Ok(Value::bool(vm.hash_find(h, &arg(a, 0))?.is_some()))
    };
    for name in ["key?", "has_key?", "include?", "member?"] {
        vm.def(&c, name, has_key);
    }
    vm.def(&c, "value?", |vm, s, a, _| {
        Ok(Value::bool(
            pairs(vm, s)?.iter().any(|(_, v)| values_eq(&vm.heap, v, &arg(a, 0))),
        ))
    });
    vm.def(&c, "key", |vm, s, a, _| {
        Ok(pairs(vm, s)?
            .into_iter()
            .find(|(_, v)| values_eq(&vm.heap, v, &arg(a, 0)))
            .map(|(k, _)| k)
            .unwrap_or(Value::NIL))
    });
    vm.def(&c, "keys", |vm, s, _, _| {
        Ok(vm.array(
            pairs(vm, s)?.into_iter().map(|(k, _)| k).collect(),
        ))
    });
    vm.def(&c, "values", |vm, s, _, _| {
        Ok(vm.array(
            pairs(vm, s)?.into_iter().map(|(_, v)| v).collect(),
        ))
    });
    vm.def(&c, "length", |vm, s, _, _| {
        Ok(Value::small_int(vm.heap[this_hash(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::small_int(vm.heap[this_hash(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "count", |vm, s, a, b| match b {
        None if a.is_empty() => Ok(Value::small_int(vm.heap[this_hash(vm, s)?].borrow().len() as i64)),
        _ => enumerable::count(vm, s, a, b),
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::bool(vm.heap[this_hash(vm, s)?].borrow().is_empty()))
    });
    vm.def(&c, "to_h", |_, s, _, _| Ok(*s));
    vm.def(&c, "to_s", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "==", |vm, s, a, _| {
        Ok(Value::bool(values_eq(&vm.heap, s, &arg(a, 0))))
    });
    vm.def(&c, "default", |vm, s, _, _| {
        Ok(vm.heap[this_hash(vm, s)?].borrow().default)
    });
    vm.def(&c, "default=", |vm, s, a, _| {
        vm.heap[this_hash(vm, s)?].borrow_mut().default = arg(a, 0);
        Ok(arg(a, 0))
    });
    let each: NativeFn = |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (k, v) in pairs(vm, s)? {
            let kv = pair(vm, &k, &v);
            vm.call_block(&b, vec![kv])?;
        }
        Ok(*s)
    };
    vm.def(&c, "each", each);
    vm.def(&c, "each_pair", each);
    vm.def(&c, "each_key", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (k, _) in pairs(vm, s)? {
            vm.call_block(&b, vec![k])?;
        }
        Ok(*s)
    });
    vm.def(&c, "each_value", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (_, v) in pairs(vm, s)? {
            vm.call_block(&b, vec![v])?;
        }
        Ok(*s)
    });
    vm.def(&c, "delete", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        Ok(vm.hash_delete(h, &arg(a, 0))?.unwrap_or(Value::NIL))
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        vm.heap[this_hash(vm, s)?].borrow_mut().clear();
        Ok(*s)
    });
    vm.def(&c, "rehash", |vm, s, _, _| {
        let h = this_hash(vm, s)?;
        vm.hash_rehash(h)?;
        Ok(*s)
    });
    vm.def(&c, "merge", |vm, s, a, b| {
        let copy = vm.heap[this_hash(vm, s)?].borrow().clone();
        let hv = vm.hash(copy);
        // a copy of the receiver: its class and ivars too
        super::kernel::copy_extra(vm, s, &hv);
        merge_into(vm, this_hash(vm, &hv)?, a, b)?;
        Ok(hv)
    });
    let update: NativeFn = |vm, s, a, b| {
        merge_into(vm, this_hash(vm, s)?, a, b)?;
        Ok(*s)
    };
    vm.def(&c, "merge!", update);
    vm.def(&c, "update", update);
    vm.def(&c, "select", |vm, s, _, b| filter(vm, s, b, true));
    vm.def(&c, "filter", |vm, s, _, b| filter(vm, s, b, true));
    vm.def(&c, "reject", |vm, s, _, b| filter(vm, s, b, false));
    vm.def(&c, "transform_values", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs(vm, s)? {
            let v = vm.call_block(&b, vec![v])?;
            vm.hash_set(h, k, v)?;
        }
        Ok(hv)
    });
    vm.def(&c, "transform_keys", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs(vm, s)? {
            let k = vm.call_block(&b, vec![k])?;
            vm.hash_set(h, k, v)?;
        }
        Ok(hv)
    });
    vm.def(&c, "invert", |vm, s, _, _| {
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs(vm, s)? {
            vm.hash_set(h, v, k)?;
        }
        Ok(hv)
    });
    vm.def(&c, "values_at", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let default = vm.heap[h].borrow().default;
        let mut values = Vec::new();
        for k in a {
            values.push(vm.hash_get(h, k)?.unwrap_or(default));
        }
        Ok(vm.array(values))
    });
    vm.def(&c, "sort", |vm, s, _, b| {
        let v = to_vec(vm, s)?;
        Ok({ let v = sort_values(vm, v, b)?; vm.array(v) })
    });

    let a = vm.c.array;
    vm.def(&a, "to_h", |vm, s, _, b| {
        let (hv, h) = new_hash(vm);
        let items = vm.heap[this_ary(vm, s)?].borrow().clone();
        for x in items {
            let x = match b {
                Some(b) => vm.call_block(b, vec![x])?,
                None => x,
            };
            let V::Array(p) = vm.kind(x) else {
                return Err(vm.type_err("wrong element type (expected array)"));
            };
            let (k, v) = {
                let p = vm.heap[p].borrow();
                (p.first().cloned().unwrap_or(Value::NIL), p.get(1).cloned().unwrap_or(Value::NIL))
            };
            vm.hash_set(h, k, v)?;
        }
        Ok(hv)
    });
}

fn merge_into(vm: &mut Vm, h: HashRef, others: &[Value], b: Option<&Value>) -> Result<(), Unwind> {
    for o in others {
        for (k, v) in pairs(vm, o)? {
            let old = vm.hash_get(h, &k)?;
            let v = match (b, old) {
                (Some(b), Some(old)) => vm.call_block(b, vec![k, old, v])?,
                _ => v,
            };
            vm.hash_set(h, k, v)?;
        }
    }
    Ok(())
}

fn filter(vm: &mut Vm, s: &Value, b: Option<&Value>, keep: bool) -> VmResult {
    let b = need_block(vm, b)?;
    let (hv, h) = new_hash(vm);
    for (k, v) in pairs(vm, s)? {
        if vm.call_block(&b, vec![k, v])?.truthy() == keep {
            vm.hash_set(h, k, v)?;
        }
    }
    Ok(hv)
}
