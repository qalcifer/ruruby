//! Method visibility: private/protected/public (roadmap step 5).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn private_and_protected_with_names() {
    let out = run(r##"
        fn err() {
          r = catch_panic { yield; :ok }
          if r.ok? { r.ok } else { r.err.message }
        }
        class A {
          fn pub = "#{priv} #{self.priv} #{prot}"
          fn priv = "priv"
          fn prot = "prot"
          fn peer(o) = o.prot
          fn poke(o) = o.priv
          fn x=(v) = (@x = v)
          fn set = (self.x = 5; @x)
          private :priv, :x=
          protected :prot
        }
        class B < A {
          public :priv
          fn call_peer(o) = o.prot
        }
        a = A.new
        p a.pub, err { a.priv }, err { a.prot }, a.peer(A.new), err { a.poke(A.new) }
        p B.new.priv, err { A.new.priv }, B.new.call_peer(A.new), a.set, err { a.x = 1 }
        p a.send(:priv), err { a.priv(*[]) }
        p A.send(:private, :pub), err { a.pub }
    "##);
    assert_eq!(
        out,
        "\"priv priv prot\"\n\"private method 'priv' called for an instance of A\"\n\
         \"protected method 'prot' called for an instance of A\"\n\"prot\"\n\
         \"private method 'priv' called for an instance of A\"\n\"priv\"\n\
         \"private method 'priv' called for an instance of A\"\n\"prot\"\n5\n\
         \"private method 'x=' called for an instance of A\"\n\"priv\"\n\
         \"private method 'priv' called for an instance of A\"\n:pub\n\
         \"private method 'pub' called for an instance of A\"\n"
    );
}

