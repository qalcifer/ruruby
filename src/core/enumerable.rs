//! Enumerable. mruby: `mrblib/enum.rb`, `mruby-enum-ext`.
//!
//! Each method materializes the receiver with [`to_vec`] (Array, Hash, Range,
//! or any class that defines `each` and includes Enumerable). mruby builds
//! these on `each` + blocks and supports lazy Enumerators; that's a roadmap item.

use super::*;
use crate::core::array::uniq;
use crate::vm::{Arith, VmResult};

fn yield1(vm: &mut Vm, b: &Value, x: &Value) -> VmResult {
    vm.call_block(b, vec![*x])
}

pub fn init(vm: &mut Vm) {
    let e = vm.c.enumerable;
    vm.def(&e, "to_a", |vm, s, _, _| Ok({ let v = to_vec(vm, s)?; vm.array(v) }));
    vm.def(&e, "entries", |vm, s, _, _| {
        Ok({ let v = to_vec(vm, s)?; vm.array(v) })
    });
    let map: NativeFn = |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in to_vec(vm, s)? {
            out.push(yield1(vm, &b, &x)?);
        }
        Ok(vm.array(out))
    };
    vm.def(&e, "map", map);
    vm.def(&e, "collect", map);
    vm.def(&e, "flat_map", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in to_vec(vm, s)? {
            let y = yield1(vm, &b, &x)?;
            match vm.kind(y) {
                V::Array(a) => out.extend(vm.heap[a].borrow().iter().cloned()),
                _ => out.push(y),
            }
        }
        Ok(vm.array(out))
    });
    vm.def(&e, "filter_map", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in to_vec(vm, s)? {
            let v = yield1(vm, &b, &x)?;
            if v.truthy() {
                out.push(v);
            }
        }
        Ok(vm.array(out))
    });
    vm.def(&e, "select", |vm, s, _, b| select(vm, s, b, true));
    vm.def(&e, "find_all", |vm, s, _, b| select(vm, s, b, true));
    vm.def(&e, "grep", |vm, s, a, b| grep(vm, s, a, b, true));
    vm.def(&e, "grep_v", |vm, s, a, b| grep(vm, s, a, b, false));
    vm.def(&e, "filter", |vm, s, _, b| select(vm, s, b, true));
    vm.def(&e, "reject", |vm, s, _, b| select(vm, s, b, false));
    vm.def(&e, "partition", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let (mut yes, mut no) = (Vec::new(), Vec::new());
        for x in to_vec(vm, s)? {
            if yield1(vm, &b, &x)?.truthy() {
                yes.push(x)
            } else {
                no.push(x)
            }
        }
        Ok({ let v = vec![vm.array(yes), vm.array(no)]; vm.array(v) })
    });
    let find: NativeFn = |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for x in to_vec(vm, s)? {
            if yield1(vm, &b, &x)?.truthy() {
                return Ok(x);
            }
        }
        Ok(Value::NIL)
    };
    vm.def(&e, "find", find);
    vm.def(&e, "detect", find);
    vm.def(&e, "find_index", |vm, s, a, b| {
        for (i, x) in to_vec(vm, s)?.into_iter().enumerate() {
            let hit = match b {
                Some(b) => yield1(vm, b, &x)?.truthy(),
                None => vm.equal(&x, &arg(a, 0))?,
            };
            if hit {
                return Ok(Value::small_int(i as i64));
            }
        }
        Ok(Value::NIL)
    });
    vm.def(&e, "each_with_index", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for (i, x) in to_vec(vm, s)?.into_iter().enumerate() {
            vm.call_block(&b, vec![x, Value::small_int(i as i64)])?;
        }
        Ok(*s)
    });
    vm.def(&e, "each_with_object", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let memo = arg(a, 0);
        for x in to_vec(vm, s)? {
            vm.call_block(&b, vec![x, memo])?;
        }
        Ok(memo)
    });
    vm.def(&e, "each_slice", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let n = int_of(vm, &arg(a, 0))?;
        if n <= 0 {
            return Err(vm.arg_err("invalid slice size"));
        }
        for chunk in to_vec(vm, s)?.chunks(n as usize) {
            let chunk = vm.array(chunk.to_vec());
            vm.call_block(&b, vec![chunk])?;
        }
        Ok(Value::NIL)
    });
    vm.def(&e, "each_cons", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let n = int_of(vm, &arg(a, 0))?;
        if n <= 0 {
            return Err(vm.arg_err("invalid size"));
        }
        for w in to_vec(vm, s)?.windows(n as usize) {
            let w = vm.array(w.to_vec());
            vm.call_block(&b, vec![w])?;
        }
        Ok(*s)
    });
    let inject: NativeFn = |vm, s, a, b| {
        let mut items = to_vec(vm, s)?.into_iter();
        let sym = |vm: &Vm, v: &Value| match vm.kind(*v) {
            V::Sym(s) => Some(s),
            _ => None,
        };
        let (mut acc, op) = match (a, b) {
            ([op], None) if sym(vm, op).is_some() => (items.next().unwrap_or(Value::NIL), sym(vm, op)),
            ([init, op], None) if sym(vm, op).is_some() => (*init, sym(vm, op)),
            ([init], Some(_)) => (*init, None),
            ([], Some(_)) => (items.next().unwrap_or(Value::NIL), None),
            _ => return Err(vm.arg_err("wrong arguments for inject")),
        };
        for x in items {
            acc = match (op, b) {
                (Some(op), _) => vm.call_method(&acc, op, vec![x], None)?,
                (None, Some(b)) => vm.call_block(b, vec![acc, x])?,
                _ => unreachable!(),
            };
        }
        Ok(acc)
    };
    vm.def(&e, "inject", inject);
    vm.def(&e, "reduce", inject);
    vm.def(&e, "sum", sum);
    vm.def(&e, "count", count);
    vm.def(&e, "min", |vm, s, a, b| {
        extreme(vm, s, a, b, Ordering::Less)
    });
    vm.def(&e, "max", |vm, s, a, b| {
        extreme(vm, s, a, b, Ordering::Greater)
    });
    vm.def(&e, "min_by", |vm, s, _, b| {
        extreme_by(vm, s, b, Ordering::Less)
    });
    vm.def(&e, "max_by", |vm, s, _, b| {
        extreme_by(vm, s, b, Ordering::Greater)
    });
    vm.def(&e, "minmax", |vm, s, _, _| {
        let lo = extreme(vm, s, &[], None, Ordering::Less)?;
        let hi = extreme(vm, s, &[], None, Ordering::Greater)?;
        Ok(vm.array(vec![lo, hi]))
    });
    vm.def(&e, "sort", |vm, s, _, b| {
        let v = to_vec(vm, s)?;
        Ok({ let v = sort_values(vm, v, b)?; vm.array(v) })
    });
    vm.def(&e, "sort_by", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut keyed = Vec::new();
        for x in to_vec(vm, s)? {
            let k = yield1(vm, &b, &x)?;
            keyed.push(vm.array(vec![k, x]));
        }
        let first = |vm: &Vm, p: &Value| match vm.kind(*p) {
            V::Array(p) => vm.heap[p].borrow()[0],
            _ => unreachable!(),
        };
        let sorted = merge_sort(vm, keyed, &mut |vm, x, y| {
            let (x, y) = (first(vm, x), first(vm, y));
            compare(vm, &x, &y)
        })?;
        Ok(vm.array(
            sorted
                .into_iter()
                .map(|p| {
                    if let V::Array(p) = vm.kind(p) {
                        vm.heap[p].borrow()[1]
                    } else {
                        p
                    }
                })
                .collect(),
        ))
    });
    vm.def(&e, "include?", |vm, s, a, _| {
        for x in to_vec(vm, s)? {
            if vm.equal(&x, &arg(a, 0))? {
                return Ok(Value::TRUE);
            }
        }
        Ok(Value::FALSE)
    });
    vm.def(&e, "member?", |vm, s, a, _| {
        for x in to_vec(vm, s)? {
            if vm.equal(&x, &arg(a, 0))? {
                return Ok(Value::TRUE);
            }
        }
        Ok(Value::FALSE)
    });
    vm.def(&e, "all?", |vm, s, a, b| {
        quantifier(vm, s, a, b, |hits, n| hits == n)
    });
    vm.def(&e, "any?", |vm, s, a, b| {
        quantifier(vm, s, a, b, |hits, _| hits > 0)
    });
    vm.def(&e, "none?", |vm, s, a, b| {
        quantifier(vm, s, a, b, |hits, _| hits == 0)
    });
    vm.def(&e, "one?", |vm, s, a, b| {
        quantifier(vm, s, a, b, |hits, _| hits == 1)
    });
    vm.def(&e, "take", |vm, s, a, _| {
        let n = int_of(vm, &arg(a, 0))?.max(0) as usize;
        Ok({ let v = to_vec(vm, s)?.into_iter().take(n).collect(); vm.array(v) })
    });
    vm.def(&e, "drop", |vm, s, a, _| {
        let n = int_of(vm, &arg(a, 0))?.max(0) as usize;
        Ok({ let v = to_vec(vm, s)?.into_iter().skip(n).collect(); vm.array(v) })
    });
    vm.def(&e, "take_while", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let mut out = Vec::new();
        for x in to_vec(vm, s)? {
            if !yield1(vm, &b, &x)?.truthy() {
                break;
            }
            out.push(x);
        }
        Ok(vm.array(out))
    });
    vm.def(&e, "drop_while", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let items = to_vec(vm, s)?;
        let mut start = items.len();
        for (i, x) in items.iter().enumerate() {
            if !yield1(vm, &b, x)?.truthy() {
                start = i;
                break;
            }
        }
        Ok(vm.array(items[start..].to_vec()))
    });
    vm.def(&e, "group_by", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let hv = vm.hash(RHash::new());
        let h = this_hash(vm, &hv)?;
        for x in to_vec(vm, s)? {
            let k = yield1(vm, &b, &x)?;
            let found = vm.hash_get(h, &k)?;
            match found.map(|v| vm.kind(v)) {
                Some(V::Array(v)) => vm.heap[v].borrow_mut().push(x),
                _ => {
                    let group = vm.array(vec![x]);
                    vm.hash_set(h, k, group)?;
                }
            }
        }
        Ok(hv)
    });
    vm.def(&e, "tally", |vm, s, _, _| {
        let hv = vm.hash(RHash::new());
        let h = this_hash(vm, &hv)?;
        for x in to_vec(vm, s)? {
            let n = match vm.hash_get(h, &x)?.map(|v| vm.kind(v)) {
                Some(V::Int(n)) => n + 1,
                _ => 1,
            };
            let n = vm.int(n);
            vm.hash_set(h, x, n)?;
        }
        Ok(hv)
    });
    vm.def(&e, "uniq", |vm, s, _, b| {
        let items = to_vec(vm, s)?;
        let Some(b) = b else {
            return Ok({ let u = uniq(vm, items); vm.array(u) });
        };
        let mut keys: Vec<Value> = Vec::new();
        let mut out = Vec::new();
        for x in items {
            let k = yield1(vm, b, &x)?;
            if !keys.iter().any(|y| eql(&vm.heap, &k, y)) {
                keys.push(k);
                out.push(x);
            }
        }
        Ok(vm.array(out))
    });
    vm.def(&e, "zip", |vm, s, a, _| {
        let mine = to_vec(vm, s)?;
        let others = a
            .iter()
            .map(|o| to_vec(vm, o))
            .collect::<Result<Vec<_>, _>>()?;
        let rows: Vec<Value> = mine
            .into_iter()
            .enumerate()
            .map(|(i, x)| {
                let mut row = vec![x];
                row.extend(others.iter().map(|o| o.get(i).cloned().unwrap_or(Value::NIL)));
                vm.array(row)
            })
            .collect();
        Ok(vm.array(rows))
    });
    vm.def(&e, "each_entry", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for x in to_vec(vm, s)? {
            yield1(vm, &b, &x)?;
        }
        Ok(*s)
    });
}

