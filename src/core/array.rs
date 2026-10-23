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
    let c = vm.c.array;
    vm.heap[c].includes.push(vm.c.enumerable);
    // what Array.new and a subclass's `super` in initialize run
    vm.def(&c, "initialize", |vm, s, a, b| {
        vm.check_frozen(s)?;
        let items = super::kernel::array_new_items(vm, a, b)?;
        *vm.heap[this_ary(vm, s)?].borrow_mut() = items;
        Ok(*s)
    });
    // Array[1, 2]: an instance of the receiver (a subclass too)
    vm.def_singleton(&c, "[]", |vm, s, a, _| {
        let v = vm.array(a.to_vec());
        if let V::Class(cls) = vm.kind(*s) {
            vm.set_builtin_class(&v, cls);
        }
        Ok(v)
    });
    vm.def(&c, "each", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let arr = this_ary(vm, s)?;
        let mut i = 0;
        loop {
            let v = match vm.heap[arr].borrow().get(i) {
                Some(v) => *v,
                None => break,
            };
            vm.call_block(&b, vec![v])?;
            i += 1;
        }
        Ok(*s)
    });
    vm.def(&c, "each_index", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let n = vm.heap[this_ary(vm, s)?].borrow().len();
        for i in 0..n {
            vm.call_block(&b, vec![Value::small_int(i as i64)])?;
        }
        Ok(*s)
    });
    vm.def(&c, "length", |vm, s, _, _| {
        Ok(Value::small_int(vm.heap[this_ary(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::small_int(vm.heap[this_ary(vm, s)?].borrow().len() as i64))
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::bool(vm.heap[this_ary(vm, s)?].borrow().is_empty()))
    });
    vm.def(&c, "to_a", |_, s, _, _| Ok(*s));
    vm.def(&c, "to_s", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "==", |vm, s, a, _| {
        let V::Array(o) = vm.kind(arg(a, 0)) else {
            return Ok(Value::FALSE);
        };
        let (x, y) = (items(vm, s)?, vm.heap[o].borrow().clone());
        if x.len() != y.len() {
            return Ok(Value::FALSE);
        }
        for (p, q) in x.iter().zip(&y) {
            if !vm.equal(p, q)? {
                return Ok(Value::FALSE);
            }
        }
        Ok(Value::TRUE)
    });
    vm.def(&c, "<=>", |vm, s, a, _| match vm.kind(arg(a, 0)) {
        V::Array(_) => Ok(Value::small_int(compare(vm, s, &a[0])? as i64)),
        _ => Ok(Value::NIL),
    });
    vm.def(&c, "[]", aref);
    vm.def(&c, "slice", aref);
    vm.def(&c, "at", aref);
    vm.def(&c, "[]=", aset);
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
    vm.def(&c, "fetch", |vm, s, a, _| {
        let v = items(vm, s)?;
        let i = int_of(vm, &arg(a, 0))?;
        match norm_index(i, v.len()).filter(|&i| i < v.len()) {
            Some(i) => Ok(v[i]),
            None if a.len() > 1 => Ok(a[1]),
            None => Err(vm.err(
                &vm.c.index_error,
                format!("index {} outside of array bounds", i),
            )),
        }
    });
    vm.def(&c, "first", |vm, s, a, _| {
        check_argc(vm, a, 0, 1)?;
        let v = items(vm, s)?;
        if a.is_empty() {
            return Ok(v.first().cloned().unwrap_or(Value::NIL));
        }
        let n = count_arg(vm, &a[0])?;
        Ok(vm.array(v.into_iter().take(n).collect()))
    });
    vm.def(&c, "last", |vm, s, a, _| {
        check_argc(vm, a, 0, 1)?;
        let v = items(vm, s)?;
        if a.is_empty() {
            return Ok(v.last().cloned().unwrap_or(Value::NIL));
        }
        let n = count_arg(vm, &a[0])?.min(v.len());
        Ok(vm.array(v[v.len() - n..].to_vec()))
    });
    let push: NativeFn = |vm, s, a, _| {
        vm.check_frozen(s)?;
        vm.heap[this_ary(vm, s)?].borrow_mut().extend_from_slice(a);
        Ok(*s)
    };
    vm.def(&c, "push", push);
    vm.def(&c, "append", push);
    vm.def(&c, "<<", push);
    // pop / pop(n), shift / shift(n)
    vm.def(&c, "pop", |vm, s, a, _| {
        vm.check_frozen(s)?;
        check_argc(vm, a, 0, 1)?;
        let arr = this_ary(vm, s)?;
        if let Some(n) = a.first() {
            let n = count_arg(vm, n)?;
            let mut v = vm.heap[arr].borrow_mut();
            let at = v.len().saturating_sub(n);
            let taken = v.split_off(at);
            drop(v);
            return Ok(vm.array(taken));
        }
        Ok(vm.heap[arr].borrow_mut().pop().unwrap_or(Value::NIL))
    });
    vm.def(&c, "shift", |vm, s, a, _| {
        vm.check_frozen(s)?;
        check_argc(vm, a, 0, 1)?;
        let arr = this_ary(vm, s)?;
        if let Some(n) = a.first() {
            let n = count_arg(vm, n)?;
            let mut v = vm.heap[arr].borrow_mut();
            let n = n.min(v.len());
            let taken: Vec<Value> = v.drain(..n).collect();
            drop(v);
            return Ok(vm.array(taken));
        }
        let mut v = vm.heap[arr].borrow_mut();
        Ok(if v.is_empty() {
            Value::NIL
        } else {
            v.remove(0)
        })
    });
    let unshift: NativeFn = |vm, s, a, _| {
        vm.check_frozen(s)?;
        let arr = this_ary(vm, s)?;
        vm.heap[arr].borrow_mut().splice(0..0, a.iter().cloned());
        Ok(*s)
    };
    vm.def(&c, "unshift", unshift);
    vm.def(&c, "prepend", unshift);
    vm.def(&c, "insert", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let arr = this_ary(vm, s)?;
        let len = vm.heap[arr].borrow().len();
        let i = int_of(vm, &arg(a, 0))?;
        let i = if i < 0 { i + len as i64 + 1 } else { i }.clamp(0, len as i64) as usize;
        vm.heap[arr].borrow_mut().splice(i..i, a[1..].iter().cloned());
        Ok(*s)
    });
    vm.def(&c, "concat", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let mut extra = Vec::new();
        for x in a {
            extra.extend(vm.heap[this_ary(vm, x)?].borrow().iter().cloned());
        }
        vm.heap[this_ary(vm, s)?].borrow_mut().extend(extra);
        Ok(*s)
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        vm.check_frozen(s)?;
        vm.heap[this_ary(vm, s)?].borrow_mut().clear();
        Ok(*s)
    });
    vm.def(&c, "replace", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let new = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        *vm.heap[this_ary(vm, s)?].borrow_mut() = new;
        Ok(*s)
    });
    // delete(x) { |x| }: the block's value when nothing equals x
    vm.def(&c, "delete", |vm, s, a, b| {
        vm.check_frozen(s)?;
        let target = arg(a, 0);
        let mut found = None;
        let mut kept = Vec::new();
        for x in items(vm, s)? {
            if vm.equal(&x, &target)? {
                found = Some(x);
            } else {
                kept.push(x);
            }
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = kept;
        match (found, b) {
            (Some(x), _) => Ok(x),
            (None, Some(b)) => vm.call_block(b, vec![target]),
            (None, None) => Ok(Value::NIL),
        }
    });
    vm.def(&c, "delete_at", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let arr = this_ary(vm, s)?;
        let len = vm.heap[arr].borrow().len();
        Ok(
            match norm_index(int_of(vm, &arg(a, 0))?, len).filter(|&i| i < len) {
                Some(i) => vm.heap[arr].borrow_mut().remove(i),
                None => Value::NIL,
            },
        )
    });
    vm.def(&c, "delete_if", |vm, s, _, b| {
        vm.check_frozen(s)?;
        let b = need_block(vm, b)?;
        let mut kept = Vec::new();
        for x in items(vm, s)? {
            if !vm.call_block(&b, vec![x])?.truthy() {
                kept.push(x);
            }
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = kept;
        Ok(*s)
    });
    vm.def(&c, "map!", |vm, s, _, b| {
        vm.check_frozen(s)?;
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in items(vm, s)? {
            out.push(vm.call_block(&b, vec![x])?);
        }
        *vm.heap[this_ary(vm, s)?].borrow_mut() = out;
        Ok(*s)
    });
    let map_bang = vm.intern("map!");
    let collect_bang = vm.intern("collect!");
    let _ = vm.alias_method(&c, collect_bang, map_bang);
    vm.def(&c, "sort!", |vm, s, _, b| {
        vm.check_frozen(s)?;
        let sorted = sort_values(vm, items(vm, s)?, b)?;
        *vm.heap[this_ary(vm, s)?].borrow_mut() = sorted;
        Ok(*s)
    });
    vm.def(&c, "reverse!", |vm, s, _, _| {
        vm.check_frozen(s)?;
        vm.heap[this_ary(vm, s)?].borrow_mut().reverse();
        Ok(*s)
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
        let sep = match vm.kind(arg(a, 0)) {
            V::Nil => String::new(),
            _ => str_of(vm, &arg(a, 0))?,
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
    vm.def(&c, "*", |vm, s, a, _| match vm.kind(arg(a, 0)) {
        V::Str(sep) => {
            let sep = vm.heap[sep].borrow().clone();
            Ok({ let v = join(vm, s, &sep)?; vm.str(v) })
        }
        _ => {
            let n = int_of(vm, &arg(a, 0))?;
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
                return Ok(Value::small_int(i as i64));
            }
        }
        Ok(Value::NIL)
    });
    vm.def(&c, "count", |vm, s, a, b| {
        let v = items(vm, s)?;
        if a.is_empty() && b.is_none() {
            return Ok(Value::small_int(v.len() as i64));
        }
        let mut n = 0;
        for x in v {
            let hit = match b {
                Some(b) => vm.call_block(b, vec![x])?.truthy(),
                None => vm.equal(&x, &a[0])?,
            };
            n += hit as i64;
        }
        Ok(vm.int(n))
    });
    vm.def(&c, "fill", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let v = arg(a, 0);
        vm.heap[this_ary(vm, s)?].borrow_mut()
            .iter_mut()
            .for_each(|x| *x = v);
        Ok(*s)
    });
    vm.def(&c, "product", |vm, s, a, _| {
        let other = vm.heap[this_ary(vm, &arg(a, 0))?].borrow().clone();
        let mut out = Vec::new();
        for x in items(vm, s)? {
            for y in &other {
                out.push(vm.array(vec![x, *y]));
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
            .map(|i| vm.array(rows.iter().map(|r| r[i]).collect()))
            .collect();
        Ok(vm.array(cols))
    });
    vm.def(&c, "assoc", |vm, s, a, _| {
        for x in items(vm, s)? {
            if let V::Array(pair) = vm.kind(x)
                && vm.heap[pair]
                    .borrow()
                    .first()
                    .is_some_and(|k| values_eq(&vm.heap, k, &arg(a, 0)))
            {
                return Ok(x);
            }
        }
        Ok(Value::NIL)
    });
}

/// `a[i] = v`, `a[start, len] = v`, `a[range] = v` (the last two splice).
fn aset(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    vm.check_frozen(s)?;
    check_argc(vm, a, 2, 3)?;
    let arr = this_ary(vm, s)?;
    let len = vm.heap[arr].borrow().len() as i64;
    let value = a[a.len() - 1];
    let (start, count) = match (vm.kind(a[0]), a.len()) {
        (V::Range(r), 2) => {
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
        (_, 3) => {
            let n = int_of(vm, &a[1])?;
            if n < 0 {
                return Err(vm.err(&vm.c.index_error, format!("negative length ({n})")));
            }
            let i = int_of(vm, &a[0])?;
            (if i < 0 { i + len } else { i }, n)
        }
        (_, _) => {
            let i = int_of(vm, &a[0])?;
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
                v.resize(i + 1, Value::NIL);
            }
            v[i] = value;
            return Ok(value);
        }
    };
    if start < 0 {
        return Err(vm.err(
            &vm.c.index_error,
            format!("index {} too small for array", start - len),
        ));
    }
    let replacement = match vm.kind(value) {
        V::Array(x) => vm.heap[x].borrow().clone(),
        _ => vec![value],
    };
    let mut v = vm.heap[arr].borrow_mut();
    let start = start as usize;
    if start > v.len() {
        v.resize(start, Value::NIL);
    }
    let end = (start + count as usize).min(v.len());
    v.splice(start..end, replacement);
    Ok(value)
}

fn aref(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    if !(1..=2).contains(&a.len()) {
        check_argc(vm, a, 1, 2)?;
    }
    // `a[i]` is hot (Ruby iterators index in a loop): read it in place
    if let [x] = a
        && let V::Int(i) = vm.kind(*x)
    {
        let v = vm.heap[this_ary(vm, s)?].borrow();
        return Ok(norm_index(i, v.len()).and_then(|i| v.get(i).cloned()).unwrap_or(Value::NIL));
    }
    let v = items(vm, s)?;
    let len = v.len();
    match (vm.kind(arg(a, 0)), a.len()) {
        (V::Int(i), 1) => Ok(norm_index(i, len)
            .and_then(|i| v.get(i).cloned())
            .unwrap_or(Value::NIL)),
        (V::Int(i), 2) => {
            let n = int_of(vm, &a[1])?;
            Ok(match norm_index(i, len) {
                Some(i) if n >= 0 => vm.array(v[i..(i + n as usize).min(len)].to_vec()),
                _ => Value::NIL,
            })
        }
        (V::Range(r), _) => {
            let lo = int_of(vm, &vm.heap[r].0)?;
            let Some(start) = norm_index(lo, len) else {
                return Ok(Value::NIL);
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
        (V::Float(_), 1) => Ok(norm_index(int_of(vm, &a[0])?, len)
            .and_then(|i| v.get(i).cloned())
            .unwrap_or(Value::NIL)),
        _ => Err(vm.type_err(format!(
            "no implicit conversion of {} into Integer",
            vm.class_name(&arg(a, 0))
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
        parts.push(match vm.kind(x) {
            V::Array(_) => join_rec(vm, &x, sep, seen)?,
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
        match vm.kind(*x) {
            V::Array(inner) if depth != 0 => {
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
            _ => out.push(*x),
        }
    }
    true
}

/// A count argument (first(n), pop(n)…): ArgumentError when negative.
fn count_arg(vm: &Vm, v: &Value) -> Result<usize, Unwind> {
    let n = int_of(vm, v)?;
    if n < 0 {
        return Err(vm.arg_err("negative array size"));
    }
    Ok(n as usize)
}

pub fn init_more(vm: &mut Vm) {
    let c = vm.c.array;
    // rindex(x) / rindex { |x| }: the last position
    vm.def(&c, "rindex", |vm, s, a, b| {
        let v = items(vm, s)?;
        for i in (0..v.len()).rev() {
            let hit = match (a.first(), b) {
                (Some(x), _) => vm.equal(&v[i], x)?,
                (None, Some(b)) => vm.call_block(b, vec![v[i]])?.truthy(),
                (None, None) => return Err(vm.arg_err("wrong number of arguments (given 0, expected 1)")),
            };
            if hit {
                return Ok(Value::small_int(i as i64));
            }
        }
        Ok(Value::NIL)
    });
    vm.def(&c, "initialize_copy", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let src = this_ary(vm, &arg(a, 0))?;
        let v = vm.heap[src].borrow().clone();
        *vm.heap[this_ary(vm, s)?].borrow_mut() = v;
        Ok(*s)
    });
}

