//! Components (lang-fork-ui): the natives a `component`'s tree compiles to
//! (see `Parser::parse_component`); the elements themselves are in ui.rb.

use super::arg;
use crate::gc::ClassRef;
use crate::value::*;
use crate::vm::{Unwind, Vm};

fn class_arg(vm: &Vm, v: &Value) -> Option<ClassRef> {
    match vm.kind(*v) {
        V::Class(c) => Some(c),
        _ => None,
    }
}

/// `UI::Element`, from the prelude.
fn element_class(vm: &mut Vm) -> Option<ClassRef> {
    let ui = vm.intern("UI");
    let el = vm.intern("Element");
    let m = class_arg(vm, &vm.heap[vm.c.object].consts.borrow().get(&ui).copied()?)?;
    let e = vm.heap[m].consts.borrow().get(&el).copied()?;
    class_arg(vm, &e)
}

/// After `new` made a component and ran its init: its tree.
pub fn build(vm: &mut Vm, obj: &Value) -> Result<(), Unwind> {
    let b = vm.intern("__build");
    let cls = vm.class_of(obj);
    if vm.find_method(&cls, b).is_some() {
        vm.call_method(obj, b, vec![], None)?;
    }
    Ok(())
}

pub fn init(vm: &mut Vm) {
    let m = vm.c.module;
    vm.def(&m, "__component", |vm, s, _, _| {
        if let Some(c) = class_arg(vm, s) {
            vm.heap[c].is_component.set(true);
        }
        Ok(Value::NIL)
    });
    let o = vm.c.object;
    // an element of a tree: made without init (its props are bound next)
    vm.def(&o, "__ui_new", |vm, _, a, _| {
        let k = arg(a, 0);
        let element = element_class(vm);
        let cls = match class_arg(vm, &k) {
            Some(c) if element.is_some_and(|e| vm.ancestors(&c).contains(&e)) => c,
            _ => {
                let what = vm.inspect(&k)?;
                return Err(vm.type_err(format!(
                    "{what} isn't an element: a tree holds UI elements (use ui::{{Text, ...}}) and components"
                )));
            }
        };
        let obj = vm.object(RObject::new(cls));
        super::props::apply_defaults(vm, &obj, cls)?;
        Ok(obj)
    });
    // its props bound: a component builds its own tree
    vm.def(&o, "__ui_ready", |vm, _, a, _| {
        let e = arg(a, 0);
        let cls = vm.class_of(&e);
        if vm.heap[cls].is_component.get() {
            build(vm, &e)?;
        }
        Ok(e)
    });
}
