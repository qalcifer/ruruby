//! Instructions that answer built-in methods in place (`GetIdx` for `[]`,
//! `Size` for `size`/`length`): the same results as the calls, and the
//! calls themselves for subclasses, singletons and redefinitions.

use ruruby::run_capture;

#[test]
fn get_idx_and_size() {
    let out = run_capture(r##"
        a = [10, 20, 30]
        p a[0], a[-1], a[3], a[-4], a[1.9], a[0..1]
        h = {a: 1, "s" => 2}
        p h[:a], h["s"], h[:zz], Hash.new(7)[:x], Hash.new { |hh, k| k.to_s }[:y]
        class Key
          def initialize(n) = @n = n
          def hash = 1
          def eql?(o) = true
        end
        p({Key.new(1) => :found}[Key.new(2)])
        class Sub < Array
          def [](i) = :sub
          def size = :subsize
        end
        s = Sub.new
        p s[0], s.size
        b = [1, 2]
        def b.[](i) = :singleton
        def b.length = :len
        p b[0], b.length, [1, 2][0]
        p "héllo".size, [1, 2, 3].size, {a: 1}.length, (1..4).size
        o = Object.new
        def o.size = 42
        p o.size
        class Array
          alias_method :__orig_aref, :[]
          alias_method :__orig_size, :size
          def [](*) = :redefined
          def size = :resized
        end
        p [1][0], [1].size, {a: 1}[:a], {a: 1}.size
        class Array
          alias_method :[], :__orig_aref
          alias_method :size, :__orig_size
        end
        p [1][0], [1].size
        class Hash
          def size = :hsize
        end
        p({a: 1}.size, [5][0])
        class C
          def go = size
          private def size = :private_size
        end
        p C.new.go
        p (begin; nil[0]; rescue NoMethodError => e; e.name; end)
    "##)
    .unwrap();
    assert_eq!(
        out.split('\n').collect::<Vec<_>>().join(" "),
        r#"10 30 nil nil 20 [10, 20] 1 2 nil 7 "y" :found :sub :subsize :singleton :len 1 5 3 1 4 42 :redefined :resized 1 1 1 1 :hsize 5 :private_size :[] "#
    );
}
