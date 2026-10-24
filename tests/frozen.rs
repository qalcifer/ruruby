//! Frozen objects (roadmap step 5, part F): `freeze`, `frozen?` and
//! FrozenError from every change.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const FZ: &str = r##"
    fn fz() {
      r = catch_panic { yield; :changed }
      if r.ok? { r.ok } else { r.err.message }
    }
"##;

#[test]
fn frozen_values() {
    let out = run(&(FZ.to_owned()
        + r##"
        p 1.frozen?, :a.frozen?, none.frozen?, 1.5.frozen?, (1..2).frozen?, "x".frozen?, proc {}.frozen?
        a = [1, 2].freeze
        p fz { a << 3 }, fz { a.pop }, fz { a[0] = 9 }, fz { a.map! { _1 } }, fz { a.sort! }, fz { a.clear }
        s = "s".freeze
        p fz { s << "x" }, fz { s.replace("y") }, s + "t"
        h = {a: 1}.freeze
        p fz { h[:b] = 2 }, fz { h.delete(:a) }, fz { h.clear }, fz { h.default = 0 }, h.merge(b: 2)
        p a.dup.frozen?, a.clone.frozen?, a.clone(freeze: false).frozen?, a.dup << 3
    "##));
    assert_eq!(
        out,
        "true\ntrue\ntrue\ntrue\ntrue\nfalse\nfalse\n\
         \"can't modify frozen Array: [1, 2]\"\n\"can't modify frozen Array: [1, 2]\"\n\
         \"can't modify frozen Array: [1, 2]\"\n\"can't modify frozen Array: [1, 2]\"\n\
         \"can't modify frozen Array: [1, 2]\"\n\"can't modify frozen Array: [1, 2]\"\n\
         \"can't modify frozen String: \\\"s\\\"\"\n\"can't modify frozen String: \\\"s\\\"\"\n\"st\"\n\
         \"can't modify frozen Hash: {a: 1}\"\n\"can't modify frozen Hash: {a: 1}\"\n\
         \"can't modify frozen Hash: {a: 1}\"\n\"can't modify frozen Hash: {a: 1}\"\n{a: 1, b: 2}\n\
         false\ntrue\nfalse\n[1, 2, 3]\n"
    );
}

#[test]
fn frozen_objects_and_classes() {
    let out = run(&(FZ.to_owned()
        + r##"
        class P { attr_accessor :a; fn set = (@b = 1) }
        o = P.new.freeze
        p fz { o.a = 1 }, fz { o.set }, fz { o.instance_variable_set(:@c, 1) }, fz { fn o.s = 1 }
        p fz { o.extend(Comparable) }, (class << o { self }).frozen?
        c = Class.new.freeze
        p fz { c.class_eval { fn x = 1 } }.start_with?("can't modify frozen class"), fz { c.define_method(:y) { 1 } } != :changed
        class F { @@v = 1; freeze }
        class G < F { fn set = (@@v = 2) }
        p fz { class F { fn z = 1 } }, fz { class F { K = 1 } }, fz { F.attr_reader :q }, fz { G.new.set }
        m = Module.new.freeze
        p fz { m.module_eval { fn w = 1 } }.start_with?("can't modify frozen module")
        p defined?(1).frozen?, defined?(String).frozen?
    "##));
    assert_eq!(
        out,
        "\"can't modify frozen P: #<P>\"\n\"can't modify frozen P: #<P>\"\n\"can't modify frozen P: #<P>\"\n\
         \"can't modify frozen class: #<Class:#<P>>\"\n\"can't modify frozen P: #<P>\"\ntrue\ntrue\ntrue\n\
         \"can't modify frozen class: F\"\n\"can't modify frozen class: F\"\n\"can't modify frozen class: F\"\n\
         \"can't modify frozen class: F\"\ntrue\ntrue\ntrue\n"
    );
}

#[test]
fn hash_string_keys_are_frozen_copies() {
    let out = run(r##"
        h = {}
        k = "key"
        h[k] = 1
        f = "f".freeze
        h[f] = 2
        p h.keys[0].frozen?, k.frozen?, h.keys[0].equal?(k), h.keys[1].equal?(f)
        GC.stress = true
        x = {}
        30.times { |i| x["k#{i}"] = i }
        GC.start
        p x.keys.all?(&:frozen?), x["k7"], "k7".frozen?
    "##);
    assert_eq!(out, "true\nfalse\nfalse\ntrue\ntrue\n7\nfalse\n");
}
