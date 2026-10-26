//! Reactive natives (lang-fork-ui), on top of src/reactive.rs:
//!
//! ```text
//! e = effect { puts "count: #{counter.count}" }   # runs now, then after each change
//! bind(label, :text) { "Count: #{counter.count}" }  # keeps label.text up to date
//! batch { a.x = 1; a.y = 2 }                       # what depends on both runs once
//! untracked { counter.count }                      # a read that isn't a dependency
//! e.stop
//! ```

use super::{arg, need_block};
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{Unwind, Vm, VmResult};

/// Times one computation may run in one flush before it's called a cycle
/// (a binding whose value feeds back into itself).
const MAX_RUNS: usize = 100;

/// Runs computation `c` once, recording what it reads.
fn run(vm: &mut Vm, c: usize) -> Result<(), Unwind> {
    let Some((block, target)) = vm.rx.begin(c) else {
        return Ok(());
    };
    let r = vm.call_block(&block, vec![]);
    vm.rx.end();
    let v = r?;
    if let Some((obj, writer)) = target {
        vm.call_method(&obj, writer, vec![v], None)?;
    }
    Ok(())
}

/// Runs the computations that changes queued, until none are (unless a
/// batch is open or a flush is already running further up).
pub fn flush(vm: &mut Vm) -> Result<(), Unwind> {
    if !vm.rx.start_flush() {
        return Ok(());
    }
    let r = flush_rounds(vm);
    vm.rx.end_flush();
    r
}

fn flush_rounds(vm: &mut Vm) -> Result<(), Unwind> {
    let mut runs: std::collections::HashMap<usize, usize> = Default::default();
    while let Some(c) = vm.rx.next() {
        let n = runs.entry(c).or_default();
        *n += 1;
        if *n > MAX_RUNS {
            return Err(vm.type_err(format!(
                "reactive cycle: a computation ran {MAX_RUNS} times for one change (does a binding feed its own input?)"
            )));
        }
        run(vm, c)?;
    }
    Ok(())
}

/// The `Effect` handle of computation `c`.
fn handle(vm: &mut Vm, c: usize) -> VmResult {
    let s = vm.intern("Effect");
    let class = match vm.heap[vm.c.object].consts.borrow().get(&s).map(|v| vm.kind(*v)) {
        Some(V::Class(k)) => k,
        _ => vm.c.object,
    };
    let h = vm.object(RObject::new(class));
    let id = vm.intern("%id");
    vm.ivar_set(&h, id, Value::small_int(c as i64))?;
    vm.freeze(&h);
    Ok(h)
}

fn handle_id(vm: &mut Vm, s: &Value) -> Option<usize> {
    let id = vm.intern("%id");
    match vm.kind(vm.ivar_lookup(s, id)?) {
        V::Int(n) => Some(n as usize),
        _ => None,
    }
}

fn start(vm: &mut Vm, block: Value, target: Option<(Value, Sym)>) -> VmResult {
    let c = vm.rx.add(block, target);
    // its first run's writes are flushed after it
    if let Err(e) = run(vm, c).and_then(|_| flush(vm)) {
        vm.rx.stop(c);
        return Err(e);
    }
    handle(vm, c)
}

pub fn init(vm: &mut Vm) {
    let effect = vm.new_class("Effect", Some(vm.c.object), false);
    vm.set_const("Effect", Value::from(effect));
    let o = vm.c.object;
    vm.def(&o, "effect", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        start(vm, b, None)
    });
    // bind(obj, :prop) { value }: obj.prop = value, now and when it changes
    vm.def(&o, "bind", |vm, _, a, b| {
        let b = need_block(vm, b)?;
        let obj = arg(a, 0);
        let prop = match vm.kind(arg(a, 1)) {
            V::Sym(s) => vm.sym_name(s).to_string(),
            V::Str(s) => vm.heap[s].borrow().clone(),
            _ => return Err(vm.type_err("bind(object, :prop) { value }")),
        };
        let writer = vm.intern(&format!("{prop}="));
        if vm.find_method(&vm.class_of(&obj), writer).is_none() {
            let what = vm.inspect(&obj)?;
            return Err(vm.type_err(format!("bind: {what} has no writer {prop}=")));
        }
        start(vm, b, Some((obj, writer)))
    });
    vm.def(&o, "batch", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        vm.rx.begin_batch();
        let r = vm.call_block(&b, vec![]);
        vm.rx.end_batch();
        let v = r?;
        flush(vm)?;
        Ok(v)
    });
    vm.def(&o, "untracked", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        vm.rx.begin_untracked();
        let r = vm.call_block(&b, vec![]);
        vm.rx.end();
        r
    });
    vm.def(&effect, "stop", |vm, s, _, _| {
        if let Some(c) = handle_id(vm, s) {
            vm.rx.stop(c);
        }
        Ok(Value::NIL)
    });
    vm.def(&effect, "active?", |vm, s, _, _| {
        let c = handle_id(vm, s);
        Ok(Value::bool(c.is_some_and(|c| vm.rx.active(c))))
    });
    vm.def(&effect, "inspect", |vm, s, _, _| {
        let c = handle_id(vm, s);
        let state = if c.is_some_and(|c| vm.rx.active(c)) { "active" } else { "stopped" };
        Ok(vm.str(format!("#<Effect {state}>")))
    });
    // (live computations, watched fields): for tests
    let rx = vm.intern("Reactive");
    let module = vm.new_class("Reactive", None, true);
    vm.heap[vm.c.object].consts.borrow_mut().insert(rx, Value::from(module));
    vm.def_singleton(&module, "stats", |vm, _, _, _| {
        let (c, f) = vm.rx.stats();
        let v = vec![Value::small_int(c as i64), Value::small_int(f as i64)];
        Ok(vm.array(v))
    });
}
