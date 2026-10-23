//! Integer and Float. mruby: `src/numeric.c`, `mrblib/numeric.rb`, `mruby-numeric-ext`, `mruby-math`.

use super::*;
use crate::vm::{Arith, VmResult};

pub fn init(vm: &mut Vm) {
    let n = vm.c.numeric;
    vm.def(&n, "+", |vm, s, a, _| vm.arith(Arith::Add, s, &arg(a, 0)));
    vm.def(&n, "-", |vm, s, a, _| vm.arith(Arith::Sub, s, &arg(a, 0)));
    vm.def(&n, "*", |vm, s, a, _| vm.arith(Arith::Mul, s, &arg(a, 0)));
    vm.def(&n, "/", |vm, s, a, _| vm.arith(Arith::Div, s, &arg(a, 0)));
    vm.def(&n, "%", |vm, s, a, _| vm.arith(Arith::Mod, s, &arg(a, 0)));
    vm.def(&n, "modulo", |vm, s, a, _| {
        vm.arith(Arith::Mod, s, &arg(a, 0))
    });
    vm.def(&n, "**", |vm, s, a, _| vm.arith(Arith::Pow, s, &arg(a, 0)));
    vm.def(&n, "pow", |vm, s, a, _| vm.arith(Arith::Pow, s, &arg(a, 0)));
    vm.def(&n, "div", |vm, s, a, _| {
        let q = vm.arith(Arith::Div, s, &arg(a, 0))?;
        Ok(vm.int(float_of(vm, &q)?.floor() as i64))
    });
    vm.def(&n, "fdiv", |vm, s, a, _| {
        Ok(Value::float(float_of(vm, s)? / float_of(vm, &arg(a, 0))?))
    });
    vm.def(&n, "divmod", |vm, s, a, _| {
        let d = arg(a, 0);
        let q = vm.arith(Arith::Div, s, &d)?;
        let q = match vm.kind(q) {
            V::Float(f) => Value::float(f.floor()),
            _ => q,
        };
        let m = vm.arith(Arith::Mod, s, &d)?;
        Ok(vm.array(vec![q, m]))
    });
    vm.def(&n, "-@", |vm, s, _, _| {
        vm.arith(Arith::Sub, &Value::small_int(0), s)
    });
    vm.def(&n, "+@", |_, s, _, _| Ok(*s));
    vm.def(&n, "==", |vm, s, a, _| {
        Ok(Value::bool(values_eq(&vm.heap, s, &arg(a, 0))))
    });
    vm.def(&n, "<", |vm, s, a, _| cmp(vm, s, a, Ordering::is_lt));
    vm.def(&n, "<=", |vm, s, a, _| cmp(vm, s, a, Ordering::is_le));
    vm.def(&n, ">", |vm, s, a, _| cmp(vm, s, a, Ordering::is_gt));
    vm.def(&n, ">=", |vm, s, a, _| cmp(vm, s, a, Ordering::is_ge));
    vm.def(&n, "<=>", |vm, s, a, _| {
        Ok(num_cmp(&vm.heap, s, &arg(a, 0))
            .map(|o| Value::small_int(o as i64))
            .unwrap_or(Value::NIL))
    });
    vm.def(&n, "between?", |vm, s, a, _| {
        Ok(Value::bool(
            compare(vm, s, &arg(a, 0))?.is_ge() && compare(vm, s, &arg(a, 1))?.is_le(),
        ))
    });
    vm.def(&n, "clamp", |vm, s, a, _| {
        let (lo, hi) = (arg(a, 0), arg(a, 1));
        Ok(if compare(vm, s, &lo)?.is_lt() {
            lo
        } else if compare(vm, s, &hi)?.is_gt() {
            hi
        } else {
            *s
        })
    });
    vm.def(&n, "to_s", to_s);
    vm.def(&n, "inspect", to_s);
    vm.def(&n, "to_i", |vm, s, _, _| Ok(vm.int(int_of(vm, s)?)));
    vm.def(&n, "to_int", |vm, s, _, _| Ok(vm.int(int_of(vm, s)?)));
    vm.def(&n, "truncate", |vm, s, _, _| Ok(vm.int(int_of(vm, s)?)));
    vm.def(&n, "to_f", |vm, s, _, _| Ok(Value::float(float_of(vm, s)?)));
    vm.def(&n, "abs", |vm, s, _, _| {
        Ok(match vm.kind(*s) {
            V::Int(i) => vm.int(i.abs()),
            V::Float(f) => Value::float(f.abs()),
            _ => unreachable!(),
        })
    });
    vm.def(&n, "zero?", |vm, s, _, _| {
        Ok(Value::bool(float_of(vm, s)? == 0.0))
    });
    vm.def(&n, "positive?", |vm, s, _, _| {
        Ok(Value::bool(float_of(vm, s)? > 0.0))
    });
    vm.def(&n, "negative?", |vm, s, _, _| {
        Ok(Value::bool(float_of(vm, s)? < 0.0))
    });
    vm.def(&n, "integer?", |vm, s, _, _| {
        Ok(Value::bool(matches!(vm.kind(*s), V::Int(_))))
    });
    vm.def(&n, "floor", |vm, s, a, _| round_with(vm, s, a, f64::floor));
    vm.def(&n, "ceil", |vm, s, a, _| round_with(vm, s, a, f64::ceil));
    vm.def(&n, "round", |vm, s, a, _| round_with(vm, s, a, f64::round));
    vm.def(&n, "step", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        let (limit, step) = (
            arg(a, 0),
            if a.len() > 1 {
                arg(a, 1)
            } else {
                Value::small_int(1)
            },
        );
        let positive = float_of(vm, &step)? > 0.0;
        let mut i = *s;
        loop {
            let o = compare(vm, &i, &limit)?;
            if (positive && o.is_gt()) || (!positive && o.is_lt()) {
                break;
            }
            vm.call_block(&b, vec![i])?;
            i = vm.arith(Arith::Add, &i, &step)?;
        }
        Ok(*s)
    });

    let i = vm.c.integer;
    vm.def(&i, "times", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        let n = int_of(vm, s)?;
        for k in 0..n {
            let v = vm.int(k);
                vm.call_block(&b, vec![v])?;
        }
        Ok(*s)
    });
    vm.def(&i, "upto", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        for k in int_of(vm, s)?..=int_of(vm, &arg(a, 0))? {
            let v = vm.int(k);
                vm.call_block(&b, vec![v])?;
        }
        Ok(*s)
    });
    vm.def(&i, "downto", |vm, s, a, b| {
        let b = need_block(vm, b)?;
        for k in (int_of(vm, &arg(a, 0))?..=int_of(vm, s)?).rev() {
            let v = vm.int(k);
                vm.call_block(&b, vec![v])?;
        }
        Ok(*s)
    });
    vm.def(&i, "even?", |vm, s, _, _| {
        Ok(Value::bool(int_of(vm, s)? % 2 == 0))
    });
    vm.def(&i, "odd?", |vm, s, _, _| {
        Ok(Value::bool(int_of(vm, s)? % 2 != 0))
    });
    vm.def(&i, "succ", |vm, s, _, _| {
        vm.arith(Arith::Add, s, &Value::small_int(1))
    });
    vm.def(&i, "next", |vm, s, _, _| {
        vm.arith(Arith::Add, s, &Value::small_int(1))
    });
    vm.def(&i, "pred", |vm, s, _, _| {
        vm.arith(Arith::Sub, s, &Value::small_int(1))
    });
    vm.def(&i, "chr", |vm, s, _, _| {
        let n = int_of(vm, s)?;
        char::from_u32(n as u32)
            .map(|c| vm.str(c.to_string()))
            .ok_or_else(|| vm.err(&vm.c.argument_error, format!("{n} out of char range")))
    });
    vm.def(&i, "gcd", |vm, s, a, _| {
        Ok(vm.int(gcd(int_of(vm, s)?, int_of(vm, &arg(a, 0))?)))
    });
    vm.def(&i, "lcm", |vm, s, a, _| {
        let (x, y) = (int_of(vm, s)?, int_of(vm, &arg(a, 0))?);
        Ok(vm.int(if x == 0 || y == 0 {
            0
        } else {
            (x / gcd(x, y) * y).abs()
        }))
    });
    vm.def(&i, "digits", |vm, s, _, _| {
        let mut n = int_of(vm, s)?;
        if n < 0 {
            return Err(vm.err(&vm.c.argument_error, "out of domain"));
        }
        let mut v = vec![Value::small_int(n % 10)];
        n /= 10;
        while n > 0 {
            v.push(Value::small_int(n % 10));
            n /= 10;
        }
        Ok(vm.array(v))
    });
    vm.def(&i, "&", |vm, s, a, _| {
        Ok(vm.int(int_of(vm, s)? & int_of(vm, &arg(a, 0))?))
    });
    vm.def(&i, "|", |vm, s, a, _| {
        Ok(vm.int(int_of(vm, s)? | int_of(vm, &arg(a, 0))?))
    });
    vm.def(&i, "^", |vm, s, a, _| {
        Ok(vm.int(int_of(vm, s)? ^ int_of(vm, &arg(a, 0))?))
    });
    vm.def(&i, "<<", |vm, s, a, _| {
        Ok(vm.int(
            int_of(vm, s)?.wrapping_shl(int_of(vm, &arg(a, 0))? as u32),
        ))
    });
    vm.def(&i, ">>", |vm, s, a, _| {
        Ok(vm.int(
            int_of(vm, s)?.wrapping_shr(int_of(vm, &arg(a, 0))? as u32),
        ))
    });

    let f = vm.c.float;
    vm.def(&f, "nan?", |vm, s, _, _| {
        Ok(Value::bool(float_of(vm, s)?.is_nan()))
    });
    vm.def(&f, "infinite?", |vm, s, _, _| {
        let x = float_of(vm, s)?;
        Ok(if x.is_infinite() {
            vm.int(if x > 0.0 { 1 } else { -1 })
        } else {
            Value::NIL
        })
    });
    vm.def(&f, "finite?", |vm, s, _, _| {
        Ok(Value::bool(float_of(vm, s)?.is_finite()))
    });

    let version = vm.str("3.4.0");
    vm.set_const("RUBY_VERSION", version);
    let engine = vm.str("ruruby");
    vm.set_const("RUBY_ENGINE", engine);
    let float_consts = [
        ("INFINITY", f64::INFINITY),
        ("NAN", f64::NAN),
        ("EPSILON", f64::EPSILON),
        ("MAX", f64::MAX),
        ("MIN", f64::MIN_POSITIVE),
    ];
    for (name, v) in float_consts {
        let s = vm.intern(name);
        vm.heap[f].consts.borrow_mut().insert(s, Value::float(v));
    }
    let math = vm.new_class("Math", None, true);
    for (name, v) in [("PI", std::f64::consts::PI), ("E", std::f64::consts::E)] {
        let s = vm.intern(name);
        vm.heap[math].consts.borrow_mut().insert(s, Value::float(v));
    }
    vm.def_singleton(&math, "sqrt", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.sqrt()))
    });
    vm.def_singleton(&math, "cbrt", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.cbrt()))
    });
    vm.def_singleton(&math, "sin", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.sin()))
    });
    vm.def_singleton(&math, "cos", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.cos()))
    });
    vm.def_singleton(&math, "tan", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.tan()))
    });
    vm.def_singleton(&math, "atan", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.atan()))
    });
    vm.def_singleton(&math, "exp", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.exp()))
    });
    vm.def_singleton(&math, "log", |vm, _, a, _| {
        let x = float_of(vm, &arg(a, 0))?;
        Ok(Value::float(if a.len() > 1 {
            x.log(float_of(vm, &a[1])?)
        } else {
            x.ln()
        }))
    });
    vm.def_singleton(&math, "log2", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.log2()))
    });
    vm.def_singleton(&math, "log10", |vm, _, a, _| {
        Ok(Value::float(float_of(vm, &arg(a, 0))?.log10()))
    });
    vm.def_singleton(&math, "hypot", |vm, _, a, _| {
        Ok(Value::float(
            float_of(vm, &arg(a, 0))?.hypot(float_of(vm, &arg(a, 1))?),
        ))
    });
    vm.set_const("Math", Value::from(math));
}

