//! lang-fork: Result values (src/core/result.rb), `?`, `none`, panics.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn ok_and_err_values() {
    let out = run(r##"
        def half(n) = n.even? ? Ok(n / 2) : Err("#{n} is odd")
        p half(4), half(3), half(4).ok?, half(3).err?
        p half(4).unwrap, half(3).unwrap_or(0), half(3).unwrap_or_else { |e| e.size }
        p half(8).and_then { |x| half(x) }, half(6).and_then { |x| half(x) }, half(4).map { |x| x * 10 }, half(3).map_err(&:upcase)
        p half(4) == Ok(2), half(4).eql?(Ok(2)), { Ok(1) => :a }[Ok(1)], half(4).ok, half(3).err, half(4).err
        [half(4), half(5)].each do |r|
          case r
          in Ok(v) then puts "ok #{v}"
          in Err(e) then puts "err #{e}"
          end
        end
        p catch_panic { half(3).unwrap }.err.message
        p catch_panic { half(3).expect("need even") }.err.message
        p catch_panic { panic("boom") }.map_err { |e| [e.class, e.message] }
        p half(4).frozen?, Ok(1).is_a?(Result)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Ok(2)"#,
        r#"Err("3 is odd")"#,
        r#"true"#,
        r#"true"#,
        r#"2"#,
        r#"0"#,
        r#"8"#,
        r#"Ok(2)"#,
        r#"Err("3 is odd")"#,
        r#"Ok(20)"#,
        r#"Err("3 IS ODD")"#,
        r#"true"#,
        r#"true"#,
        r#":a"#,
        r#"2"#,
        r#""3 is odd""#,
        r#"none"#,
        r#"ok 2"#,
        r#"err 5 is odd"#,
        r#""called unwrap on Err(\"3 is odd\")""#,
        r#""need even: \"3 is odd\"""#,
        r#"Err([Panic, "boom"])"#,
        r#"true"#,
        r#"true"#,
    ]);
}

/// `expr?`: Err and none return from the method (from a block: from the
/// method around it), Ok(v) gives v. After a method name `?` belongs to
/// the name (`empty?`): `.first()?`.
#[test]
fn try_operator() {
    let out = run(r##"
        def half(n) = n.even? ? Ok(n / 2) : Err("#{n} is odd")
        def quarter(n)
          h = half(n)?
          Ok(half(h)?)
        end
        p quarter(8), quarter(6), quarter(5)
        def first_word(s)
          w = s.split(" ").first()?
          w.upcase
        end
        p first_word("hi there"), first_word("")
        def from_block(xs)
          xs.each { |x| half(x)? }
          :all_even
        end
        p from_block([2, 4]), from_block([2, 3, 4])
        def local_try(n)
          r = half(n)
          v = r?
          v + 100
        end
        p local_try(10), local_try(1)
        def chain(n) = Ok(half(n)?.to_s + "!")
        p chain(4), chain(3)
        x = 5
        p x > 3 ? :big : :small, [1].empty?, x.even? ? 1 : 2
        def plain(v) = Ok(v?)
        p plain(7), plain(nil)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Ok(2)"#,
        r#"Err("3 is odd")"#,
        r#"Err("5 is odd")"#,
        r#""HI""#,
        r#"none"#,
        r#":all_even"#,
        r#"Err("3 is odd")"#,
        r#"105"#,
        r#"Err("1 is odd")"#,
        r#"Ok("2!")"#,
        r#"Err("3 is odd")"#,
        r#":big"#,
        r#"false"#,
        r#"2"#,
        r#"Ok(7)"#,
        r#"none"#,
    ]);
}

/// `catch_panic` (Rust's catch_unwind): Ok(value), or Err(panic) for a
/// panic of any kind; break/next/return pass through it.
#[test]
fn catch_panic_boundary() {
    let out = run(r#"
        p catch_panic { 1 + 1 }, catch_panic { none.foo }.err.class, catch_panic { 1 / 0 }.err.message
        p [1, 2].map { |x| catch_panic { next x * 10 } }
        def f = catch_panic { return :returned }
        p f, catch_panic { [1].each { break :b } }
    "#);
    assert_eq!(out, "Ok(2)\nNoMethodError\n\"divided by 0\"\n[Ok(10), Ok(20)]\n:returned\nOk(:b)\n");
}

/// Lookups that can miss (Hash#fetch, Array#fetch) and conversions that can
/// fail (Integer(), Float()) give Ok/Err; a default or block can't miss.
#[test]
fn fallible_core_methods() {
    let out = run(r##"
        h = {a: 1, b: none}
        p h.fetch(:a), h.fetch(:b), h.fetch(:zz), h.fetch(:zz, 0), h.fetch(:zz) { |k| k.to_s }
        d = Hash.new(5)
        p d.fetch(:x), d[:x]
        a = [10, 20, 30]
        p a.fetch(0), a.fetch(-1), a.fetch(3), a.fetch(-4), a.fetch(9, :d), a.fetch(9) { |i| i * 2 }
        p Integer("42"), Integer(" 1_000 "), Integer("4x"), Integer(3.9), Integer(none)
        p Float("2.5"), Float("x"), Float(3)
        def port(cfg)
          raw = cfg.fetch(:port)?
          n = Integer(raw)?
          Ok(n + 1)
        end
        p port({port: "8080"}), port({}), port({port: "eighty"})
        p catch_panic { h.fetch(:zz) { |k| none.boom } }.err.class
        p h.fetch(:a).unwrap + 1, a.fetch(5).unwrap_or(0)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Ok(1)"#,
        r#"Ok(none)"#,
        r#"Err("key not found: :zz")"#,
        r#"Ok(0)"#,
        r#"Ok("zz")"#,
        r#"Err("key not found: :x")"#,
        r#"5"#,
        r#"Ok(10)"#,
        r#"Ok(30)"#,
        r#"Err("index 3 outside of array bounds: -3...3")"#,
        r#"Err("index -4 outside of array bounds: -3...3")"#,
        r#"Ok(:d)"#,
        r#"Ok(18)"#,
        r#"Ok(42)"#,
        r#"Ok(1000)"#,
        r#"Err("invalid value for Integer(): \"4x\"")"#,
        r#"Ok(3)"#,
        r#"Err("no implicit conversion of none into Integer")"#,
        r#"Ok(2.5)"#,
        r#"Err("invalid value for Float(): \"x\"")"#,
        r#"Ok(3.0)"#,
        r#"Ok(8081)"#,
        r#"Err("key not found: :port")"#,
        r#"Err("invalid value for Integer(): \"eighty\"")"#,
        r#"NoMethodError"#,
        r#"2"#,
        r#"0"#,
    ]);
}
