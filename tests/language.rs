//! End-to-end tests: Ruby source in, printed output out.
//! (mruby keeps its equivalents in `test/t/*.rb`, run by `mrbtest`.)

use ruruby::{Error, run_capture};

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

fn run_err(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => panic!("expected an error, got output {out:?}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn literals_and_inspect() {
    assert_eq!(
        run(r##"p 1, -2, 3.5, 1e20, nil, true, :sym, "s\n""##),
        "1\n-2\n3.5\n1.0e+20\nnone\ntrue\n:sym\n\"s\\n\"\n"
    );
    assert_eq!(
        run(r##"p [1, [2, 3]], {a: 1, "b" => 2}, 1..3, 1...3"##),
        "[1, [2, 3]]\n{a: 1, \"b\" => 2}\n1..3\n1...3\n"
    );
    assert_eq!(run("puts [1, [2, nil]]"), "1\n2\n\n");
}

#[test]
fn arithmetic() {
    assert_eq!(
        run("p 7 / 2, -7 / 2, -7 % 3, 2 ** 10, 10.0 / 4, 1 + 2 * 3 - 4"),
        "3\n-4\n2\n1024\n2.5\n3\n"
    );
    assert_eq!(
        run("p(-2 ** 2, (-2) ** 2, 0.1 + 0.2)"),
        "-4\n4\n0.30000000000000004\n"
    );
    // no Bignum yet: overflow falls back to Float
    assert_eq!(
        run("p 9_223_372_036_854_775_807 + 1"),
        "9.223372036854776e+18\n"
    );
    assert_eq!(run_err("1 / 0"), "divided by 0 (ZeroDivisionError)");
}

#[test]
fn strings() {
    assert_eq!(run(r##"x = 5; puts "x=#{x} y=#{x * 2}""##), "x=5 y=10\n");
    assert_eq!(
        run("s = 'abc'; s << 'def'; p s, s.length, s.upcase, s[1], s[1, 2], s[-2..]"),
        "\"abcdef\"\n6\n\"ABCDEF\"\n\"b\"\n\"bc\"\n\"ef\"\n"
    );
    assert_eq!(
        run("p 'a,b,,c'.split(','), '  x  '.strip, 'ab' * 3, 'az'.succ"),
        "[\"a\", \"b\", \"\", \"c\"]\n\"x\"\n\"ababab\"\n\"ba\"\n"
    );
    assert_eq!(
        run("puts format('%05.1f|%-3s|%+d|%x', 3.14159, 'a', 5, 255)"),
        "003.1|a  |+5|ff\n"
    );
}

#[test]
fn control_flow() {
    let src = "
        i = 0
        r = while i < 10
          i += 1
          next if i.odd?
          break i * 100 if i == 6
        end
        p i, r
        p(if false then 1 elsif none then 2 else 3 end)
        p((1 unless true))
        x = 5
        y = case x
            when 0..3 then :low
            when 4, 5 then :mid
            else :high
            end
        p y
    ";
    assert_eq!(run(src), "6\n600\n3\nnone\n:mid\n");
}

#[test]
fn methods_and_default_args() {
    let src = r##"
        def greet(name, greeting = "Hello", punct = "!")
          "#{greeting}, #{name}#{punct}"
        end
        puts greet("A")
        puts greet("B", "Hi")
        puts greet "C", "Yo", "?"
    "##;
    assert_eq!(run(src), "Hello, A!\nHi, B!\nYo, C?\n");
    assert_eq!(
        run_err("def f(a); end; f"),
        "wrong number of arguments (given 0, expected 1) (ArgumentError)"
    );
}

#[test]
fn blocks_closures_break_and_return() {
    let src = r##"
        def each_twice(a)
          a.each { |x| yield x; yield x }
        end
        out = []
        each_twice([1, 2]) { |x| out << x }
        p out
        counter = 0
        inc = proc { counter += 1 }
        3.times { inc.call }
        p counter
        p [1, 2, 3, 4].each { |x| break x * 10 if x == 3 }
        p [1, 2, 3].map { |x| next 0 if x == 2; x }
        def find_first(a)
          a.each { |x| return x if x > 1 }
          none
        end
        p find_first([1, 5, 7])
        p({a: 1, b: 2}.map { |k, v| "#{k}=#{v}" })
        p ["x", "y"].map(&:upcase)
    "##;
    assert_eq!(
        run(src),
        "[1, 1, 2, 2]\n3\n30\n[1, 0, 3]\n5\n[\"a=1\", \"b=2\"]\n[\"X\", \"Y\"]\n"
    );
}

#[test]
fn classes_modules_super() {
    let src = r##"
        module Named
          def label
            "<#{name}>"
          end
        end
        class Base
          include Named
          attr_accessor :name
          def initialize(name)
            @name = name
          end
          def describe
            "Base #{name}"
          end
          def self.build(n)
            new(n)
          end
        end
        class Child < Base
          def describe
            super + " (child)"
          end
        end
        c = Child.build("x")
        puts c.describe
        puts c.label
        c.name = "y"
        p c.name, c.is_a?(Named), Child.ancestors
        p c
    "##;
    assert_eq!(
        run(src),
        "Base x (child)\n<x>\n\"y\"\ntrue\n[Child, Base, Named, Object, Kernel, BasicObject]\n#<Child @name=\"y\">\n"
    );
}

#[test]
fn user_defined_enumerable_and_comparable() {
    let src = r##"
        class Version
          include Comparable
          attr_reader :major, :minor
          def initialize(major, minor)
            @major = major
            @minor = minor
          end
          def <=>(o)
            [major, minor] <=> [o.major, o.minor]
          end
          def to_s
            "#{major}.#{minor}"
          end
        end
        class Shelf
          include Enumerable
          def initialize(items)
            @items = items
          end
          def each
            @items.each { |i| yield i }
          end
        end
        vs = [Version.new(1, 2), Version.new(0, 9), Version.new(1, 10)]
        puts vs.max, vs.min, vs.sort.join(" ")
        p Version.new(1, 0) == Version.new(1, 0)
        shelf = Shelf.new([3, 1, 2])
        p shelf.sort, shelf.map { |x| x * 2 }, shelf.include?(2), shelf.min_by { |x| -x }
    "##;
    assert_eq!(
        run(src),
        "1.10\n0.9\n0.9 1.2 1.10\ntrue\n[1, 2, 3]\n[6, 2, 4]\ntrue\n3\n"
    );
}

/// lang-fork: expected failures are Err values, bugs are panics.
#[test]
fn errors_are_results_and_panics() {
    let src = r##"
        def risky(n) = n > 1 ? Err("bad #{n}") : Ok(n)
        [1, 2].each do |n|
          case risky(n)
          in Ok(v) then puts v
          in Err(e) then puts "failed: #{e}"
          end
        end
        p Integer("zz")
        def m = catch_panic { yield }.unwrap_or(:saved)
        p(m { 1 / 0 })
    "##;
    assert_eq!(run(src), "1\nfailed: bad 2\nErr(\"invalid value for Integer(): \\\"zz\\\"\")\n:saved\n");
    assert_eq!(run_err("panic 'boom'"), "boom (Panic)");
    assert_eq!(
        run_err("nil.foo"),
        "undefined method 'foo' for none (NoMethodError)"
    );
    assert_eq!(run_err("Nope"), "uninitialized constant Nope (NameError)");
}

#[test]
fn hashes_and_enumerable() {
    let src = r##"
        h = Hash.new(0)
        "a b a c b a".split.each { |w| h[w] += 1 }
        p h, h.sort_by { |k, v| -v }.first, h.select { |k, v| v > 1 }.keys
        p (1..10).select(&:even?).sum, [3, 1, 2].sort.reverse, [1, 2, 3].inject { |a, b| a * b }
        p [1, 2, 3, 4].partition(&:odd?)
    "##;
    assert_eq!(
        run(src),
        "{\"a\" => 3, \"b\" => 2, \"c\" => 1}\n[\"a\", 3]\n[\"a\", \"b\"]\n30\n[3, 2, 1]\n6\n[[1, 3], [2, 4]]\n"
    );
}

#[test]
fn multiple_assignment() {
    assert_eq!(
        run("a, b = 1, 2; a, b = b, a; p a, b; c, d = [3, 4]; p c + d"),
        "2\n1\n7\n"
    );
}

#[test]
fn deep_recursion_is_an_exception() {
    // the VM recurses on the native stack, so give the test thread room (main.rs does the same)
    let out = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(|| run_capture("def f(n); f(n + 1); end; f(0)").map_err(|e| e.to_string()))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(out.unwrap_err(), "stack level too deep (SystemStackError)");
}

#[test]
fn syntax_error_is_reported() {
    assert!(matches!(run_capture("def f("), Err(Error::Syntax(_))));
}

/// Loops test their condition at the bottom too (codegen repeats it):
/// `next`, `break`, `until`, non-Integer operands and conditions with
/// side effects still behave.
#[test]
fn loop_conditions() {
    let out = run(r#"
        i = 0; out = []
        while i < 6
          i += 1
          next if i == 2
          break if i == 5
          out << i
        end
        p out, i
        j = 0.5
        j += 1 while j < 3
        p j
        k = 10
        until k <= 7
          k -= 1
        end
        p k
        calls = 0
        n = 0
        while (calls += 1; n < 3)
          n += 1
        end
        p calls
        x = 0
        r = while x < 3 do x += 1 end
        p r
        a, b = 0, 5
        a += 1 while a < b
        p a
        s = "aaa"
        s = s[1..] while s.size > 1
        p s
    "#);
    assert_eq!(out, "[1, 3, 4]\n5\n3.5\n7\n4\nnone\n5\n\"a\"\n");
}

/// `yield` to blocks taking exactly the arguments given starts their frame
/// directly; auto-splat, defaults, lambdas, keywords, block parameters,
/// break/next/return and upvars behave as through the general path.
#[test]
fn yield_paths() {
    let out = run(r#"
        def two; yield 1, 2; end
        def one; yield [3, 4]; end
        def no_args; yield; end
        p two { |a, b| a + b }, one { |a, b| [a, b] }, one { |a| a }, no_args { |x = 5| x }
        r = [1, 2, 3].each { |x| break x * 10 if x == 2 }
        p r
        def find; [1, 2, 3].each { |x| return x if x > 1 }; nil; end
        p find
        p [1, 2, 3].map { |x| next 0 if x == 2; x }
        def lam; yield 7; end
        p lam(&->(x) { x + 1 })
        p catch_panic { lam(&->(x, y) { x }) }.err.class
        def kw; yield a: 1; end
        p kw { |a:| a }, kw { |h| h }
        def blk; yield 1; end
        p blk { |x, &b| [x, b] }
        def deep(n) = n == 0 ? yield : deep(n - 1) { yield }
        p deep(50) { :bottom }
        x = 10
        [1].each { |v| x += v }
        p x
    "#);
    assert_eq!(out.lines().collect::<Vec<_>>().join(" "), "3 [3, 4] [3, 4] 5 20 2 [1, 0, 3] 8 ArgumentError 1 {a: 1} [1, none] :bottom 11");
}

/// Calls of Ruby methods that take just the arguments given start their
/// frame in send_op: blocks, break/return, private methods, arity errors,
/// `__method__` and the frame limit behave as through start_invoke.
#[test]
fn method_call_paths() {
    let out = run(r#"
        def add(a, b) = a + b
        def with_blk(x, &b) = b ? b.call(x) : :none
        def each2(a, b); yield a; yield b; :done; end
        def first_big(xs); xs.each { |x| return x if x > 2 }; nil; end
        def name = __method__
        def kw(h) = h
        class P
          def run = secret(2)
          private def secret(n) = n * 21
        end
        def fact(n) = n <= 1 ? 1 : n * fact(n - 1)
        p add(1, 2), with_blk(3) { |v| v * 2 }, with_blk(3), each2(1, 2) { |v| break v * 100 if v == 2 }
        p first_big([1, 5, 3]), name, kw(a: 1), P.new.run, fact(20)
        p catch_panic { P.new.secret(1) }.err.message[0, 22]
        p catch_panic { add(1) }.err.message
        def down(n) = n == 0 ? 0 : down(n - 1)
        p down(5000)
        p catch_panic { down(1_000_000) }.err.class
    "#);
    assert_eq!(out, "3\n6\n:none\n200\n5\n:name\n{a: 1}\n42\n2432902008176640000\n\"private method 'secret\"\n\"wrong number of arguments (given 1, expected 2)\"\n0\nSystemStackError\n");
}