fn cmp(vm: &mut Vm, s: &Value, a: &[Value], pred: fn(Ordering) -> bool) -> VmResult {
    let o = arg(a, 0);
    match num_cmp(&vm.heap, s, &o) {
        Some(ord) => Ok(Value::bool(pred(ord))),
        None if o.is_numeric() => Ok(Value::FALSE), // NaN
        None => Err(vm.arg_err(format!(
            "comparison of {} with {} failed",
            vm.class_name(s),
            vm.class_name(&o)
        ))),
    }
}

fn to_s(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    match (vm.kind(*s), a) {
        (V::Int(i), [base]) => {
            let base = int_of(vm, base)?;
            if !(2..=36).contains(&base) {
                return Err(vm.arg_err(format!("invalid radix {base}")));
            }
            let (neg, mut n) = (i < 0, i.unsigned_abs());
            let mut digits = Vec::new();
            loop {
                digits.push(std::char::from_digit((n % base as u64) as u32, base as u32).unwrap());
                n /= base as u64;
                if n == 0 {
                    break;
                }
            }
            if neg {
                digits.push('-');
            }
            Ok(vm.str(digits.iter().rev().collect::<String>()))
        }
        _ => Ok({ let v = vm.to_s(s)?; vm.str(v) }),
    }
}

fn round_with(vm: &mut Vm, s: &Value, a: &[Value], f: fn(f64) -> f64) -> VmResult {
    let digits = if a.is_empty() { 0 } else { int_of(vm, &a[0])? };
    match vm.kind(*s) {
        V::Int(_) if digits >= 0 => Ok(*s),
        _ => {
            let x = float_of(vm, s)?;
            if digits > 0 {
                let m = 10f64.powi(digits as i32);
                Ok(Value::float(f(x * m) / m))
            } else {
                let m = 10f64.powi((-digits) as i32);
                let r = f(x / m) * m;
                if r.is_finite() {
                    Ok(vm.int(r as i64))
                } else {
                    Ok(Value::float(r))
                }
            }
        }
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}
