//! The garbage collector (roadmap step 4): mark-sweep at safe points of the
//! main loop. `GC.stress = true` collects before every instruction there,
//! so a value the collector failed to find as a root is freed right away
//! and the next use panics ("use of a freed object").

use ruruby::{Interpreter, run_capture};

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn cyclic_garbage_is_freed() {
    let out = run(r#"
        class Node { attr_accessor :other }
        fn make_proc = (x = proc { x })
        keep = []
        i = 0
        while i < 30_000 {
          a = Node::new; b = Node::new; a.other = b; b.other = a
          make_proc
          keep << [i] if i % 1000 == 0
          i += 1
        }
        GC.start
        none
        p GC.count > 0, GC.live < 5_000, keep.size, keep.last
    "#);
    assert_eq!(out, "true\ntrue\n30\n[29000]\n");
}

#[test]
fn every_kind_of_root_survives_stress() {
    let mut interp = Interpreter::new();
    interp.capture_output();
    let src = r#"
        GC.stress = true
        $global = ["global"]
        CONST = {k: "const"}
        class Holder {
          @class_ivar = "class ivar"
          @@cvar = ["cvar"]
          fn self.ivar = @class_ivar
          fn cvar = @@cvar
          define_method(:dm) { "define_method #{[1]}" }
        }
        obj = Object::new
        fn obj.single = "singleton method"
        counter = 0
        bump = || { counter += 1; "closure #{counter}" }
        h = Hash::new { |hash, k| hash[k] = "default #{k}" }
        fn deep(n) = if n == 0 { ["deep"] } else { [deep(n - 1)].flatten }
        fn ensured() {
          $log = "ensure ran"
          yield
        }
        r = [1, 2, 3].map { |x| "m#{x}" }
        e = catch_panic { panic "boom #{[1]}" }.err
        v = ensured { [1, 2].each { |x| break "broke #{x}" if x == 2 } }
        at_exit { puts "at_exit #{[1, 2].sum}" }
        p $global, CONST, Holder::ivar, Holder::new.cvar, Holder::new.dm, obj.single
        p bump.(), bump.(), h[:a], deep(30), r, e.message, v, $log
        p "a" * 3, [1, [2, [3]]].flatten, {a: [1]}.map { |k, v| [k, v] }, (1..4).to_a
        p GC.count > 10
    "#;
    interp.eval(src).unwrap_or_else(|e| panic!("{e}"));
    interp.run_at_exit().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        interp.take_output(),
        "[\"global\"]\n{k: \"const\"}\n\"class ivar\"\n[\"cvar\"]\n\"define_method [1]\"\n\
         \"singleton method\"\n\"closure 1\"\n\"closure 2\"\n\"default a\"\n[\"deep\"]\n\
         [\"m1\", \"m2\", \"m3\"]\n\"boom [1]\"\n\"broke 2\"\n\"ensure ran\"\n\"aaa\"\n\
         [1, 2, 3]\n[[:a, [1]]]\n[1, 2, 3, 4]\ntrue\nat_exit 3\n"
    );
}

#[test]
fn gc_module() {
    let out = run(r#"
        p GC.disable, GC.disable, GC.enable, GC.enable
        n = GC.count
        GC.start
        x = 1
        p GC.count == n + 1
        GC.disable
        GC.start
        x = 2
        p GC.count == n + 1
        GC.enable
        p GC.stress, (GC.stress = true), GC.stress
    "#);
    assert_eq!(out, "false\ntrue\ntrue\nfalse\ntrue\ntrue\nfalse\ntrue\ntrue\n");
}

#[test]
fn garbage_is_collected_while_iterators_run() {
    // times/each/map are Ruby code (prelude.rb), so their loops reach the
    // collector's safe points
    let out = run(r#"
        seen = 0
        30_000.times { |i| x = [i, "s#{i}"]; seen = GC.count if i == 29_999 }
        list = [1] * 3
        list.each { 20_000.times { |i| {i => [i]} } }
        p seen > 0, GC.count > seen, GC.live < 50_000
    "#);
    assert_eq!(out, "true\ntrue\ntrue\n");
}

#[test]
fn anonymous_classes_are_collected() {
    // classes live in the heap too; unreachable ones are freed, named and
    // referenced ones survive
    let out = run(r#"
        Kept = Class::new { fn hi = "kept" }
        held = [Class::new(StandardError) { fn hi = "held" }]
        i = 0
        while i < 20_000 {
          c = Class::new { fn x = 1 }
          c.new.x
          Module::new
          i += 1
        }
        GC.start
        none
        p GC.live < 20_000, Kept::new.hi, held[0].new.hi, held[0].ancestors.include?(StandardError)
        GC.stress = true
        k = Class::new(Kept) { fn hi = super + "!" }
        p k.new.hi, Kept::name, k.superclass == Kept
    "#);
    assert_eq!(out, "true\n\"kept\"\n\"held\"\ntrue\n\"kept!\"\n\"Kept\"\ntrue\n");
}