fn select(vm: &mut Vm, s: &Value, b: Option<&Value>, keep: bool) -> VmResult {
    let b = need_block(vm, b)?;
    let mut out = Vec::new();
    for x in to_vec(vm, s)? {
        if yield1(vm, &b, &x)?.truthy() == keep {
            out.push(x);
        }
    }
    Ok(vm.array(out))
}

pub fn count(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>) -> VmResult {
    let mut n = 0;
    for x in to_vec(vm, s)? {
        let hit = match (a, b) {
            ([target], _) => vm.equal(&x, target)?,
            (_, Some(b)) => yield1(vm, b, &x)?.truthy(),
            _ => true,
        };
        n += hit as i64;
    }
    Ok(vm.int(n))
}

pub fn sum(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>) -> VmResult {
    let mut acc = if a.is_empty() {
        Value::small_int(0)
    } else {
        a[0]
    };
    for x in to_vec(vm, s)? {
        let x = match b {
            Some(b) => yield1(vm, b, &x)?,
            None => x,
        };
        acc = if acc.is_numeric() && x.is_numeric() {
            vm.arith(Arith::Add, &acc, &x)?
        } else {
            vm.call_method(&acc, vm.s.add, vec![x], None)?
        };
    }
    Ok(acc)
}

fn extreme(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>, want: Ordering) -> VmResult {
    let items = to_vec(vm, s)?;
    if let Some(n) = a.first() {
        let n = int_of(vm, n)?.max(0) as usize;
        let mut sorted = sort_values(vm, items, b)?;
        if want == Ordering::Greater {
            sorted.reverse();
        }
        sorted.truncate(n);
        return Ok(vm.array(sorted));
    }
    let mut best: Option<Value> = None;
    for x in items {
        best = Some(match best {
            None => x,
            Some(cur) => {
                let o = match b {
                    Some(b) => match vm.call_block(b, vec![x, cur]).map(|r| vm.kind(r))? {
                        V::Int(i) => i.cmp(&0),
                        _ => return Err(vm.arg_err("comparison failed")),
                    },
                    None => compare(vm, &x, &cur)?,
                };
                if o == want { x } else { cur }
            }
        });
    }
    Ok(best.unwrap_or(Value::NIL))
}

