//! Hash. mruby: `src/hash.c`, `mrblib/hash.rb`, `mruby-hash-ext`.

use super::*;
use crate::vm::VmResult;

fn pairs(vm: &Vm, s: &Value) -> Result<Vec<(Value, Value)>, Unwind> {
    Ok(vm.heap[this_hash(vm, s)?].borrow().pairs.clone())
}

fn pair(vm: &mut Vm, k: &Value, v: &Value) -> Value {
    vm.array(vec![*k, *v])
}

/// A new empty Hash: the value and its handle.
fn new_hash(vm: &mut Vm) -> (Value, HashRef) {
    let v = vm.hash(RHash::new());
    let Value::Hash(h) = v else { unreachable!() };
    (v, h)
}

pub fn init(vm: &mut Vm) {
    let c = vm.c.hash;
    vm.heap[c].includes.push(vm.c.enumerable);
    vm.def(&c, "[]", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let key = arg(a, 0);
        let (found, default, default_proc) = {
            let h = vm.heap[h].borrow();
            (h.get(&vm.heap, &key), h.default, h.default_proc)
        };
        match (found, default_proc) {
            (Some(v), _) => Ok(v),
            (None, Some(p)) => vm.call_block(&p, vec![*s, key]),
            (None, None) => Ok(default),
        }
    });
    let store: NativeFn = |vm, s, a, _| {
        check_argc(vm, a, 2, 2)?;
        let h = this_hash(vm, s)?;
        vm.hash_set(h, a[0], a[1]);
        Ok(a[1])
    };
    vm.def(&c, "[]=", store);
    vm.def(&c, "store", store);
    vm.def(&c, "fetch", |vm, s, a, b| {
        let key = arg(a, 0);
        if let Some(v) = vm.heap[this_hash(vm, s)?].borrow().get(&vm.heap, &key) {
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
            cur = match &cur {
                Value::Nil => return Ok(Value::Nil),
                _ => vm.send(&cur, "[]", vec![*k])?,
            };
        }
        Ok(cur)
    });
    let has_key: NativeFn = |vm, s, a, _| {
        Ok(Value::Bool(
            vm.heap[this_hash(vm, s)?].borrow().get(&vm.heap, &arg(a, 0)).is_some(),
        ))
    };
    for name in ["key?", "has_key?", "include?", "member?"] {
        vm.def(&c, name, has_key);
    }
    vm.def(&c, "value?", |vm, s, a, _| {
        Ok(Value::Bool(
            pairs(vm, s)?.iter().any(|(_, v)| values_eq(&vm.heap, v, &arg(a, 0))),
        ))
    });
    vm.def(&c, "key", |vm, s, a, _| {
        Ok(pairs(vm, s)?
            .into_iter()
            .find(|(_, v)| values_eq(&vm.heap, v, &arg(a, 0)))
            .map(|(k, _)| k)
            .unwrap_or(Value::Nil))
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
        Ok(Value::Int(vm.heap[this_hash(vm, s)?].borrow().pairs.len() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::Int(vm.heap[this_hash(vm, s)?].borrow().pairs.len() as i64))
    });
    vm.def(&c, "count", |vm, s, a, b| match b {
        None if a.is_empty() => Ok(Value::Int(vm.heap[this_hash(vm, s)?].borrow().pairs.len() as i64)),
        _ => enumerable::count(vm, s, a, b),
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::Bool(vm.heap[this_hash(vm, s)?].borrow().pairs.is_empty()))
    });
    vm.def(&c, "to_h", |_, s, _, _| Ok(*s));
    vm.def(&c, "to_s", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "==", |vm, s, a, _| {
        Ok(Value::Bool(values_eq(&vm.heap, s, &arg(a, 0))))
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
        Ok(vm.heap[this_hash(vm, s)?].borrow_mut()
            .remove(&vm.heap, &arg(a, 0))
            .unwrap_or(Value::Nil))
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        vm.heap[this_hash(vm, s)?].borrow_mut().pairs.clear();
        Ok(*s)
    });
    vm.def(&c, "merge", |vm, s, a, b| {
        let copy = vm.heap[this_hash(vm, s)?].borrow().clone();
        let hv = vm.hash(copy);
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
            vm.hash_set(h, k, v);
        }
        Ok(hv)
    });
    vm.def(&c, "transform_keys", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs(vm, s)? {
            let k = vm.call_block(&b, vec![k])?;
            vm.hash_set(h, k, v);
        }
        Ok(hv)
    });
    vm.def(&c, "invert", |vm, s, _, _| {
        let (hv, h) = new_hash(vm);
        for (k, v) in pairs(vm, s)? {
            vm.hash_set(h, v, k);
        }
        Ok(hv)
    });
    vm.def(&c, "values_at", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let values: Vec<Value> = {
            let h = vm.heap[h].borrow();
            a.iter()
                .map(|k| h.get(&vm.heap, k).unwrap_or_else(|| h.default))
                .collect()
        };
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
            let Value::Array(p) = &x else {
                return Err(vm.type_err("wrong element type (expected array)"));
            };
            let (k, v) = {
                let p = vm.heap[p].borrow();
                (p.first().cloned().unwrap_or(Value::Nil), p.get(1).cloned().unwrap_or(Value::Nil))
            };
            vm.hash_set(h, k, v);
        }
        Ok(hv)
    });
}

fn merge_into(vm: &mut Vm, h: HashRef, others: &[Value], b: Option<&Value>) -> Result<(), Unwind> {
    for o in others {
        for (k, v) in pairs(vm, o)? {
            let old = vm.heap[h].borrow().get(&vm.heap, &k);
            let v = match (b, old) {
                (Some(b), Some(old)) => vm.call_block(b, vec![k, old, v])?,
                _ => v,
            };
            vm.hash_set(h, k, v);
        }
    }
    Ok(())
}

fn filter(vm: &mut Vm, s: &Value, b: Option<&Value>, keep: bool) -> VmResult {
    let b = need_block(vm, b)?;
    let (hv, h) = new_hash(vm);
    for (k, v) in pairs(vm, s)? {
        if vm.call_block(&b, vec![k, v])?.truthy() == keep {
            vm.hash_set(h, k, v);
        }
    }
    Ok(hv)
}
