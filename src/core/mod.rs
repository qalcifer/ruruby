//! Built-in classes and methods. In mruby these live in `src/*.c` (C
//! functions registered with `mrb_define_method`), `mrblib/*.rb` (core
//! methods written in Ruby), and the default mrbgems (`mruby-string-ext`,
//! `mruby-array-ext`, `mruby-enum-ext`, ...).

pub mod array;
pub mod enumerable;
pub mod gc;
pub mod hash;
pub mod kernel;
pub mod load;
pub mod numeric;
pub mod range;
pub mod reflect;
pub mod string;

pub use crate::gc::{ArrayRef, ClassRef, HashRef, StrRef};
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{Unwind, Vm};
use std::cell::RefCell;
use std::cmp::Ordering;

pub fn init(vm: &mut Vm) {
    kernel::init(vm);
    numeric::init(vm);
    string::init(vm);
    array::init(vm);
    array::init_more(vm);
    hash::init(vm);
    range::init(vm);
    enumerable::init(vm);
    reflect::init(vm);
    gc::init(vm);
    load::init(vm);
}

/// Core methods written in Ruby (`prelude.rb`), compiled into every VM.
pub fn load_prelude(vm: &mut Vm) {
    const PRELUDE: &str = include_str!("prelude.rb");
    let prog = crate::parser::parse_file(PRELUDE, "prelude.rb", 1).expect("prelude parses");
    let irep = crate::codegen::compile(&prog, &mut vm.syms).expect("prelude compiles");
    if let Err(e) = vm.run_main(irep) {
        panic!("prelude failed: {}", vm.exc_message(&e));
    }
}

/// Argument `i`, or nil when missing.
pub fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::NIL)
}

pub fn check_argc(vm: &Vm, a: &[Value], min: usize, max: usize) -> Result<(), Unwind> {
    if a.len() < min || a.len() > max {
        let expected = if min == max {
            min.to_string()
        } else {
            format!("{min}..{max}")
        };
        return Err(vm.arg_err(format!(
            "wrong number of arguments (given {}, expected {})",
            a.len(),
            expected
        )));
    }
    Ok(())
}

pub fn need_block(vm: &Vm, b: Option<&Value>) -> Result<Value, Unwind> {
    b.cloned()
        .ok_or_else(|| vm.err(&vm.c.local_jump_error, "no block given (yield)"))
}

pub fn int_of(vm: &Vm, v: &Value) -> Result<i64, Unwind> {
    match vm.kind(*v) {
        V::Int(i) => Ok(i),
        // truncated; RangeError where no Integer fits (NaN, infinite, huge)
        V::Float(f) if f.is_finite() && f.abs() < 9.2e18 => Ok(f as i64),
        V::Float(f) => Err(vm.err(
            &vm.c.range_error,
            format!("float {} out of range of integer", crate::value::fmt_float(f)),
        )),
        _ => Err(vm.type_err(format!(
            "no implicit conversion of {} into Integer",
            describe(vm, v)
        ))),
    }
}

pub fn float_of(vm: &Vm, v: &Value) -> Result<f64, Unwind> {
    match vm.kind(*v) {
        V::Int(i) => Ok(i as f64),
        V::Float(f) => Ok(f),
        _ => Err(vm.type_err(format!("can't convert {} into Float", describe(vm, v)))),
    }
}

pub fn str_of(vm: &Vm, v: &Value) -> Result<String, Unwind> {
    match vm.kind(*v) {
        V::Str(s) => Ok(vm.heap[s].borrow().clone()),
        _ => Err(vm.type_err(format!(
            "no implicit conversion of {} into String",
            describe(vm, v)
        ))),
    }
}

pub fn sym_of(vm: &mut Vm, v: &Value) -> Result<Sym, Unwind> {
    match vm.kind(*v) {
        V::Sym(s) => Ok(s),
        V::Str(s) => {
            let s = vm.heap[s].borrow().clone();
            Ok(vm.intern(&s))
        }
        _ => Err(vm.type_err(format!("{} is not a symbol nor a string", describe(vm, v)))),
    }
}

fn describe(vm: &Vm, v: &Value) -> String {
    if v.is_nil() {
        "nil".into()
    } else {
        vm.class_name(v)
    }
}

pub fn this_str(vm: &Vm, v: &Value) -> Result<StrRef, Unwind> {
    match vm.kind(*v) {
        V::Str(s) => Ok(s),
        _ => Err(vm.type_err("expected a String receiver")),
    }
}