#[test]
fn visibility_errors_and_inherited_methods() {
    let out = run(r##"
        class A { fn m = :a }
        class B < A { private :m }
        p A.new.m, catch_panic { B.new.m }.err.message
        p catch_panic { A.send(:private, :nope) }.err.message
        class A { fn m = :redefined }
        p A.new.m
        class C { fn m = 1; private :m; fn m = 2 }
        p C.new.m
    "##);
    assert_eq!(
        out,
        ":a\n\"private method 'm' called for an instance of B\"\n\
         \"undefined method 'nope' for class 'A'\"\n:redefined\n2\n"
    );
}

#[test]
fn scope_default_visibility() {
    let out = run(r##"
        fn err() {
          r = catch_panic { yield }
          if r.ok? { r.ok } else { r.err.message }
        }
        class A {
          fn a = :a
          private
          fn b = :b
          protected
          fn c = :c
          public
          fn d = "#{b} #{c}"
          fn initialize = none
          private
          fn self.cm = :cm
        }
        x = A.new
        p x.a, x.d, err { x.b }, err { x.c }, err { x.initialize }, A.cm
        class A { fn e = :e }
        p x.e
        module M {
          module_function
          fn f = :f
        }
        p M.f, M.private_instance_methods(false), M.public_method_defined?(:f)
        class C {
          private
          class D { fn inner = :inner }
          fn m() {
            fn nested = :nested
            :m
          }
        }
        p C::D.new.inner, C.new.send(:m), C.new.nested
    "##);
    assert_eq!(
        out,
        ":a\n\"b c\"\n\"private method 'b' called for an instance of A\"\n\
         \"protected method 'c' called for an instance of A\"\n\
         \"private method 'initialize' called for an instance of A\"\n:cm\n:e\n:f\n[:f]\nfalse\n:inner\n:m\n:nested\n"
    );
}

#[test]
fn natives_that_define_methods_follow_the_scope() {
    // attr_* and define_method called in a class body (or a block in it,
    // or a class_eval block) under `private` define private methods; called
    // on another class or from a method they define public ones
    let out = run(r##"
        fn err = catch_panic { yield }.unwrap_or(:private)
        other = Class.new
        c = Class.new {
          private
          attr_accessor :a
          [1].each { define_method(:d) { :d } }
          other.attr_reader :pub
          fn self.make = attr_reader(:from_cm)
        }
        c.make
        p err { c.new.a }, err { c.new.d }, other.new.pub, c.new.from_cm
        e = Class.new
        e.class_eval { private; attr_reader :r; define_method(:m) { :m } }
        p err { e.new.r }, err { e.new.m }, e.new.send(:m)
        mod = Module.new {
          module_function
          attr_reader :mf
          define_method(:dm) { :dm }
        }
        p mod.respond_to?(:mf), mod.dm, mod.private_instance_methods(false).sort
        m2 = Module.new {
          fn f = :f
          module_function :f
        }
        p m2.f, m2.private_instance_methods(false)
        p catch_panic { Module.new { module_function :nope } }.err.message.start_with?("undefined method 'nope' for module")
    "##);
    assert_eq!(
        out,
        ":private\n:private\nnone\nnone\n:private\n:private\n:m\nfalse\n:dm\n[:dm, :mf]\n:f\n[:f]\ntrue\n"
    );
}

#[test]
fn aliases_and_builtin_private_methods() {
    let out = run(r##"
        fn err = catch_panic { yield }.unwrap_or(:private)
        class A {
          private fn priv = :priv
          alias_method :priv2, :priv
          alias priv3 priv
          fn pub = :pub
          alias pub2 pub
        }
        a = A.new
        p err { a.priv2 }, err { a.priv3 }, a.pub2, a.send(:priv2)
        p err { Object.new.initialize }, err { Object.new.respond_to_missing?(:x, false) }
    "##);
    assert_eq!(out, ":private\n:private\n:pub\n:priv\n:private\n:private\n");
}

#[test]
fn respond_to_and_public_send() {
    let out = run(r##"
        class R {
          fn pub = :pub
          private fn priv = :priv
          protected fn prot = :prot
          fn respond_to_missing?(name, all = false) = (@asked = [name, all]; name == :ghost)
          fn asked = @asked
        }
        r = R.new
        p r.respond_to?(:pub), r.respond_to?(:priv), r.respond_to?(:priv, true)
        p r.respond_to?(:prot), r.respond_to?(:prot, true), r.respond_to?(:initialize)
        p r.respond_to?(:ghost), r.asked, r.respond_to?(:nope, true), r.asked
        p r.public_send(:pub), catch_panic { r.public_send(:priv) }.err.message
        p (catch_panic { r.respond_to? }.err && :argc)
    "##);
    assert_eq!(
        out,
        "true\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\n[:ghost, false]\nfalse\n[:nope, true]\n\
         :pub\n\"private method 'priv' called for an instance of R\"\n:argc\n"
    );
}

#[test]
fn listings_by_visibility() {
    let out = run(r##"
        class M {
          fn mpub = 1
          private fn mpriv = 1
        }
        class A < M {
          fn pub = 1
          private fn priv = 1
          protected fn prot = 1
        }
        class B < A {
          public :priv
          undef_method :pub
        }
        p A.instance_methods(false), A.public_instance_methods(false)
        p A.private_instance_methods(false), A.protected_instance_methods(false)
        p A.method_defined?(:mpub), A.method_defined?(:mpriv), A.method_defined?(:prot)
        p A.method_defined?(:mpub, false), A.private_method_defined?(:mpriv)
        p B.method_defined?(:priv, false), B.method_defined?(:pub), A.method_defined?(:priv)
        o = A.new
        fn o.single = 1
        p o.singleton_methods, o.methods.include?(:priv), o.private_methods.include?(:priv)
        p o.protected_methods.include?(:prot), o.public_methods.include?(:mpub)
        p B.instance_methods.include?(:pub), B.public_instance_methods(false)
    "##);
    assert_eq!(
        out,
        "[:prot, :pub]\n[:pub]\n[:priv]\n[:prot]\ntrue\nfalse\ntrue\nfalse\ntrue\n\
         true\nfalse\nfalse\n[:single]\nfalse\ntrue\ntrue\ntrue\nfalse\n[:priv]\n"
    );
}

#[test]
fn blocks_share_the_scope_visibility() {
    // a `private` in a block reaches the scope it was written in; blocks
    // run by class_eval & co. are scopes of their own, starting public
    let out = run(r##"
        a = Class.new { (|| { private }).call; fn m = 1 }
        b = Class.new { private; [1].each { [2].each { fn n = 1 } }; fn o = 1 }
        c = Class.new { private; class_eval { fn q = 1; [1].each { fn r = 1 } }; fn s = 1 }
        d = Class.new { class_eval { private }; fn t = 1 }
        p a.private_method_defined?(:m), b.private_instance_methods(false).sort
        p c.public_instance_methods(false).sort, c.private_method_defined?(:s)
        p d.public_method_defined?(:t)
    "##);
    assert_eq!(out, "true\n[:n, :o]\n[:q, :r]\ntrue\ntrue\n");
}

#[test]
fn blocks_without_upvars_still_share_visibility() {
    // a block that reads no outer local skips the frame's env, unless the
    // frame's code sets a default visibility
    let out = run(r##"
        c = Class.new {
          blk = proc { attr_reader :late }
          private
          blk.call
          [1].each { fn in_block = 1 }
        }
        p c.private_method_defined?(:late), c.private_method_defined?(:in_block)
        d = Class.new { [1].each { define_method(:pub) { 1 } } }
        p d.public_method_defined?(:pub)
    "##);
    assert_eq!(out, "true\ntrue\ntrue\n");
}
