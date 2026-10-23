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
        def err
          yield
          :ok
        rescue NoMethodError => e
          e.message
        end
        class A
          def pub = "#{priv} #{self.priv} #{prot}"
          def priv = "priv"
          def prot = "prot"
          def peer(o) = o.prot
          def poke(o) = o.priv
          def x=(v) = (@x = v)
          def set = (self.x = 5; @x)
          private :priv, :x=
          protected :prot
        end
        class B < A
          public :priv
          def call_peer(o) = o.prot
        end
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
        class A; def m = :a; end
        class B < A; private :m; end
        p A.new.m, (begin; B.new.m; rescue NoMethodError => e; e.message; end)
        p (begin; A.send(:private, :nope); rescue NameError => e; e.message; end)
        class A; def m = :redefined; end
        p A.new.m
        class C; def m = 1; private :m; def m = 2; end
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
        def err
          yield
        rescue NoMethodError => e
          e.message
        end
        class A
          def a = :a
          private
          def b = :b
          protected
          def c = :c
          public
          def d = "#{b} #{c}"
          def initialize = nil
          private
          def self.cm = :cm
        end
        x = A.new
        p x.a, x.d, err { x.b }, err { x.c }, err { x.initialize }, A.cm
        class A; def e = :e; end
        p x.e
        module M
          module_function
          def f = :f
        end
        class B; include M; def g = f; end
        p M.f, B.new.g, err { B.new.f }
        class C
          private
          class D; def inner = :inner; end
          def m
            def nested = :nested
            :m
          end
        end
        p C::D.new.inner, C.new.send(:m), C.new.nested
    "##);
    assert_eq!(
        out,
        ":a\n\"b c\"\n\"private method 'b' called for an instance of A\"\n\
         \"protected method 'c' called for an instance of A\"\n\
         \"private method 'initialize' called for an instance of A\"\n:cm\n:e\n:f\n:f\n\
         \"private method 'f' called for an instance of B\"\n:inner\n:m\n:nested\n"
    );
}

#[test]
fn natives_that_define_methods_follow_the_scope() {
    // attr_* and define_method called in a class body (or a block in it,
    // or a class_eval block) under `private` define private methods; called
    // on another class or from a method they define public ones
    let out = run(r##"
        def err
          yield
        rescue NoMethodError
          :private
        end
        other = Class.new
        c = Class.new do
          private
          attr_accessor :a
          [1].each { define_method(:d) { :d } }
          other.attr_reader :pub
          def self.make = attr_reader(:from_cm)
        end
        c.make
        p err { c.new.a }, err { c.new.d }, other.new.pub, c.new.from_cm
        e = Class.new
        e.class_eval { private; attr_reader :r; define_method(:m) { :m } }
        p err { e.new.r }, err { e.new.m }, e.new.send(:m)
        mod = Module.new do
          module_function
          attr_reader :mf
          define_method(:dm) { :dm }
        end
        k = Class.new { include mod; def call = [mf, dm] }
        p mod.respond_to?(:mf), mod.dm, k.new.call, err { k.new.dm }
        m2 = Module.new do
          def f = :f
          module_function :f
        end
        p m2.f, err { Class.new { include m2 }.new.f }
        p (begin; Module.new { module_function :nope }; rescue NameError => e; e.message.start_with?("undefined method 'nope' for module"); end)
    "##);
    assert_eq!(
        out,
        ":private\n:private\nnil\nnil\n:private\n:private\n:m\nfalse\n:dm\n[nil, :dm]\n\
         :private\n:f\n:private\ntrue\n"
    );
}

#[test]
fn aliases_and_builtin_private_methods() {
    let out = run(r##"
        def err
          yield
        rescue NoMethodError
          :private
        end
        class A
          private def priv = :priv
          alias_method :priv2, :priv
          alias priv3 priv
          def pub = :pub
          alias pub2 pub
        end
        a = A.new
        p err { a.priv2 }, err { a.priv3 }, a.pub2, a.send(:priv2)
        p err { Object.new.initialize }, err { Object.new.respond_to_missing?(:x, false) }
    "##);
    assert_eq!(out, ":private\n:private\n:pub\n:priv\n:private\n:private\n");
}

#[test]
fn respond_to_and_public_send() {
    let out = run(r##"
        class R
          def pub = :pub
          private def priv = :priv
          protected def prot = :prot
          def respond_to_missing?(name, all = false) = (@asked = [name, all]; name == :ghost)
          def asked = @asked
        end
        r = R.new
        p r.respond_to?(:pub), r.respond_to?(:priv), r.respond_to?(:priv, true)
        p r.respond_to?(:prot), r.respond_to?(:prot, true), r.respond_to?(:initialize)
        p r.respond_to?(:ghost), r.asked, r.respond_to?(:nope, true), r.asked
        p r.public_send(:pub), (begin; r.public_send(:priv); rescue NoMethodError => e; e.message; end)
        p (begin; r.respond_to?; rescue ArgumentError; :argc; end)
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
        module M
          def mpub = 1
          private def mpriv = 1
        end
        class A
          include M
          def pub = 1
          private def priv = 1
          protected def prot = 1
        end
        class B < A
          public :priv
          undef_method :pub
        end
        p A.instance_methods(false), A.public_instance_methods(false)
        p A.private_instance_methods(false), A.protected_instance_methods(false)
        p A.method_defined?(:mpub), A.method_defined?(:mpriv), A.method_defined?(:prot)
        p A.method_defined?(:mpub, false), A.private_method_defined?(:mpriv)
        p B.method_defined?(:priv, false), B.method_defined?(:pub), A.method_defined?(:priv)
        o = A.new
        def o.single = 1
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
        a = Class.new { -> { private }.call; def m = 1 }
        b = Class.new { private; [1].each { [2].each { def n = 1 } }; def o = 1 }
        c = Class.new { private; class_eval { def q = 1; [1].each { def r = 1 } }; def s = 1 }
        d = Class.new { class_eval { private }; def t = 1 }
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
        c = Class.new do
          blk = proc { attr_reader :late }
          private
          blk.call
          [1].each { def in_block = 1 }
        end
        p c.private_method_defined?(:late), c.private_method_defined?(:in_block)
        d = Class.new { [1].each { define_method(:pub) { 1 } } }
        p d.public_method_defined?(:pub)
    "##);
    assert_eq!(out, "true\ntrue\ntrue\n");
}
