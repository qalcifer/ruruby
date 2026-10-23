//! Values of the built-in types carrying more than their contents (roadmap
//! step 5, part E): instance variables, and a class (a subclass of Array,
//! Hash or String).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn instance_variables_on_builtin_values() {
    let out = run(r##"
        GC.stress = true
        a = [1]; s = "x"; h = {}
        a.instance_variable_set(:@n, [1, 2])
        s.instance_variable_set(:@t, "tag")
        h.instance_variable_set(:@h, {k: 1})
        GC.start
        p a.instance_variable_get(:@n), s.instance_variable_get(:@t), h.instance_variables
        p a.instance_variable_defined?(:@n), a.remove_instance_variable(:@n), a.instance_variables
        GC.stress = false
        3000.times { |i| x = [i]; x.instance_variable_set(:@i, i) }
        GC.start
        fresh = Array.new(3000) { [] }
        p fresh.all? { |x| x.instance_variables.empty? }, 1.instance_variables
    "##);
    assert_eq!(out, "[1, 2]\n\"tag\"\n[:@h]\ntrue\n[1, 2]\n[]\ntrue\n[]\n");
}

#[test]
fn instances_of_builtin_subclasses() {
    let out = run(r##"
        class SubArray < Array; end
        a = SubArray[1, 2, 3]
        p a.class, a, a.is_a?(Array), a.size, a.map { _1 * 2 }.class
        class Entries < Array
          self.class.alias_method :[], :new
          def initialize(entries) = replace(entries)
          def keys = map { |k, v| k }
          def to_s = "#{self.class}#{super}"
        end
        e = Entries[[1, :a], [2, :b]]
        p e.class, e.keys, e.to_s
        h = Class.new(Hash) { attr_accessor :foo }.new(0)
        h.foo = 5
        h[:x] = 1
        p h.foo, h[:x], h[:nope], h.class.superclass
        s = Class.new(String) { def shout = upcase + "!" }.new("hi")
        p s.shout, s.class.superclass, (s + "x").class
        c = Class.new(Array) { def initialize(n) = super(n) { |i| i * i } }
        p c.new(4), Class.new(Hash)[a: 1].class.superclass
    "##);
    assert_eq!(
        out,
        "SubArray\n[1, 2, 3]\ntrue\n3\nArray\nEntries\n[1, 2]\n\"Entries[[1, :a], [2, :b]]\"\n\
         5\n1\n0\nHash\n\"HI!\"\nString\nString\n[0, 1, 4, 9]\nHash\n"
    );
}

#[test]
fn new_and_brackets_on_the_builtin_classes() {
    let out = run(r##"
        p Array.new(3, 0), Array.new([1, 2]), Array.new(2) { _1 + 1 }, Array[1, :b]
        p Hash.new(5)[:z], Hash[1, 2, 3, 4], Hash[[[1, 2], [3]]], Hash[{a: 1}], String.new("q")
        p (begin; Hash[1, 2, 3]; rescue ArgumentError => e; e.message; end)
        p (begin; Array.new(-1); rescue ArgumentError => e; e.message; end)
    "##);
    assert_eq!(
        out,
        "[0, 0, 0]\n[1, 2]\n[1, 2]\n[1, :b]\n5\n{1 => 2, 3 => 4}\n{1 => 2, 3 => nil}\n\
         {a: 1}\n\"q\"\n\"odd number of arguments for Hash\"\n\"negative array size\"\n"
    );
}

#[test]
fn copies_keep_class_and_ivars() {
    let out = run(r##"
        class E < Array; def hi = :hi; end
        e = E[3, 1, 2]
        e.instance_variable_set(:@t, 1)
        d = e.dup
        p d.class, d.hi, d.instance_variable_get(:@t), d.reverse!.class, d, e, e.clone.class
        c = Class.new(Hash) { attr_accessor :foo }
        h = c.new(0)
        h.foo = :iv
        h[:a] = 1
        m = h.merge({b: 2})
        p m.class == c, m.foo, m, m[:zz], {}.merge(h).class, h.clone.foo
        s = Class.new(String).new("x")
        p s.dup.class == s.class, [1].dup.class, "y".dup.class
    "##);
    assert_eq!(
        out,
        "E\n:hi\n1\nE\n[2, 1, 3]\n[3, 1, 2]\nE\ntrue\n:iv\n{a: 1, b: 2}\n0\nHash\n:iv\n\
         true\nArray\nString\n"
    );
}

#[test]
fn singleton_methods_on_builtin_values() {
    let out = run(r##"
        GC.stress = true
        s = "abc"
        def s.shout = upcase
        a = [1, 2]
        module Sum; def total = sum; end
        a.extend(Sum)
        h = {k: 1}
        class << h; def keyz = keys; end
        GC.start
        p s.shout, a.total, h.keyz, s.singleton_methods, s.class, a.is_a?(Sum)
        p "x".respond_to?(:shout), s.dup.respond_to?(:shout)
        p (begin; 1.singleton_class; rescue TypeError => e; e.message; end)
        p [0, 1, 2].map { |i| t = "t#{i}"; def t.m = self * 2; t.m }
    "##);
    assert_eq!(
        out,
        "\"ABC\"\n3\n[:k]\n[:shout]\nString\ntrue\nfalse\nfalse\n\
         \"can't define singleton for Integer\"\n[\"t0t0\", \"t1t1\", \"t2t2\"]\n"
    );
}
