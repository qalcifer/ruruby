//! Range. mruby: `src/range.c`, `mrblib/range.rb`, `mruby-range-ext`.

use super::*;

pub fn range_vec(vm: &mut Vm, r: &(Value, Value, bool)) -> Result<Vec<Value>, Unwind> {
    match (vm.kind(r.0), vm.kind(r.1)) {
        (V::Int(lo), V::Int(hi)) => {
            let hi = if r.2 { hi - 1 } else { hi };
            Ok((lo..=hi).map(|i| vm.int(i)).collect())
        }
        (V::Str(lo), V::Str(hi)) => {
            let (lo, hi) = (vm.heap[lo].borrow().clone(), vm.heap[hi].borrow().clone());
            let mut out = Vec::new();
            let mut cur = vm.str(lo);
            loop {
                let s = str_of(vm, &cur)?;
                if s.chars().count() > hi.chars().count() {
                    break;
                }
                if s == hi {
                    if !r.2 {
                        out.push(cur);
                    }
                    break;
                }
                let next = vm.send(&cur, "succ", vec![])?;
                out.push(cur);
                cur = next;
            }
            Ok(out)
        }
        _ => Err(vm.type_err(format!("can't iterate from {}", vm.class_name(&r.0)))),
    }
}

/// The receiver's (begin, end, exclusive?).
fn this(vm: &Vm, s: &Value) -> Result<(Value, Value, bool), Unwind> {
    match vm.kind(*s) {
        V::Range(r) => Ok(vm.heap[r]),
        _ => Err(vm.type_err("expected a Range")),
    }
}

pub fn init(vm: &mut Vm) {
    let c = vm.c.range;
    vm.heap[c].includes.push(vm.c.enumerable);
    vm.def(&c, "first", |vm, s, a, _| {
        let r = this(vm, s)?;
        if a.is_empty() {
            return Ok(r.0);
        }
        let n = int_of(vm, &a[0])?.max(0) as usize;
        let v = range_vec(vm, &r)?;
        Ok(vm.array(v.into_iter().take(n).collect()))
    });
    vm.def(&c, "begin", |vm, s, _, _| Ok(this(vm, s)?.0));
    vm.def(&c, "last", |vm, s, a, _| {
        let r = this(vm, s)?;
        if a.is_empty() {
            return Ok(r.1);
        }
        let n = int_of(vm, &a[0])?.max(0) as usize;
        let v = range_vec(vm, &r)?;
        Ok(vm.array(v[v.len().saturating_sub(n)..].to_vec()))
    });
    vm.def(&c, "end", |vm, s, _, _| Ok(this(vm, s)?.1));
    vm.def(&c, "exclude_end?", |vm, s, _, _| {
        Ok(Value::bool(this(vm, s)?.2))
    });
    vm.def(&c, "each", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let r = this(vm, s)?;
        if let (V::Int(lo), V::Int(hi)) = (vm.kind(r.0), vm.kind(r.1)) {
            // no materialization for the common integer case
            let hi = if r.2 { hi - 1 } else { hi };
            for i in lo..=hi {
                let v = vm.int(i);
                vm.call_block(&b, vec![v])?;
            }
        } else {
            for x in range_vec(vm, &r)? {
                vm.call_block(&b, vec![x])?;
            }
        }
        Ok(*s)
    });
    vm.def(&c, "reverse_each", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let r = this(vm, s)?;
        for x in range_vec(vm, &r)?.into_iter().rev() {
            vm.call_block(&b, vec![x])?;
        }
        Ok(*s)
    });
    vm.def(&c, "step", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let r = this(vm, s)?;
        let step = int_of(vm, &arg(a, 0))?;
        if step <= 0 {
            return Err(vm.arg_err("step can't be negative or zero"));
        }
        let (lo, hi) = (int_of(vm, &r.0)?, int_of(vm, &r.1)?);
        let mut i = lo;
        while i < hi || (!r.2 && i == hi) {
            let v = vm.int(i);
                vm.call_block(&b, vec![v])?;
            i += step;
        }
        Ok(*s)
    });
    vm.def(&c, "size", size);
    vm.def(&c, "count", |vm, s, a, b| {
        if a.is_empty() && b.is_none() {
            size(vm, s, a, b)
        } else {
            enumerable::count(vm, s, a, b)
        }
    });
    let cover: NativeFn = |vm, s, a, _| {
        let r = this(vm, s)?;
        let x = arg(a, 0);
        let above = compare(vm, &r.0, &x).map(|o| o.is_le()).unwrap_or(false);
        let below = r.1.is_nil()
            || compare(vm, &x, &r.1)
                .map(|o| if r.2 { o.is_lt() } else { o.is_le() })
                .unwrap_or(false);
        Ok(Value::bool(above && below))
    };
    for name in ["include?", "member?", "cover?", "==="] {
        vm.def(&c, name, cover);
    }
    vm.def(&c, "to_s", |vm, s, _, _| {
        let r = this(vm, s)?;
        let (lo, hi) = (vm.to_s(&r.0)?, vm.to_s(&r.1)?);
        Ok(vm.str(format!("{lo}{}{hi}", if r.2 { "..." } else { ".." })))
    });
    vm.def(&c, "inspect", |vm, s, _, _| Ok({ let v = vm.inspect(s)?; vm.str(v) }));
    vm.def(&c, "sum", |vm, s, a, b| {
        let r = this(vm, s)?;
        match (vm.kind(r.0), vm.kind(r.1), b) {
            (V::Int(lo), V::Int(hi), None) if a.is_empty() => {
                let hi = if r.2 { hi - 1 } else { hi };
                if hi < lo {
                    return Ok(Value::small_int(0));
                }
                let n = (hi - lo + 1) as i128;
                Ok(vm.int(((lo as i128 + hi as i128) * n / 2) as i64))
            }
            _ => enumerable::sum(vm, s, a, b),
        }
    });
}

fn size(vm: &mut Vm, s: &Value, _: &[Value], _: Option<&Value>) -> crate::vm::VmResult {
    let r = this(vm, s)?;
    match (vm.kind(r.0), vm.kind(r.1)) {
        (V::Int(lo), V::Int(hi)) => {
            let hi = if r.2 { hi - 1 } else { hi };
            Ok(vm.int((hi - lo + 1).max(0)))
        }
        _ => Ok(Value::small_int(range_vec(vm, &r)?.len() as i64)),
    }
}
