//! Panics (lang-fork): a bug or broken invariant stops the program, like
//! Rust's `panic!`. Expected failures are `Err(...)` values (result.rb).
//!
//! `panic(message)` panics; errors the VM and core methods detect (calling a
//! missing method, a wrong argument type, dividing by zero) are panics of
//! their kind (`NoMethodError`, `TypeError`, ...). Nothing in normal code
//! catches them. `catch_panic { }` is the counterpart of Rust's
//! `std::panic::catch_unwind`, for tests and for boundaries that must
//! survive a failing callback: `Ok(value)`, or `Err(panic)` with its
//! `message` and `class`.

use super::need_block;
use crate::value::*;
use crate::vm::{Unwind, Vm};

pub fn init(vm: &mut Vm) {
    let exception = vm.c.exception;
    let panic = vm.new_class("Panic", Some(exception), false);
    vm.set_const("Panic", Value::from(panic));
    let o = vm.c.object;
    // Kernel#raise for code the compiler generates (an unmatched `case/in`)
    let (raise, internal) = (vm.intern("raise"), vm.intern("__raise"));
    if let Some((m, _)) = vm.find_method(&o, raise) {
        vm.heap[o].methods.insert(internal, m);
    }
    vm.def(&o, "panic", |vm, _, a, _| {
        let msg = match a.first() {
            None => "explicit panic".to_string(),
            Some(v) => vm.to_s(v)?,
        };
        let s = vm.intern("Panic");
        let found = vm.heap[vm.c.object].consts.borrow().get(&s).copied();
        let panic = match found.map(|v| vm.kind(v)) {
            Some(V::Class(c)) => c,
            _ => vm.c.runtime_error,
        };
        Err(vm.err(&panic, msg))
    });
    vm.def(&o, "catch_panic", |vm, _, _, b| {
        let b = need_block(vm, b)?;
        let (ok, err) = match vm.results {
            Some((ok, err, _)) => (ok, err),
            None => return Err(vm.err(&vm.c.runtime_error, "catch_panic before result.rb")),
        };
        let (class, v) = match vm.call_block(&b, vec![]) {
            Ok(v) => (ok, v),
            Err(e) => match vm.materialize(e) {
                Unwind::Error(exc) => (err, exc),
                // break/next/return pass through
                other => return Err(other),
            },
        };
        let new = vm.intern("new");
        vm.call_method(&Value::from(class), new, vec![v], None)
    });
}