pub fn this_ary(vm: &Vm, v: &Value) -> Result<ArrayRef, Unwind> {
    match vm.kind(*v) {
        V::Array(a) => Ok(a),
        _ => Err(vm.type_err("expected an Array receiver")),
    }
}

pub fn this_hash(vm: &Vm, v: &Value) -> Result<HashRef, Unwind> {
    match vm.kind(*v) {
        V::Hash(h) => Ok(h),
        _ => Err(vm.type_err("expected a Hash receiver")),
    }
}

/// `a <=> b` as an Ordering (numbers and strings natively, otherwise via `<=>`).
pub fn compare(vm: &mut Vm, a: &Value, b: &Value) -> Result<Ordering, Unwind> {
    if let Some(o) = num_cmp(&vm.heap, a, b) {
        return Ok(o);
    }
    if let (V::Str(x), V::Str(y)) = (vm.kind(*a), vm.kind(*b)) {
        return Ok(vm.heap[x].borrow().as_str().cmp(vm.heap[y].borrow().as_str()));
    }
    if let (V::Array(x), V::Array(y)) = (vm.kind(*a), vm.kind(*b)) {
        let (x, y) = (vm.heap[x].borrow().clone(), vm.heap[y].borrow().clone());
        for (p, q) in x.iter().zip(&y) {
            let o = compare(vm, p, q)?;
            if o != Ordering::Equal {
                return Ok(o);
            }
        }
        return Ok(x.len().cmp(&y.len()));
    }
    let r = if a.is_numeric() || matches!(vm.kind(*a), V::Str(_)) {
        Value::NIL
    } else {
        vm.call_method(a, vm.s.cmp, vec![*b], None)?
    };
    match vm.kind(r) {
        V::Int(i) => Ok(i.cmp(&0)),
        _ => Err(vm.arg_err(format!(
            "comparison of {} with {} failed",
            vm.class_name(a),
            describe(vm, b)
        ))),
    }
}

type CmpFn<'a> = dyn FnMut(&mut Vm, &Value, &Value) -> Result<Ordering, Unwind> + 'a;

/// Stable merge sort that can propagate Ruby exceptions from the comparator.
pub fn merge_sort(vm: &mut Vm, mut v: Vec<Value>, cmp: &mut CmpFn) -> Result<Vec<Value>, Unwind> {
    if v.len() <= 1 {
        return Ok(v);
    }
    let right = v.split_off(v.len() / 2);
    let left = merge_sort(vm, v, cmp)?;
    let right = merge_sort(vm, right, cmp)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if cmp(vm, &right[j], &left[i])? == Ordering::Less {
            out.push(right[j]);
            j += 1;
        } else {
            out.push(left[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Ok(out)
}

pub fn sort_values(vm: &mut Vm, v: Vec<Value>, blk: Option<&Value>) -> Result<Vec<Value>, Unwind> {
    match blk {
        Some(b) => {
            let b = *b;
            merge_sort(
                vm,
                v,
                &mut |vm, x, y| match vm.call_block(&b, vec![*x, *y]).map(|r| vm.kind(r))? {
                    V::Int(i) => Ok(i.cmp(&0)),
                    V::Float(f) if !f.is_nan() => Ok(f.partial_cmp(&0.0).unwrap_or(Ordering::Equal)),
                    _ => {
                        let (cx, cy) = (vm.class_name(x), vm.class_name(y));
                        Err(vm.arg_err(format!("comparison of {cx} with {cy} failed")))
                    }
                },
            )
        }
        None => merge_sort(vm, v, &mut compare),
    }
}

/// Materializes anything enumerable: Array, Hash (as [k, v] pairs), Range,
/// or any object with an `each` method.
pub fn to_vec(vm: &mut Vm, v: &Value) -> Result<Vec<Value>, Unwind> {
    match vm.kind(*v) {
        V::Array(a) => Ok(vm.heap[a].borrow().clone()),
        V::Hash(h) => {
            let pairs = vm.heap[h].borrow().to_vec();
            Ok(pairs.into_iter().map(|(k, v)| vm.array(vec![k, v])).collect())
        }
        V::Range(r) => {
            let r = vm.heap[r];
            range::range_vec(vm, &r)
        }
        _ => {
            let buf = vm.heap.alloc(RefCell::new(Vec::new()));
            let collector = vm.proc_value(ProcBody::Collect(buf));
            vm.call_method(v, vm.s.each, vec![], Some(collector))?;
            let out = vm.heap[buf].borrow().clone();
            Ok(out)
        }
    }
}
