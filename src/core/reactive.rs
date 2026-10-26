//! Reactive natives (lang-fork-ui), on `ruruby-reactive` (src/reactive.rs):
//!
//! ```text
//! e = effect { puts "count: #{counter.count}" }   # runs now, then after each change
//! bind(label, :text) { "Count: #{counter.count}" }  # keeps label.text up to date
//! batch { a.x = 1; a.y = 2 }                       # what depends on both runs once
//! untracked { counter.count }                      # a read that isn't a dependency
//! e.stop
//! ```

use super::{arg, need_block};
use crate::reactive::{Computation, key};
use crate::symbol::Sym;
use crate::value::*;
use crate::vm::{Unwind, Vm, VmResult};
use ruruby_reactive::{FlushError, MAX_RUNS};

/// Runs the computation with `tag` once (the runtime records its reads).
fn run_tag(vm: &mut Vm, tag: u64) -> Result<(), Unwind> {
    let Some(c) = vm.rx.comps.get(&tag) else {
        return Ok(());
    };
    let (block, target) = (c.block, c.target);
    let v = vm.call_block(&block, vec![])?;
    if let Some((obj, writer)) = target {
        vm.call_method(&obj, writer, vec![v], None)?;
    }
    Ok(())
}

fn flush_error(vm: &Vm, e: FlushError<Unwind>) -> Unwind {
    match e {
        FlushError::External(e) => e,
        FlushError::Cycle(_) => vm.type_err(format!(
            "reactive cycle: a computation ran {MAX_RUNS} times for one change (does a binding feed its own input?)"
        )),
    }
}

/// Runs what changes queued (unless a batch is open or a computation or
/// flush is running further up: it flushes when it ends).
pub fn flush(vm: &mut Vm) -> Result<(), Unwind> {
    let rt = vm.rx.rt.clone();
    rt.flush_with(&mut |_, tag| run_tag(vm, tag)).map_err(|e| flush_error(vm, e))
}

/// The `Effect` handle of computation `tag`.
fn handle(vm: &mut Vm, tag: u64) -> VmResult {
    let s = vm.intern("Effect");
    let class = match vm.heap[vm.c.object].consts.borrow().get(&s).map(|v| vm.kind(*v)) {
        Some(V::Class(k)) => k,
        _ => vm.c.object,
    };
    let h = vm.object(RObject::new(class));
    let id = vm.intern("%id");
    vm.ivar_set(&h, id, Value::small_int(tag as i64))?;
    vm.freeze(&h);
    Ok(h)
}

/// The live node of an `Effect` handle.
fn handle_node(vm: &mut Vm, s: &Value) -> Option<ruruby_reactive::NodeId> {
    let id = vm.intern("%id");
    let V::Int(tag) = vm.kind(vm.ivar_lookup(s, id)?) else { return None };
    let node = vm.rx.comps.get(&(tag as u64))?.node;
    vm.rx.rt.is_alive(node).then_some(node)
}

fn start(vm: &mut Vm, block: Value, target: Option<(Value, Sym)>) -> VmResult {
    let tag = vm.rx.new_tag();
    // a binding's output is its target field: what reads it runs after it
    let output = target.and_then(|(obj, writer)| match vm.kind(obj) {
        V::Object(o) => {
            let name = vm.sym_name(writer).trim_end_matches('=').to_string();
            Some(key(o.id(), vm.intern(&format!("@{name}"))))
        }
        _ => None,
    });
    let rt = vm.rx.rt.clone();
    let node = rt.external(tag, output);
    vm.rx.comps.insert(tag, Computation { block, target, node });
    let r = rt.run_now(node, &mut |_, tag| run_tag(vm, tag)).and_then(|_| flush(vm));
    if let Err(e) = r {
        rt.dispose(node);
        vm.rx.comps.remove(&tag);
        return Err(e);
    }
    handle(vm, tag)
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
        vm.rx.rt.begin_batch();
        let r = vm.call_block(&b, vec![]);
        vm.rx.rt.end_batch();
        let v = r?;
        flush(vm)?;
        Ok(v)
    });
    vm.def(&o, "untracked", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        vm.rx.rt.begin_untracked();
        let r = vm.call_block(&b, vec![]);
        vm.rx.rt.end_untracked();
        r
    });
    vm.def(&effect, "stop", |vm, s, _, _| {
        if let Some(n) = handle_node(vm, s) {
            vm.rx.rt.dispose(n);
        }
        Ok(Value::NIL)
    });
    vm.def(&effect, "active?", |vm, s, _, _| Ok(Value::bool(handle_node(vm, s).is_some())));
    vm.def(&effect, "inspect", |vm, s, _, _| {
        let state = if handle_node(vm, s).is_some() { "active" } else { "stopped" };
        Ok(vm.str(format!("#<Effect {state}>")))
    });
    // [live computations, watched fields]: for tests
    let rx = vm.intern("Reactive");
    let module = vm.new_class("Reactive", None, true);
    vm.heap[vm.c.object].consts.borrow_mut().insert(rx, Value::from(module));
    vm.def_singleton(&module, "stats", |vm, _, _, _| {
        let (c, f) = vm.rx.rt.stats();
        let v = vec![Value::small_int(c as i64), Value::small_int(f as i64)];
        Ok(vm.array(v))
    });
}
