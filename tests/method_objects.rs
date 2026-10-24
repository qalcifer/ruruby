//! Method and UnboundMethod objects (roadmap step 5, part F).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn bound_methods() {
    let out = run(r##"
        class A
          def hi(x, y = 1) = "A#hi #{x} #{y}"
          def self.cm = :cm
          private def priv = :priv
        end
        a = A.new
        m = a.method(:hi)
        p m.call(1), m.(2, 3), m[4], m.arity, m.name, m.owner, m.receiver.equal?(a), m
        p [1, 2].map(&a.method(:hi)), m.to_proc.lambda?, [3, 1].map(&10.method(:+))
        p A.method(:cm).call, A.method(:cm), a.method(:priv).call, "x".method(:upcase).call
        p catch_panic { a.public_method(:priv) }.err.message
        p catch_panic { a.method(:nope) }.err.message
        p a.method(:hi) == a.method(:hi), a.method(:hi) == A.new.method(:hi), m.instance_variables
        GC.stress = true
        keep = a.method(:hi)
        GC.start
        p keep.call(:gc)
    "##);
    assert_eq!(
        out,
        "\"A#hi 1 1\"\n\"A#hi 2 3\"\n\"A#hi 4 1\"\n-2\n:hi\nA\ntrue\n#<Method: A#hi>\n\
         [\"A#hi 1 1\", \"A#hi 2 1\"]\ntrue\n[13, 11]\n:cm\n#<Method: A.cm>\n:priv\n\"X\"\n\
         \"private method 'priv' called for an instance of A\"\n\
         \"undefined method 'nope' for an instance of A\"\ntrue\nfalse\n[]\n\"A#hi gc 1\"\n"
    );
}

#[test]
fn unbound_methods() {
    let out = run(r##"
        class A; def hi(x) = "A#hi #{x}"; attr_accessor :v; end
        class B < A; def hi(x) = "B#hi #{x}"; end
        u = A.instance_method(:hi)
        p u, u.arity, u.owner, u.name, u.bind(B.new).call(1), u.bind_call(B.new, 2), B.new.hi(3)
        p A.instance_method(:v).arity, A.instance_method(:v=).arity, A.new.method(:hi).unbind
        p catch_panic { u.bind(1) }.err.message
        p catch_panic { A.instance_method(:nope) }.err.message
        class A; private def secret = 1; end
        p catch_panic { A.public_instance_method(:secret) }.err.message
    "##);
    assert_eq!(
        out,
        "#<UnboundMethod: A#hi>\n1\nA\n:hi\n\"A#hi 1\"\n\"A#hi 2\"\n\"B#hi 3\"\n0\n1\n\
         #<UnboundMethod: A#hi>\n\"bind argument must be an instance of A\"\n\
         \"undefined method 'nope' for class 'A'\"\n\"method 'secret' for class 'A' is private\"\n"
    );
}
