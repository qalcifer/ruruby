//! Array. mruby: `src/array.c`, `mrblib/array.rb`, `mruby-array-ext`.
//! Iteration methods shared with Hash/Range come from Enumerable (`enumerable.rs`).
//!
//! Rule of thumb used throughout: never hold a `RefCell` borrow across a call
//! back into the VM (a block may mutate the same array).

use super::*;
use crate::core::string::norm_index;
use crate::vm::VmResult;

fn items(vm: &Vm, s: &Value) -> Result<Vec<Value>, Unwind> {
    Ok(vm.heap[this_ary(vm, s)?].borrow().clone())
}

pub fn init(vm: &mut Vm) {
    let c = vm.c.array.clone();
    c.includes.borrow_mut().push(vm.c.enumerable.clone());
    vm.def(&c, "each", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let arr = this_ary(vm, s)?;
        let mut i = 0;
        loop {
            let v = match vm.heap[arr].borrow().get(i) {
                Some(v) => v.clone(),
                None => break,
            };
            vm.call_block(&b, vec![v])?;
            i += 1;
        }
        Ok(s.clone())
    });
    vm.def(&c, "each_index", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let n = vm.heap[this_ary(vm, s)?].borrow().len();
        for i in 0..n {
            vm.call_block(&b, vec![Value::Int(i as i64)])?;
        }
        Ok(s.clone())
    });
    vm.def(&c, "length", |vm, s, _, _| {
        Ok(Value::Int(vm.heap[this_ary(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::Int(vm.heap[this_ary(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::Bool(vm.heap[this_ary(vm, s)?].borrow().is_empty()))
    });
    vm.def(&c, "to_a", |_, s, _, _| Ok(s.clone()));
    vm.def(&c, "to_s", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "==", |vm, s, a, _| {
        let Value::Array(o) = arg(a, 0) else {
            return Ok(Value::Bool(false));
        };
        let (x, y) = (items(vm, s)?, vm.heap[o].borrow().clone());
        if x.len() != y.len() {
            return Ok(Value::Bool(false));
        }
        for (p, q) in x.iter().zip(&y) {
            if !vm.equal(p, q)? {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    });
    vm.def(&c, "<=>", |vm, s, a, _| match arg(a, 0) {
        o @ Value::Array(_) => Ok(Value::Int(compare(vm, s, &o)? as i64)),
        _ => Ok(Value::Nil),
    });
    vm.def(&c, "[]", aref);
    vm.def(&c, "slice", aref);
    vm.def(&c, "at", aref);
    vm.def(&c, "[]=", aset);
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
    vm.def(&c, "fetch", |vm, s, a, _| {
        let v = items(vm, s)?;
        let i = int_of(vm, &arg(a, 0))?;
        match norm_index(i, v.len()).filter(|&i| i < v.len()) {
            Some(i) => Ok(v[i].clone()),
            None if a.len() > 1 => Ok(a[1].clone()),
            None => Err(vm.err(
                &vm.c.index_error,
                format!("index {} outside of array bounds", i),
            )),
        }
    });
    vm.def(&c, "first", |vm, s, a, _| {
        let v = items(vm, s)?;
        if a.is_empty() {
            return Ok(v.first().cloned().unwrap_or(Value::Nil));
        }
        let n = int_of(vm, &a[0])?.max(0) as usize;
        Ok(vm.array(v.into_iter().take(n).collect()))
    });
    vm.def(&c, "last", |vm, s, a, _| {
        let v = items(vm, s)?;
        if a.is_empty() {
            return Ok(v.last().cloned().unwrap_or(Value::Nil));
        }
        let n = (int_of(vm, &a[0])?.max(0) as usize).min(v.len());
        Ok(vm.array(v[v.len() - n..].to_vec()))
    });
    let push: NativeFn = |vm, s, a, _| {
        vm.heap[this_ary(vm, s)?].borrow_mut().extend_from_slice(a);
        Ok(s.clone())
    };
    vm.def(&c, "push", push);
    vm.def(&c, "append", push);
    vm.def(&c, "<<", push);
    vm.def(&c, "pop", |vm, s, _, _| {
        Ok(vm.heap[this_ary(vm, s)?].borrow_mut().pop().unwrap_or(Value::Nil))
    });
    vm.def(&c, "shift", |vm, s, _, _| {
        let arr = this_ary(vm, s)?;
        let mut v = vm.heap[arr].borrow_mut();
        Ok(if v.is_empty() {
            Value::Nil
        } else {
            v.remove(0)
        })
    });
    let unshift: NativeFn = |vm, s, a, _| {
        let arr = this_ary(vm, s)?;
        vm.heap[arr].borrow_mut().splice(0..0, a.iter().cloned());
        Ok(s.clone())
    };
    vm.def(&c, "unshift", unshift);
    vm.def(&c, "prepend", unshift);
    vm.def(&c, "insert", |vm, s, a, _| {
        let arr = this_ary(vm, s)?;
        let len = vm.heap[arr].borrow().len();
        let i = int_of(vm, &arg(a, 0))?;
        let i = if i < 0 { i + len as i64 + 1 } else { i }.clamp(0, len as i64) as usize;
        vm.heap[arr].borrow_mut().splice(i..i, a[1..].iter().cloned());
        Ok(s.clone())
    });
    vm.def(&c, "concat", |vm, s, a, _| {
        let mut extra = Vec::new();
        for x in a {
            extra.extend(vm.heap[this_ary(vm, x)?].borrow().iter().cloned());
        }
        vm.heap[this_ary(vm, s)?].borrow_mut().extend(extra);
        Ok(s.clone())
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        vm.heap[this_ary(vm, s)?].borrow_mut().clear();
        Ok(s.clone())
    });
    vm.def(&c, "replace", |vm, s, a, _| {
        let new = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        *vm.heap[this_ary(vm, s)?].borrow_mut() = new;
        Ok(s.clone())
    });
    vm.def(&c, "delete", |vm, s, a, _| {
        let target = arg(a, 0);
        let mut found = Value::Nil;
        let mut kept = Vec::new();
        for x in items(vm, s)? {
            if vm.equal(&x, &target)? {
                found = x;
            } else {
                kept.push(x);
            }
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = kept;
        Ok(found)
    });
    vm.def(&c, "delete_at", |vm, s, a, _| {
        let arr = this_ary(vm, s)?;
        let len = vm.heap[arr].borrow().len();
        Ok(
            match norm_index(int_of(vm, &arg(a, 0))?, len).filter(|&i| i < len) {
                Some(i) => vm.heap[arr].borrow_mut().remove(i),
                None => Value::Nil,
            },
        )
    });
    vm.def(&c, "delete_if", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut kept = Vec::new();
        for x in items(vm, s)? {
            if !vm.call_block(&b, vec![x.clone()])?.truthy() {
                kept.push(x);
            }
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = kept;
        Ok(s.clone())
    });
    vm.def(&c, "map!", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in items(vm, s)? {
            out.push(vm.call_block(&b, vec![x])?);
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = out;
        Ok(s.clone())
    });
    vm.def(&c, "sort!", |vm, s, _, b| {
        let sorted = sort_values(vm, items(vm, s)?, b)?;
        *vm.heap[this_ary(vm, s)?].borrow_mut() = sorted;
        Ok(s.clone())
    });
    vm.def(&c, "reverse!", |vm, s, _, _| {
        vm.heap[this_ary(vm, s)?].borrow_mut().reverse();
        Ok(s.clone())
    });
    vm.def(&c, "reverse", |vm, s, _, _| {
        let mut v = items(vm, s)?;
        v.reverse();
        Ok(vm.array(v))
    });
    vm.def(&c, "rotate", |vm, s, a, _| {
        let mut v = items(vm, s)?;
        if !v.is_empty() {
            let n = if a.is_empty() { 1 } else { int_of(vm, &a[0])? };
            let k = n.rem_euclid(v.len() as i64) as usize;
            v.rotate_left(k);
        }
        Ok(vm.array(v))
    });
    vm.def(&c, "join", |vm, s, a, _| {
        let sep = match arg(a, 0) {
            Value::Nil => String::new(),
            v => str_of(vm, &v)?,
        };
        Ok({ let v = join(vm, s, &sep)?; vm.str(v) })
    });
    vm.def(&c, "+", |vm, s, a, _| {
        let mut v = items(vm, s)?;
        v.extend(vm.heap[this_ary(vm, &arg(a, 0))?].borrow().iter().cloned());
        Ok(vm.array(v))
    });
    vm.def(&c, "-", |vm, s, a, _| {
        let other = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        Ok(vm.array(
            items(vm, s)?
                .into_iter()
                .filter(|x| !other.iter().any(|y| eql(&vm.heap, x, y)))
                .collect(),
        ))
    });
    vm.def(&c, "*", |vm, s, a, _| match arg(a, 0) {
        Value::Str(sep) => {
            let sep = vm.heap[sep].borrow().clone();
            Ok({ let v = join(vm, s, &sep)?; vm.str(v) })
        }
        n => {
            let n = int_of(vm, &n)?;
            if n < 0 {
                return Err(vm.arg_err("negative argument"));
            }
            let v = items(vm, s)?;
            Ok(vm.array(
                (0..n).flat_map(|_| v.iter().cloned()).collect(),
            ))
        }
    });
    vm.def(&c, "&", |vm, s, a, _| {
        let other = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        let v: Vec<Value> = items(vm, s)?
            .into_iter()
            .filter(|x| other.iter().any(|y| eql(&vm.heap, x, y)))
            .collect();
        Ok(vm.array(uniq(vm, v)))
    });
    vm.def(&c, "|", |vm, s, a, _| {
        let mut v = items(vm, s)?;
        v.extend(vm.heap[this_ary(vm, &arg(a, 0))?].borrow().iter().cloned());
        Ok(vm.array(uniq(vm, v)))
    });
    vm.def(&c, "compact", |vm, s, _, _| {
        Ok(vm.array(
            items(vm, s)?.into_iter().filter(|x| !x.is_nil()).collect(),
        ))
    });
    vm.def(&c, "flatten", |vm, s, a, _| {
        let depth = if a.is_empty() { -1 } else { int_of(vm, &a[0])? };
        let mut out = Vec::new();
        let mut seen = vec![this_ary(vm, s)?.id()];
        if !flatten(vm, &items(vm, s)?, depth, &mut out, &mut seen) {
            return Err(vm.arg_err("tried to flatten recursive array"));
        }
        Ok(vm.array(out))
    });
    vm.def(&c, "index", |vm, s, a, b| {
        for (i, x) in items(vm, s)?.into_iter().enumerate() {
            let hit = match b {
                Some(b) if a.is_empty() => vm.call_block(b, vec![x])?.truthy(),
                _ => vm.equal(&x, &arg(a, 0))?,
            };
            if hit {
                return Ok(Value::Int(i as i64));
            }
        }
        Ok(Value::Nil)
    });
    vm.def(&c, "count", |vm, s, a, b| {
        let v = items(vm, s)?;
        if a.is_empty() && b.is_none() {
            return Ok(Value::Int(v.len() as i64));
        }
        let mut n = 0;
        for x in v {
            let hit = match b {
                Some(b) => vm.call_block(b, vec![x])?.truthy(),
                None => vm.equal(&x, &a[0])?,
            };
            n += hit as i64;
        }
        Ok(Value::Int(n))
    });
    vm.def(&c, "fill", |vm, s, a, _| {
        let v = arg(a, 0);
        vm.heap[this_ary(vm, s)?].borrow_mut()
            .iter_mut()
            .for_each(|x| *x = v.clone());
        Ok(s.clone())
    });
    vm.def(&c, "product", |vm, s, a, _| {
        let other = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        let mut out = Vec::new();
        for x in items(vm, s)? {
            for y in &other {
                out.push(vm.array(vec![x.clone(), y.clone()]));
            }
        }
        Ok(vm.array(out))
    });
    vm.def(&c, "transpose", |vm, s, _, _| {
        let rows: Vec<Vec<Value>> = items(vm, s)?
            .iter()
            .map(|r| this_ary(vm, r).map(|a| vm.heap[a].borrow().clone()))
            .collect::<Result<_, _>>()?;
        let n = rows.first().map_or(0, |r| r.len());
        if rows.iter().any(|r| r.len() != n) {
            return Err(vm.err(&vm.c.index_error, "element size differs"));
        }
        let cols: Vec<Value> = (0..n)
            .map(|i| vm.array(rows.iter().map(|r| r[i].clone()).collect()))
            .collect();
        Ok(vm.array(cols))
    });
    vm.def(&c, "assoc", |vm, s, a, _| {
        for x in items(vm, s)? {
            if let Value::Array(pair) = &x
                && vm.heap[pair]
                    .borrow()
                    .first()
                    .is_some_and(|k| values_eq(&vm.heap, k, &arg(a, 0)))
            {
                return Ok(x);
            }
        }
        Ok(Value::Nil)
    });
    vm.def(&c, "hash", |vm, s, _, _| {
        Ok(Value::Int(items(vm, s)?.len() as i64))
    });
}

/// `a[i] = v`, `a[start, len] = v`, `a[range] = v` (the last two splice).
fn aset(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    check_argc(vm, a, 2, 3)?;
    let arr = this_ary(vm, s)?;
    let len = vm.heap[arr].borrow().len() as i64;
    let value = a[a.len() - 1].clone();
    let (start, count) = match (&a[0], a.len()) {
        (Value::Range(r), 2) => {
            let lo = if vm.heap[r].0.is_nil() { 0 } else { int_of(vm, &vm.heap[r].0)? };
            let lo = if lo < 0 { lo + len } else { lo };
            let hi = if vm.heap[r].1.is_nil() {
                len - 1
            } else {
                int_of(vm, &vm.heap[r].1)?
            };
            let hi = if hi < 0 { hi + len } else { hi };
            let hi = if vm.heap[r].2 && !vm.heap[r].1.is_nil() { hi - 1 } else { hi };
            (lo, (hi - lo + 1).max(0))
        }
        (i, 3) => {
            let n = int_of(vm, &a[1])?;
            if n < 0 {
                return Err(vm.err(&vm.c.index_error, format!("negative length ({n})")));
            }
            let i = int_of(vm, i)?;
            (if i < 0 { i + len } else { i }, n)
        }
        (i, _) => {
            let i = int_of(vm, i)?;
            let i = if i < 0 { i + len } else { i };
            if i < 0 {
                return Err(vm.err(
                    &vm.c.index_error,
                    format!("index {} too small for array", i - len),
                ));
            }
            let mut v = vm.heap[arr].borrow_mut();
            let i = i as usize;
            if i >= v.len() {
                v.resize(i + 1, Value::Nil);
            }
            v[i] = value.clone();
            return Ok(value);
        }
    };
    if start < 0 {
        return Err(vm.err(
            &vm.c.index_error,
            format!("index {} too small for array", start - len),
        ));
    }
    let replacement = match &value {
        Value::Array(x) => vm.heap[x].borrow().clone(),
        v => vec![v.clone()],
    };
    let mut v = vm.heap[arr].borrow_mut();
    let start = start as usize;
    if start > v.len() {
        v.resize(start, Value::Nil);
    }
    let end = (start + count as usize).min(v.len());
    v.splice(start..end, replacement);
    Ok(value)
}

fn aref(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    // `a[i]` is hot (Ruby iterators index in a loop): read it in place
    if let [Value::Int(i)] = a {
        let v = vm.heap[this_ary(vm, s)?].borrow();
        return Ok(norm_index(*i, v.len()).and_then(|i| v.get(i).cloned()).unwrap_or(Value::Nil));
    }
    let v = items(vm, s)?;
    let len = v.len();
    match (arg(a, 0), a.len()) {
        (Value::Int(i), 1) => Ok(norm_index(i, len)
            .and_then(|i| v.get(i).cloned())
            .unwrap_or(Value::Nil)),
        (Value::Int(i), 2) => {
            let n = int_of(vm, &a[1])?;
            Ok(match norm_index(i, len) {
                Some(i) if n >= 0 => vm.array(v[i..(i + n as usize).min(len)].to_vec()),
                _ => Value::Nil,
            })
        }
        (Value::Range(r), _) => {
            let lo = int_of(vm, &vm.heap[r].0)?;
            let Some(start) = norm_index(lo, len) else {
                return Ok(Value::Nil);
            };
            let hi = if vm.heap[r].1.is_nil() {
                len as i64 - 1
            } else {
                int_of(vm, &vm.heap[r].1)?
            };
            let mut end = if hi < 0 { hi + len as i64 } else { hi };
            if !vm.heap[r].2 || vm.heap[r].1.is_nil() {
                end += 1;
            }
            let end = (end.max(start as i64) as usize).min(len);
            Ok(vm.array(v[start..end].to_vec()))
        }
        (Value::Float(f), 1) => Ok(norm_index(f as i64, len)
            .and_then(|i| v.get(i).cloned())
            .unwrap_or(Value::Nil)),
        (other, _) => Err(vm.type_err(format!(
            "no implicit conversion of {} into Integer",
            vm.class_name(&other)
        ))),
    }
}

fn join(vm: &mut Vm, s: &Value, sep: &str) -> Result<String, Unwind> {
    join_rec(vm, s, sep, &mut Vec::new())
}

fn join_rec(vm: &mut Vm, s: &Value, sep: &str, seen: &mut Vec<u64>) -> Result<String, Unwind> {
    let id = this_ary(vm, s)?.id();
    if seen.contains(&id) {
        return Err(vm.arg_err("recursive array join"));
    }
    seen.push(id);
    let mut parts = Vec::new();
    for x in items(vm, s)? {
        parts.push(match &x {
            Value::Array(_) => join_rec(vm, &x, sep, seen)?,
            _ => vm.to_s(&x)?,
        });
    }
    seen.pop();
    Ok(parts.join(sep))
}

pub fn uniq(vm: &Vm, v: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for x in v {
        if !out.iter().any(|y| eql(&vm.heap, &x, y)) {
            out.push(x);
        }
    }
    out
}

fn flatten(vm: &Vm, v: &[Value], depth: i64, out: &mut Vec<Value>, seen: &mut Vec<u64>) -> bool {
    for x in v {
        match x {
            Value::Array(inner) if depth != 0 => {
                let id = inner.id();
                if seen.contains(&id) {
                    return false;
                }
                seen.push(id);
                let ok = flatten(vm, &vm.heap[inner].borrow().clone(), depth - 1, out, seen);
                seen.pop();
                if !ok {
                    return false;
                }
            }
            _ => out.push(x.clone()),
        }
    }
    true
}
