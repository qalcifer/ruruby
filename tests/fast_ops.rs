//! Instructions that answer built-in methods in place (`GetIdx`/`SetIdx`
//! for `[]`/`[]=`, `Shl` for `<<`, `Size` for `size`/`length`): the same
//! results as the calls, and the calls themselves for subclasses,
//! singletons, frozen receivers and redefinitions.

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

#[test]
fn set_idx_and_shl() {
    let out = run_capture(r##"
        a = [1, 2]
        a[0] = :x; a[2] = :y; a[-1] = :z
        a[5] = 5
        p a
        b = []
        b << 1 << 2
        p b, (1 << 4), ("s" << "t")
        h = {}
        h[:k] = 1
        s = "key"
        h[s] = 2
        s << "!"
        p h, h.keys[1].frozen?
        f = [1].freeze
        p (begin; f[0] = 2; rescue FrozenError => e; e.class; end)
        p (begin; f << 2; rescue FrozenError => e; e.class; end)
        p (begin; {}.freeze[:a] = 1; rescue FrozenError => e; e.class; end)
        class Sub < Array
          def []=(i, v) = super(i, v.to_s)
          def <<(v) = super(v * 2)
        end
        s2 = Sub.new
        s2[0] = 1
        s2 << 3
        p s2
        class Array
          alias_method :__orig_aset, :[]=
          alias_method :__orig_shl, :<<
          def []=(i, v) = nil
          def <<(v) = self
        end
        c = [0]
        c[0] = 1
        c << 2
        p c
        class Array
          alias_method :[]=, :__orig_aset
          alias_method :<<, :__orig_shl
        end
        c[0] = 1
        c << 2
        p c
        x = [1, 2, 3]
        x[1..] = [9]
        p x
    "##)
    .unwrap();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            r#"[:x, 2, :z, nil, nil, 5]"#,
            r#"[1, 2]"#,
            r#"16"#,
            r#""st""#,
            r#"{k: 1, "key" => 2}"#,
            r#"true"#,
            r#"FrozenError"#,
            r#"FrozenError"#,
            r#"FrozenError"#,
            r#"["1", 6]"#,
            r#"[0]"#,
            r#"[1, 2]"#,
            r#"[1, 9]"#,
        ]
    );
}

/// Interpolation writes Strings, numbers, Symbols, nil and booleans out
/// itself (like mruby's `mrb_obj_as_string`, so a redefined Integer#to_s
/// isn't used) and calls `to_s` on anything else; Hash literals freeze
/// their String keys instead of copying them.
#[test]
fn interpolation_and_string_keys() {
    let out = run_capture(r##"
        class Foo; def to_s = "foo!"; end
        class Bad; def to_s = 42; end
        x = 5
        p "a#{x}b#{nil}c#{true}#{1.5}#{:sym}#{"str"}#{Foo.new}#{[1, "x"]}#{{a: 1}}"
        p "#{x}", "#{x}#{x}", "lit", :"dyn#{x}", "#{1 << 60}", "#{2**70}", "#{-0.0}#{1e20}"
        s = "q"
        s = "#{s}#{s}!"
        p s, Bad.new.to_s.class
        p "#{Bad.new}".start_with?("#<Bad")
        class Integer; alias_method :__to_s, :to_s; def to_s(*) = "int"; end
        p "#{7}"
        class Integer; alias_method :to_s, :__to_s; end
        h = {"x" => 1, "y" => [2]}
        p h, h.keys.map(&:frozen?), "x".frozen?
        k = "z"
        h2 = {k => 1}
        k << "!"
        p h2, k
    "##)
    .unwrap();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            r#""a5bctrue1.5symstrfoo![1, \"x\"]{a: 1}""#,
            r#""5""#,
            r#""55""#,
            r#""lit""#,
            r#":dyn5"#,
            r#""1152921504606846976""#,
            r#""1.1805916207174113e+21""#,
            r#""-0.01.0e+20""#,
            r#""qq!""#,
            r#"Integer"#,
            r#"false"#,
            r#""7""#,
            r#"{"x" => 1, "y" => [2]}"#,
            r#"[true, true]"#,
            r#"false"#,
            r#"{"z" => 1}"#,
            r#""z!""#,
        ]
    );
}

/// GetIdx and Size read self and locals in place, and `while i < size`
/// fuses the compare: the receiver is still read before a key that
/// reassigns it, and the condition's call runs once per test.
#[test]
fn operands_in_place() {
    let out = run_capture(r##"
        a = [10, 20, 30]
        i = 1
        k = :b
        h = {b: 2}
        p a[i], h[k], a[i + 1], a[a.size - 1]
        x = [5, 6]
        p x[(x = [7, 8]; 1)], x
        class Counter
          attr_reader :calls
          def initialize = @calls = 0
          def size
            @calls += 1
            2.5
          end
        end
        c = Counter.new
        n = 0
        n += 1 while n < c.size
        p n, c.calls
        class Grow < Array
          def walk
            i = 0
            out = []
            while i < size
              out << self[i]
              push(i) if size < 4
              i += 1
            end
            out
          end
        end
        g = Grow.new
        g.push(:a)
        p g.walk
        s = "abc"
        j = 0
        j += 1 while j < s.length
        p j
        def m(arr) = arr.size
        p m([1, 2]), m({a: 1}), m("xyz")
    "##)
    .unwrap();
    assert_eq!(out.lines().collect::<Vec<_>>().join(" "), "20 2 30 30 6 [7, 8] 3 4 [:a, 0, 1, 2] 3 2 1 3");
}
