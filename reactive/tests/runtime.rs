use ruruby_reactive::{FlushError, NodeId, Runtime};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

fn log() -> Rc<RefCell<Vec<String>>> {
    Rc::new(RefCell::new(vec![]))
}

#[test]
fn signals_effects_batch_untracked() {
    let rt = Runtime::new();
    let a = rt.signal(1);
    let b = rt.signal(10);
    let out = log();
    let e = rt.effect({
        let (rt, a, b, out) = (rt.clone(), a.clone(), b.clone(), out.clone());
        move || {
            let bv = rt.untracked(|| b.get());
            out.borrow_mut().push(format!("{} {}", a.get(), bv))
        }
    });
    a.set(2);
    a.set(2); // the same value: nothing
    b.set(20); // read untracked: nothing
    rt.batch(|| {
        a.set(3);
        a.set(4);
    });
    e.dispose();
    a.set(5);
    assert_eq!(*out.borrow(), ["1 10", "2 10", "4 20"]);
    assert!(!e.is_alive());
}

/// a → b, a → c, (b, c) → effect: the effect runs once per change of a,
/// after both memos.
#[test]
fn diamond_is_glitch_free() {
    let rt = Runtime::new();
    let a = rt.signal(1);
    let b = rt.memo({
        let a = a.clone();
        move || a.get() + 1
    });
    let c = rt.memo({
        let a = a.clone();
        move || a.get() * 10
    });
    let out = log();
    let _e = rt.effect({
        let (b, c, out) = (b.clone(), c.clone(), out.clone());
        move || out.borrow_mut().push(format!("{}", b.get() + c.get()))
    });
    a.set(2);
    a.set(3);
    assert_eq!(*out.borrow(), ["12", "23", "34"]);
}

/// A memo isn't computed until read; one whose value doesn't change stops
/// the update there.
#[test]
fn memos_are_lazy_and_cut_off() {
    let rt = Runtime::new();
    let n = rt.signal(1);
    let computed = Rc::new(RefCell::new(0));
    let parity = rt.memo({
        let (n, computed) = (n.clone(), computed.clone());
        move || {
            *computed.borrow_mut() += 1;
            n.get() % 2
        }
    });
    assert_eq!(*computed.borrow(), 0, "not read yet");
    let out = log();
    let _e = rt.effect({
        let (parity, out) = (parity.clone(), out.clone());
        move || out.borrow_mut().push(format!("{}", parity.get()))
    });
    n.set(3); // parity is still 1: the effect doesn't run
    n.set(4);
    assert_eq!(*out.borrow(), ["1", "0"]);
    assert_eq!(*computed.borrow(), 3);
    assert_eq!(parity.get(), 0);
}

/// Effects made in an effect belong to it; cleanups run before re-runs
/// and on dispose.
#[test]
fn ownership_and_cleanup() {
    let rt = Runtime::new();
    let outer = rt.signal(0);
    let inner = rt.signal(0);
    let out = log();
    let e = rt.effect({
        let (rt2, outer, inner, out) = (rt.clone(), outer.clone(), inner.clone(), out.clone());
        move || {
            let o = outer.get();
            let out2 = out.clone();
            rt2.on_cleanup(move || out2.borrow_mut().push(format!("cleanup {o}")));
            let (inner, out3) = (inner.clone(), out.clone());
            let _child = rt2.effect(move || out3.borrow_mut().push(format!("inner {o} {}", inner.get())));
        }
    });
    inner.set(1);
    outer.set(1);
    inner.set(2);
    assert_eq!(rt.stats().0, 2);
    e.dispose();
    inner.set(3);
    assert_eq!(
        *out.borrow(),
        ["inner 0 0", "inner 0 1", "cleanup 0", "inner 1 1", "inner 1 2", "cleanup 1"]
    );
    assert_eq!(rt.stats(), (0, 0));
}

/// An effect writing what it reads stops at the run limit.
#[test]
fn cycles_stop() {
    let rt = Runtime::new();
    let n = rt.signal(0);
    let _e = rt.effect({
        let n = n.clone();
        move || {
            let v = n.get();
            if v < 1000 {
                n.set(v + 1)
            }
        }
    });
    assert!(n.get_untracked() < 1000, "stopped by the run limit: {}", n.get_untracked());
}

type Fields = Rc<RefCell<HashMap<u128, i64>>>;

fn read(rt: &Runtime, f: &Fields, k: u128) -> i64 {
    rt.track_key(k);
    f.borrow()[&k]
}

/// The embedder's side: keyed sources, external effects and bindings run
/// through the flush's function, in dependency order — an effect made
/// before the binding it reads still runs once, after it.
#[test]
fn external_bindings() {
    let rt = Runtime::new();
    let fields: Fields = Default::default();
    let (x, d): (u128, u128) = (1, 2);
    fields.borrow_mut().insert(x, 1);
    fields.borrow_mut().insert(d, 0);
    let out = log();
    // tag 1: an effect printing x and d; tag 2: a binding d = x * 2
    let mut exec = {
        let (rt, fields, out) = (rt.clone(), fields.clone(), out.clone());
        move |_: NodeId, tag: u64| -> Result<(), ()> {
            match tag {
                1 => {
                    let (xv, dv) = (read(&rt, &fields, x), read(&rt, &fields, d));
                    out.borrow_mut().push(format!("x={xv} d={dv}"));
                }
                2 => {
                    let v = read(&rt, &fields, x) * 2;
                    let old = fields.borrow_mut().insert(d, v);
                    if old != Some(v) {
                        rt.write_key(d);
                    }
                }
                _ => unreachable!(),
            }
            Ok(())
        }
    };
    let e = rt.external(1, None);
    rt.run_now(e, &mut exec).unwrap();
    let bind = rt.external(2, Some(d));
    rt.run_now(bind, &mut exec).unwrap();
    rt.flush_with(&mut exec).unwrap();
    fields.borrow_mut().insert(x, 5);
    assert!(rt.write_key(x));
    rt.flush_with(&mut exec).unwrap();
    assert_eq!(*out.borrow(), ["x=1 d=0", "x=1 d=2", "x=5 d=10"]);
    assert_eq!(rt.tag(e), Some(1));
    assert_eq!(rt.stats(), (2, 2));
}

#[test]
fn external_errors_and_cycles() {
    let rt = Runtime::new();
    let fields: Fields = Default::default();
    fields.borrow_mut().insert(1, 0);
    let mut failing = |_: NodeId, _: u64| -> Result<(), &str> { Err("boom") };
    let f = rt.external(3, None);
    assert_eq!(rt.run_now(f, &mut failing), Err("boom"));
    let looping = rt.external(4, Some(1));
    let mut feed = {
        let (rt, fields) = (rt.clone(), fields.clone());
        move |_: NodeId, _: u64| -> Result<(), ()> {
            let v = read(&rt, &fields, 1) + 1;
            fields.borrow_mut().insert(1, v);
            rt.write_key(1);
            Ok(())
        }
    };
    rt.run_now(looping, &mut feed).unwrap();
    assert!(matches!(rt.flush_with(&mut feed), Err(FlushError::Cycle(_))));
}
