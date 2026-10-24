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
        fn half(n) = if n.even? { Ok(n / 2) } else { Err("#{n} is odd") }
        p half(4), half(3), half(4).ok?, half(3).err?
        p half(4).unwrap, half(3).unwrap_or(0), half(3).unwrap_or_else { |e| e.size }
        p half(8).and_then { |x| half(x) }, half(6).and_then { |x| half(x) }, half(4).map { |x| x * 10 }, half(3).map_err(&:upcase)
        p half(4) == Ok(2), half(4).eql?(Ok(2)), { Ok(1) => :a }[Ok(1)], half(4).ok, half(3).err, half(4).err
        [half(4), half(5)].each { |r|
          case r {
          in Ok(v) { puts "ok #{v}" }
          in Err(e) { puts "err #{e}" }
          }
        }
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
        fn half(n) = if n.even? { Ok(n / 2) } else { Err("#{n} is odd") }
        fn quarter(n) {
          h = half(n)?
          Ok(half(h)?)
        }
        p quarter(8), quarter(6), quarter(5)
        fn first_word(s) {
          w = s.split(" ").first()?
          w.upcase
        }
        p first_word("hi there"), first_word("")
        fn from_block(xs) {
          xs.each { |x| half(x)? }
          :all_even
        }
        p from_block([2, 4]), from_block([2, 3, 4])
        fn local_try(n) {
          r = half(n)
          v = r?
          v + 100
        }
        p local_try(10), local_try(1)
        fn chain(n) = Ok(half(n)?.to_s + "!")
        p chain(4), chain(3)
        x = 5
        p(if x > 3 { :big } else { :small }, [1].empty?, if x.even? { 1 } else { 2 })
        fn plain(v) = Ok(v?)
        p plain(7), plain(none)
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
        fn f = catch_panic { return :returned }
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
        fn port(cfg) {
          raw = cfg.fetch(:port)?
          n = Integer(raw)?
          Ok(n + 1)
        }
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

/// Ok/Err holding none, other Results and collections; equality and hashing
/// by value; combinators; case/in with guards and nested patterns.
#[test]
fn results_nest_compare_and_combine() {
    let out = run(r##"
        # nested results and edge values
        p Ok(Ok(1)), Ok(Err(2)), Ok(none), Err(none), Ok([1, {a: Ok(2)}])
        p Ok(1) == Ok(1.0), Ok(1).eql?(Ok(1.0)), Ok(1) == Err(1), Ok(1) != Ok(2)
        p [Ok(1), Err(2), Ok(3)].select(&:ok?).map(&:unwrap), [Ok(1), Err(2)].map(&:err)
        p({Ok(1) => :a, Err(1) => :b}.size, Ok("x").hash == Ok("x").hash)
        p Ok(1).frozen?, Err("e").frozen?, Ok(1).is_a?(Result), Err(1).is_a?(Ok)
        # combinators
        p Ok(2).and_then { |x| Ok(x * 3) }.and_then { |x| Err("big #{x}") }.or_else { |e| Ok(e.size) }
        p Err("a").map { |x| x + 1 }, Ok(1).map_err { |e| e.upcase }, Err(5).unwrap_or_else { |e| e * 2 }
        p Ok(1).unwrap_or(0), Err(1).unwrap_or(0), Ok(3).expect("never"), Err("x").unwrap_err
        p catch_panic { Ok(1).unwrap_err }.err.message
        # case/in with guards and nested patterns
        fn show(r) {
          case r {
          in Ok(Integer => n) if n > 10 { "big #{n}" }
          in Ok(Integer => n) { "small #{n}" }
          in Ok([a, b]) { "pair #{a},#{b}" }
          in Err(String => msg) { "error #{msg}" }
          in Err(_) { "other error" }
          }
        }
        p show(Ok(50)), show(Ok(5)), show(Ok([1, 2])), show(Err("no")), show(Err(3))
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Ok(Ok(1))"#,
        r#"Ok(Err(2))"#,
        r#"Ok(none)"#,
        r#"Err(none)"#,
        r#"Ok([1, {a: Ok(2)}])"#,
        r#"true"#,
        r#"false"#,
        r#"false"#,
        r#"true"#,
        r#"[1, 3]"#,
        r#"[none, 2]"#,
        r#"2"#,
        r#"true"#,
        r#"true"#,
        r#"true"#,
        r#"true"#,
        r#"false"#,
        r#"Ok(5)"#,
        r#"Err("a")"#,
        r#"Ok(1)"#,
        r#"10"#,
        r#"1"#,
        r#"0"#,
        r#"3"#,
        r#""x""#,
        r#""called unwrap_err on Ok(1)""#,
        r#""big 50""#,
        r#""small 5""#,
        r#""pair 1,2""#,
        r#""error no""#,
        r#""other error""#,
    ]);
}

