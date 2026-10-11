//! Hash. mruby: `src/hash.c`, `mrblib/hash.rb`, `mruby-hash-ext`.

use super::*;
use crate::vm::VmResult;

fn pairs(vm: &Vm, s: &Value) -> Result<Vec<(Value, Value)>, Unwind> {
    Ok(this_hash(vm, s)?.borrow().pairs.clone())
}

fn pair(k: &Value, v: &Value) -> Value {
    Value::array(vec![k.clone(), v.clone()])
}

pub fn init(vm: &mut Vm) {
    let c = vm.c.hash.clone();
    c.includes.borrow_mut().push(vm.c.enumerable.clone());
    vm.def(&c, "[]", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let key = arg(a, 0);
        let (found, default, default_proc) = {
            let h = h.borrow();
            (h.get(&key), h.default.clone(), h.default_proc.clone())
        };
        match (found, default_proc) {
            (Some(v), _) => Ok(v),
            (None, Some(p)) => vm.call_block(&p, vec![s.clone(), key]),
            (None, None) => Ok(default),
        }
    });
    let store: NativeFn = |vm, s, a, _| {
        check_argc(vm, a, 2, 2)?;
        this_hash(vm, s)?
            .borrow_mut()
            .set(a[0].clone(), a[1].clone());
        Ok(a[1].clone())
    };
    vm.def(&c, "[]=", store);
    vm.def(&c, "store", store);
    vm.def(&c, "fetch", |vm, s, a, b| {
        let key = arg(a, 0);
        if let Some(v) = this_hash(vm, s)?.borrow().get(&key) {
            return Ok(v);
        }
        if let Some(b) = b {
            return vm.call_block(b, vec![key]);
        }
        if a.len() > 1 {
            return Ok(a[1].clone());
        }
        let k = vm.inspect(&key)?;
        Err(vm.err(&vm.c.key_error, format!("key not found: {k}")))
    });
    vm.def(&c, "dig", |vm, s, a, _| {
        let mut cur = s.clone();
        for k in a {
            cur = match &cur {
                Value::Nil => return Ok(Value::Nil),
                _ => vm.send(&cur, "[]", vec![k.clone()])?,
            };
        }
        Ok(cur)
    });
    let has_key: NativeFn = |vm, s, a, _| {
        Ok(Value::Bool(
            this_hash(vm, s)?.borrow().get(&arg(a, 0)).is_some(),
        ))
    };
    for name in ["key?", "has_key?", "include?", "member?"] {
        vm.def(&c, name, has_key);
    }
    vm.def(&c, "value?", |vm, s, a, _| {
        Ok(Value::Bool(
            pairs(vm, s)?.iter().any(|(_, v)| values_eq(v, &arg(a, 0))),
        ))
    });
    vm.def(&c, "key", |vm, s, a, _| {
        Ok(pairs(vm, s)?
            .into_iter()
            .find(|(_, v)| values_eq(v, &arg(a, 0)))
            .map(|(k, _)| k)
            .unwrap_or(Value::Nil))
    });
    vm.def(&c, "keys", |vm, s, _, _| {
        Ok(Value::array(
            pairs(vm, s)?.into_iter().map(|(k, _)| k).collect(),
        ))
    });
    vm.def(&c, "values", |vm, s, _, _| {
        Ok(Value::array(
            pairs(vm, s)?.into_iter().map(|(_, v)| v).collect(),
        ))
    });
    vm.def(&c, "length", |vm, s, _, _| {
        Ok(Value::Int(this_hash(vm, s)?.borrow().pairs.len() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::Int(this_hash(vm, s)?.borrow().pairs.len() as i64))
    });
    vm.def(&c, "count", |vm, s, a, b| match b {
        None if a.is_empty() => Ok(Value::Int(this_hash(vm, s)?.borrow().pairs.len() as i64)),
        _ => enumerable::count(vm, s, a, b),
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::Bool(this_hash(vm, s)?.borrow().pairs.is_empty()))
    });
    vm.def(&c, "to_h", |_, s, _, _| Ok(s.clone()));
    vm.def(&c, "to_s", |vm, s, _, _| Ok(Value::str(vm.inspect(s)?)));
    vm.def(&c, "inspect", |vm, s, _, _| Ok(Value::str(vm.inspect(s)?)));
    vm.def(&c, "==", |_, s, a, _| {
        Ok(Value::Bool(values_eq(s, &arg(a, 0))))
    });
    vm.def(&c, "default", |vm, s, _, _| {
        Ok(this_hash(vm, s)?.borrow().default.clone())
    });
    vm.def(&c, "default=", |vm, s, a, _| {
        this_hash(vm, s)?.borrow_mut().default = arg(a, 0);
        Ok(arg(a, 0))
    });
    let each: NativeFn = |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (k, v) in pairs(vm, s)? {
            vm.call_block(&b, vec![pair(&k, &v)])?;
        }
        Ok(s.clone())
    };
    vm.def(&c, "each", each);
    vm.def(&c, "each_pair", each);
    vm.def(&c, "each_key", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (k, _) in pairs(vm, s)? {
            vm.call_block(&b, vec![k])?;
        }
        Ok(s.clone())
    });
    vm.def(&c, "each_value", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (_, v) in pairs(vm, s)? {
            vm.call_block(&b, vec![v])?;
        }
        Ok(s.clone())
    });
    vm.def(&c, "delete", |vm, s, a, _| {
        Ok(this_hash(vm, s)?
            .borrow_mut()
            .remove(&arg(a, 0))
            .unwrap_or(Value::Nil))
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        this_hash(vm, s)?.borrow_mut().pairs.clear();
        Ok(s.clone())
    });
    vm.def(&c, "merge", |vm, s, a, b| {
        let mut h = this_hash(vm, s)?.borrow().clone();
        merge_into(vm, &mut h, a, b)?;
        Ok(Value::hash(h))
    });
    let update: NativeFn = |vm, s, a, b| {
        let mut h = this_hash(vm, s)?.borrow().clone();
        merge_into(vm, &mut h, a, b)?;
        *this_hash(vm, s)?.borrow_mut() = h;
        Ok(s.clone())
    };
    vm.def(&c, "merge!", update);
    vm.def(&c, "update", update);
    vm.def(&c, "select", |vm, s, _, b| filter(vm, s, b, true));
    vm.def(&c, "filter", |vm, s, _, b| filter(vm, s, b, true));
    vm.def(&c, "reject", |vm, s, _, b| filter(vm, s, b, false));
    vm.def(&c, "transform_values", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut h = RHash::new();
        for (k, v) in pairs(vm, s)? {
            h.set(k, vm.call_block(&b, vec![v])?);
        }
        Ok(Value::hash(h))
    });
    vm.def(&c, "transform_keys", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut h = RHash::new();
        for (k, v) in pairs(vm, s)? {
            h.set(vm.call_block(&b, vec![k])?, v);
        }
        Ok(Value::hash(h))
    });
    vm.def(&c, "invert", |vm, s, _, _| {
        let mut h = RHash::new();
        for (k, v) in pairs(vm, s)? {
            h.set(v, k);
        }
        Ok(Value::hash(h))
    });
    vm.def(&c, "values_at", |vm, s, a, _| {
        let h = this_hash(vm, s)?;
        let h = h.borrow();
        Ok(Value::array(
            a.iter()
                .map(|k| h.get(k).unwrap_or_else(|| h.default.clone()))
                .collect(),
        ))
    });
    vm.def(&c, "sort", |vm, s, _, b| {
        let v = to_vec(vm, s)?;
        Ok(Value::array(sort_values(vm, v, b)?))
    });

    let a = vm.c.array.clone();
    vm.def(&a, "to_h", |vm, s, _, b| {
        let mut h = RHash::new();
        for x in this_ary(vm, s)?.borrow().clone() {
            let x = match b {
                Some(b) => vm.call_block(b, vec![x])?,
                None => x,
            };
            let Value::Array(p) = &x else {
                return Err(vm.type_err("wrong element type (expected array)"));
            };
            let p = p.borrow();
            h.set(
                p.first().cloned().unwrap_or(Value::Nil),
                p.get(1).cloned().unwrap_or(Value::Nil),
            );
        }
        Ok(Value::hash(h))
    });
}

fn merge_into(
    vm: &mut Vm,
    h: &mut RHash,
    others: &[Value],
    b: Option<&Value>,
) -> Result<(), Unwind> {
    for o in others {
        for (k, v) in pairs(vm, o)? {
            let v = match (b, h.get(&k)) {
                (Some(b), Some(old)) => vm.call_block(b, vec![k.clone(), old, v])?,
                _ => v,
            };
            h.set(k, v);
        }
    }
    Ok(())
}

fn filter(vm: &mut Vm, s: &Value, b: Option<&Value>, keep: bool) -> VmResult {
    let b = need_block(vm, b)?;
    let mut h = RHash::new();
    for (k, v) in pairs(vm, s)? {
        if vm.call_block(&b, vec![k.clone(), v.clone()])?.truthy() == keep {
            h.set(k, v);
        }
    }
    Ok(Value::hash(h))
}
