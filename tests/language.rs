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
        "1\n-2\n3.5\n1.0e+20\nnil\ntrue\n:sym\n\"s\\n\"\n"
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
        p(if false then 1 elsif nil then 2 else 3 end)
        p((1 unless true))
        x = 5
        y = case x
            when 0..3 then :low
            when 4, 5 then :mid
            else :high
            end
        p y
    ";
    assert_eq!(run(src), "6\n600\n3\nnil\n:mid\n");
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
          nil
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

#[test]
fn exceptions() {
    let src = r##"
        class AppError < StandardError; end
        def risky(n)
          raise AppError, "bad #{n}" if n > 1
          n
        end
        [1, 2].each do |n|
          begin
            puts risky(n)
          rescue AppError => e
            puts "rescued #{e.message} (#{e.class})"
          ensure
            puts "ensure #{n}"
          end
        end
        x = Integer("zz") rescue -1
        p x
        def m
          yield
        rescue ZeroDivisionError
          :saved
        end
        p(m { 1 / 0 })
    "##;
    assert_eq!(
        run(src),
        "1\nensure 1\nrescued bad 2 (AppError)\nensure 2\n-1\n:saved\n"
    );
    assert_eq!(run_err("raise 'boom'"), "boom (RuntimeError)");
    assert_eq!(
        run_err("nil.foo"),
        "undefined method 'foo' for nil (NoMethodError)"
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
    assert_eq!(out, "[1, 3, 4]\n5\n3.5\n7\n4\nnil\n5\n\"a\"\n");
}