fn extreme_by(vm: &mut Vm, s: &Value, b: Option<&Value>, want: Ordering) -> VmResult {
    let b = need_block(vm, b)?;
    let mut best: Option<(Value, Value)> = None;
    for x in to_vec(vm, s)? {
        let k = yield1(vm, &b, &x)?;
        best = Some(match best {
            Some((bk, bx)) if compare(vm, &k, &bk)? != want => (bk, bx),
            _ => (k, x),
        });
    }
    Ok(best.map(|(_, x)| x).unwrap_or(Value::NIL))
}

fn quantifier(
    vm: &mut Vm,
    s: &Value,
    a: &[Value],
    b: Option<&Value>,
    pred: fn(usize, usize) -> bool,
) -> VmResult {
    let items = to_vec(vm, s)?;
    let mut hits = 0;
    for x in &items {
        let hit = match (a, b) {
            ([pattern], _) => vm.send(pattern, "===", vec![*x])?.truthy(),
            (_, Some(b)) => yield1(vm, b, x)?.truthy(),
            _ => x.truthy(),
        };
        hits += hit as usize;
    }
    Ok(Value::bool(pred(hits, items.len())))
}

/// grep(pattern) { |x| }: the elements `pattern === x` (or not, for
/// grep_v) takes, through the block when given.
fn grep(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>, matching: bool) -> VmResult {
    check_argc(vm, a, 1, 1)?;
    let mut out = Vec::new();
    for x in to_vec(vm, s)? {
        if vm.send(&a[0], "===", vec![x])?.truthy() == matching {
            out.push(match b {
                Some(b) => vm.call_block(b, vec![x])?,
                None => x,
            });
        }
    }
    Ok(vm.array(out))
}

