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
        p h[:a], h["s"], h[:zz], Hash::new(7)[:x], Hash::new { |hh, k| k.to_s }[:y]
        class Key {
          fn init(n) = @n = n
          fn hash = 1
          fn eql?(o) = true
        }
        p({Key::new(1) => :found}[Key::new(2)])
        class Sub < Array {
          fn [](i) = :sub
          fn size = :subsize
        }
        s = Sub::new
        p s[0], s.size
        b = [1, 2]
        fn b.[](i) = :singleton
        fn b.length = :len
        p b[0], b.length, [1, 2][0]
        p "héllo".size, [1, 2, 3].size, {a: 1}.length, (1..4).size
        o = Object::new
        fn o.size = 42
        p o.size
        class Array {
          alias_method :__orig_aref, :[]
          alias_method :__orig_size, :size
          fn [](*) = :redefined
          fn size = :resized
        }
        p [1][0], [1].size, {a: 1}[:a], {a: 1}.size
        class Array {
          alias_method :[], :__orig_aref
          alias_method :size, :__orig_size
        }
        p [1][0], [1].size
        class Hash {
          fn size = :hsize
        }
        p({a: 1}.size, [5][0])
        class C {
          fn go = size
          private fn size = :private_size
        }
        p C.new.go
        p catch_panic { none[0] }.err.name
    "##)
    .unwrap();
    assert_eq!(
        out.split('\n').collect::<Vec<_>>().join(" "),
        r#"10 30 none none 20 [10, 20] 1 2 none 7 "y" :found :sub :subsize :singleton :len 1 5 3 1 4 42 :redefined :resized 1 1 1 1 :hsize 5 :private_size :[] "#
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
        p catch_panic { f[0] = 2 }.err.class
        p catch_panic { f << 2 }.err.class
        p catch_panic { {}.freeze[:a] = 1 }.err.class
        class Sub < Array {
          fn []=(i, v) = super(i, v.to_s)
          fn <<(v) = super(v * 2)
        }
        s2 = Sub::new
        s2[0] = 1
        s2 << 3
        p s2
        class Array {
          alias_method :__orig_aset, :[]=
          alias_method :__orig_shl, :<<
          fn []=(i, v) = none
          fn <<(v) = self
        }
        c = [0]
        c[0] = 1
        c << 2
        p c
        class Array {
          alias_method :[]=, :__orig_aset
          alias_method :<<, :__orig_shl
        }
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
            r#"[:x, 2, :z, none, none, 5]"#,
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

/// Interpolation writes Strings, numbers, Symbols, none and booleans out
/// itself (like mruby's `mrb_obj_as_string`, so a redefined Integer#to_s
/// isn't used) and calls `to_s` on anything else; Hash literals freeze
/// their String keys instead of copying them.
#[test]
fn interpolation_and_string_keys() {
    let out = run_capture(r##"
        class Foo { fn to_s = "foo!" }
        class Bad { fn to_s = 42 }
        x = 5
        p "a#{x}b#{none}c#{true}#{1.5}#{:sym}#{"str"}#{Foo::new}#{[1, "x"]}#{{a: 1}}"
        p "#{x}", "#{x}#{x}", "lit", :"dyn#{x}", "#{1 << 60}", "#{2**70}", "#{-0.0}#{1e20}"
        s = "q"
        s = "#{s}#{s}!"
        p s, Bad::new.to_s.class
        p "#{Bad::new}".start_with?("#<Bad")
        class Integer { alias_method :__to_s, :to_s; fn to_s(*) = "int" }
        p "#{7}"
        class Integer { alias_method :to_s, :__to_s }
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
        class Counter {
          prop calls
          fn init = @calls = 0
          fn size() {
            @calls += 1
            2.5
          }
        }
        c = Counter::new
        n = 0
        n += 1 while n < c.size
        p n, c.calls
        class Grow < Array {
          fn walk() {
            i = 0
            out = []
            while i < size {
              out << self[i]
              push(i) if size < 4
              i += 1
            }
            out
          }
        }
        g = Grow::new
        g.push(:a)
        p g.walk
        s = "abc"
        j = 0
        j += 1 while j < s.length
        p j
        fn m(arr) = arr.size
        p m([1, 2]), m({a: 1}), m("xyz")
    "##)
    .unwrap();
    assert_eq!(out.lines().collect::<Vec<_>>().join(" "), "20 2 30 30 6 [7, 8] 3 4 [:a, 0, 1, 2] 3 2 1 3");
}