/// `?` on Ok(Ok(..)), Ok(none) (passes: Ok gives what it holds), none,
/// plain values (pass through), in blocks, chained after `)`; ternaries still parse.
#[test]
fn try_edge_cases() {
    let out = run(r##"
        p catch_panic { show(Ok("x")) }.err.class
        # ? edge cases
        fn q1 = Ok(Ok(1)?)
        fn q2 = Ok(Ok(none)?)
        fn q3(x) = Ok([x?, :after])
        fn q4(r) = (v = r?; Ok(v.to_s))
        p q1, q2, q3(5), q3(none), q4(Ok(7)), q4(Err(:bad)), q4(false)
        fn q5(list) = list.map { |r| r? }
        p q5([Ok(1), Ok(2)]), q5([Ok(1), Err(2), Ok(3)])
        fn q6(h) = Ok(h[:a]?.fetch(:b)?)
        p q6({a: {b: 1}}), q6({a: {}}), q6({})
        x = 1
        p(if x > 0 { :pos } else { :neg }, if x.positive? { 1 } else { 2 }, if [x].empty? { 1 } else { 2 })
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"NoMethodError"#,
        r#"Ok(1)"#,
        r#"Ok(none)"#,
        r#"Ok([5, :after])"#,
        r#"none"#,
        r#"Ok("7")"#,
        r#"Err(:bad)"#,
        r#"Ok("false")"#,
        r#"[1, 2]"#,
        r#"Err(2)"#,
        r#"Ok(1)"#,
        r#"Err("key not found: :b")"#,
        r#"none"#,
        r#":pos"#,
        r#"1"#,
        r#"2"#,
    ]);
}

/// panic's messages, nested catch_panic, deep and runaway recursion, core
/// errors as panics of their kind.
#[test]
fn panics_and_catch_panic() {
    let out = run(r##"
        p catch_panic { panic }.err.message, catch_panic { panic(42) }.err.message, catch_panic { panic("x") }.err.class.superclass
        p catch_panic { catch_panic { panic "inner" }.unwrap }.err.message
        p catch_panic { none }, catch_panic { [1, 2].map { |v| v * 2 } }
        fn deep(n) = if n == 0 { panic("bottom") } else { deep(n - 1) }
        p catch_panic { deep(500) }.err.message
        fn runaway(n) = runaway(n + 1)
        p catch_panic { runaway(0) }.err.class
        p catch_panic { 1 + "x" }.err.class, catch_panic { Integer.nope }.err.class, catch_panic { [].fetch(1).unwrap }.err.message
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""explicit panic""#,
        r#""42""#,
        r#"Exception"#,
        r#""called unwrap on Err(#<Panic: inner>)""#,
        r#"Ok(none)"#,
        r#"Ok([2, 4])"#,
        r#""bottom""#,
        r#"SystemStackError"#,
        r#"TypeError"#,
        r#"NoMethodError"#,
        r#""called unwrap on Err(\"index 1 outside of array bounds: 0...0\")""#,
    ]);
}

/// `?` with nothing to return to (a file's top level, a class body, a block
/// whose return would leave the top level) panics; lambdas and
/// define_method blocks return the Err.
#[test]
fn try_at_the_top_level_panics() {
    let err = |src: &str| run_capture(src).unwrap_err().to_string();
    assert_eq!(err("x = Err(\"bad\")?\np :after"), "`?` at the top level got Err(\"bad\") (Panic)");
    assert_eq!(err("[Ok(1), none].each { |r| r? }"), "`?` at the top level got none (Panic)");
    assert_eq!(err("class D { X = Err(1)? }"), "`?` at the top level got Err(1) (Panic)");
    assert_eq!(run("p Ok(3)?"), "3\n");
    let out = run(r#"
        l = |r| { Ok(r? + 1) }
        class C { define_method(:m) { |r| Ok(r? * 2) } }
        fn f = [1].each { |x| return Err(x)? }
        p l.(Ok(1)), l.(Err(:e)), C.new.m(Ok(2)), C.new.m(Err(0)), f
    "#);
    assert_eq!(out, "Ok(2)\nErr(:e)\nOk(4)\nErr(0)\nErr(1)\n");
}
