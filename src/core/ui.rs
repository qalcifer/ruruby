//! Components (lang-fork-ui): the natives a `component`'s tree compiles to
//! (see `Parser::parse_component`), and the recording of the tree's changes
//! for a renderer (src/ui.rs); the elements themselves are in ui.rb.

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

fn is_fragment(vm: &mut Vm, cls: ClassRef) -> bool {
    let (ui, frag) = (vm.intern("UI"), vm.intern("Fragment"));
    let found = vm.heap[vm.c.object].consts.borrow().get(&ui).copied();
    let Some(m) = found.and_then(|v| class_arg(vm, &v)) else { return false };
    let f = vm.heap[m].consts.borrow().get(&frag).copied();
    f.and_then(|v| class_arg(vm, &v)).is_some_and(|f| vm.ancestors(&cls).contains(&f))
}

fn object_id(vm: &Vm, v: &Value) -> Option<u64> {
    match vm.kind(*v) {
        V::Object(o) => Some(o.id()),
        _ => None,
    }
}

fn node_of(vm: &Vm, v: &Value) -> Option<u64> {
    object_id(vm, v).and_then(|o| vm.ui.node(o))
}

/// A recorded value back as a script value (UI::mutations).
fn prop_to_value(vm: &mut Vm, p: &crate::ui::PropValue) -> Value {
    use crate::ui::PropValue as P;
    match p {
        P::None => Value::NIL,
        P::Bool(b) => Value::bool(*b),
        P::Int(n) => vm.int(*n),
        P::Float(f) => Value::float(*f),
        P::Str(s) | P::Other(s) => vm.str(s.clone()),
        P::Sym(s) => Value::sym(vm.intern(s)),
        P::List(items) => {
            let v = items.iter().map(|x| prop_to_value(vm, x)).collect();
            vm.array(v)
        }
    }
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
        // a node a renderer sees (not a component or an if/for part): its
        // writes go through the slow path, which records them
        if !vm.heap[cls].is_component.get() && !is_fragment(vm, cls)
            && let V::Object(o) = vm.kind(obj)
        {
            vm.heap[o].element.set(true);
            vm.heap[o].observed.set(true);
        }
        // part of a tree (not a root, even before it's attached)
        let member = vm.intern("@__member");
        vm.ivar_set(&obj, member, Value::TRUE)?;
        super::props::apply_defaults(vm, &obj, cls)?;
        Ok(obj)
    });
    // (element, kind, [[prop, value], ...]): its Create and props, once
    vm.def(&o, "__ui_log_create", |vm, _, a, _| {
        let (el, kind) = (arg(a, 0), arg(a, 1));
        let (Some(object), V::Str(k)) = (object_id(vm, &el), vm.kind(kind)) else {
            return Ok(Value::NIL);
        };
        let kind = vm.heap[k].borrow().clone();
        let Some(id) = vm.ui.create(object, el, &kind) else {
            return Ok(Value::NIL);
        };
        let props = match vm.kind(arg(a, 2)) {
            V::Array(p) => vm.heap[p].borrow().clone(),
            _ => vec![],
        };
        for pair in props {
            let V::Array(pa) = vm.kind(pair) else { continue };
            let (name, value) = {
                let x = vm.heap[pa].borrow();
                (x.first().copied().unwrap_or(Value::NIL), x.get(1).copied().unwrap_or(Value::NIL))
            };
            let V::Sym(name) = vm.kind(name) else { continue };
            let name = vm.sym_name(name).to_string();
            let value = vm.prop_value(&value);
            vm.ui.set(id, &name, value);
        }
        Ok(Value::small_int(id as i64))
    });
    vm.def(&o, "__ui_log_node", |vm, _, a, _| {
        Ok(match object_id(vm, &arg(a, 0)).and_then(|o| vm.ui.node(o)) {
            Some(id) => Value::small_int(id as i64),
            None => Value::NIL,
        })
    });
    // (parent, index, child) / (parent, child): both already nodes
    vm.def(&o, "__ui_log_insert", |vm, _, a, _| {
        let ids = (node_of(vm, &arg(a, 0)), node_of(vm, &arg(a, 2)));
        if let ((Some(p), Some(c)), V::Int(i)) = (ids, vm.kind(arg(a, 1))) {
            vm.ui.insert(p, i as usize, c);
        }
        Ok(Value::NIL)
    });
    vm.def(&o, "__ui_log_remove", |vm, _, a, _| {
        if let (Some(p), Some(c)) = (node_of(vm, &arg(a, 0)), node_of(vm, &arg(a, 1))) {
            vm.ui.remove(p, c);
        }
        Ok(Value::NIL)
    });
    vm.def(&o, "__ui_log_forget", |vm, _, a, _| {
        if let Some(o) = object_id(vm, &arg(a, 0)) {
            vm.ui.forget(o);
        }
        Ok(Value::NIL)
    });
    // the changes since the last call, as arrays: UI::mutations (for tests;
    // a Rust host uses Interpreter::take_ui_mutations)
    vm.def(&o, "__ui_mutations", |vm, _, _, _| {
        use crate::ui::Mutation as M;
        let mut out = vec![];
        for m in vm.ui.take() {
            let int = |n: u64| Value::small_int(n as i64);
            let row = match m {
                M::Create { id, kind } => vec![Value::sym(vm.intern("create")), int(id), vm.str(kind)],
                M::Set { id, prop, value } => {
                    let v = prop_to_value(vm, &value);
                    vec![Value::sym(vm.intern("set")), int(id), Value::sym(vm.intern(&prop)), v]
                }
                M::Insert { parent, index, child } => {
                    vec![Value::sym(vm.intern("insert")), int(parent), int(index as u64), int(child)]
                }
                M::Remove { parent, child } => vec![Value::sym(vm.intern("remove")), int(parent), int(child)],
            };
            out.push(vm.array(row));
        }
        Ok(vm.array(out))
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
