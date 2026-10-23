//! The `GC` module (mruby: `src/gc.c`'s `GC.start`, `GC.enable`, ...).
//!
//! A collection only runs at a safe point of the VM loop, so `GC.start`
//! requests one and it happens right after the call returns.

use super::*;

pub fn init(vm: &mut Vm) {
    let gc = vm.new_class("GC", None, true);
    vm.def_singleton(&gc, "start", |vm, _, _, _| {
        vm.heap.gc.request();
        Ok(V::Nil)
    });
    vm.def_singleton(&gc, "count", |vm, _, _, _| Ok(V::Int(vm.heap.gc.count as i64)));
    // both return whether collection was disabled before
    vm.def_singleton(&gc, "enable", |vm, _, _, _| {
        let was = !vm.heap.gc.enabled;
        vm.heap.gc.enabled = true;
        Ok(V::Bool(was))
    });
    vm.def_singleton(&gc, "disable", |vm, _, _, _| {
        let was = !vm.heap.gc.enabled;
        vm.heap.gc.enabled = false;
        Ok(V::Bool(was))
    });
    vm.def_singleton(&gc, "stress", |vm, _, _, _| Ok(V::Bool(vm.heap.gc.stress())));
    vm.def_singleton(&gc, "stress=", |vm, _, a, _| {
        vm.heap.gc.set_stress(arg(a, 0).truthy());
        Ok(arg(a, 0))
    });
    // number of live heap objects (strings, arrays, hashes, procs, ...)
    vm.def_singleton(&gc, "live", |vm, _, _, _| Ok(V::Int(vm.heap.live() as i64)));
    vm.set_const("GC", V::Class(gc));
}
