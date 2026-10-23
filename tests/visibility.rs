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
