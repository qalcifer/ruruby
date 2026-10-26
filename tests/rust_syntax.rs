//! lang-fork: Rust-like syntax — `fn` definitions, braces for every
//! construct, closures `|x| ...`.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

fn err(src: &str) -> String {
    run_capture(src).unwrap_err().to_string()
}

/// `fn name(params) { body }` and `fn name(params) = expr`: every kind of
/// parameter, singleton and operator methods, visibility prefixes.
#[test]
fn fn_definitions() {
    let out = run(r##"
        fn add(a, b) {
          a + b
        }
        fn square(x) = x * x
        fn greet(name = "you", *rest, loud: false, &blk) {
          s = "hi #{name}"
          s = s.upcase if loud
          if blk { blk.call(s) } else { [s, rest] }
        }
        fn none_args { :no_params }
        fn empty() {}
        class V {
          fn init(n) = @n = n
          fn self.make(n) { new(n) }
          fn ==(o) { o.is_a?(V) && o.n == n }
          fn n = @n
          fn [](i) { @n * i }
          fn -@ = V.new(-@n)
          private fn hidden() { :h }
          fn call_hidden = hidden
        }
        p add(1, 2), square(4), greet, greet("Ann", 1, 2), greet(loud: true) { |s| s + "!" }, none_args, empty
        p V.make(3) == V.new(3), V.new(2)[5], (-V.new(4)).n, V.new(1).call_hidden, V.new(1).respond_to?(:hidden)
        fn multi(x) {
          if x > 1 {
            :big
          } else {
            :small
          }
        }
        p multi(5), multi(0)
        p [1, 2].map { |x| x * 10 }
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"3"#,
        r#"16"#,
        r#"["hi you", []]"#,
        r#"["hi Ann", [1, 2]]"#,
        r#""HI YOU!""#,
        r#":no_params"#,
        r#"none"#,
        r#"true"#,
        r#"10"#,
        r#"-4"#,
        r#":h"#,
        r#"false"#,
        r#":big"#,
        r#":small"#,
        r#"[10, 20]"#,
    ]);
    for src in ["fn f x { x }", "fn f(x)", "fn f(x) do x end"] {
        assert!(err(src).contains("fn: expected `(parameters)`, then `{ body }` or `= expression`"), "{src}");
    }
}

/// Braces for every construct: if / else if / else, unless, while, until,
/// for, case/when and case/in (with guards), class (with a superclass, and
/// reopened), module, class << self. In a header (condition, subject,
/// superclass) a `{` starts the body; a block there needs parentheses.
#[test]
fn braces_for_every_construct() {
    let out = run(r##"
        fn sign(x) {
          if x > 0 {
            :pos
          } else if x < 0 {
            :neg
          } else {
            :zero
          }
        }
        p sign(3), sign(-1), sign(0)
        p(if true { 1 })
        p(if false { 1 })
        unless false { p :unless_ran } else { p :no }
        i = 0
        while i < 3 { i += 1 }
        until i == 0 { i -= 1 }
        p i
        out = []
        for x in [1, 2, 3] { out << x * 2 }
        p out, x
        fn kind(v) {
          case v {
            when 1, 2 { :small }
            when Integer { :int }
            else { :other }
          }
        }
        p kind(1), kind(10), kind("s")
        fn show(r) {
          case r {
            in Ok(n) if n > 10 { "big #{n}" }
            in Ok(n) { "ok #{n}" }
            in Err(e) { "err #{e}" }
          }
        }
        p show(Ok(50)), show(Ok(1)), show(Err(:x))
        class Animal {
          fn init(name) { @name = name }
          fn speak = "..."
          fn to_s { "#{self.class.name}(#{@name}): #{speak}" }
        }
        class Dog < Animal {
          fn speak = "woof"
        }
        module Greeting {
          fn self.hello(who) = "hello #{who}"
          VERSION = 1
        }
        class Dog {
          class << self { fn create(n) = new(n) }
        }
        puts Dog::create("rex"), Animal::new("a"), Greeting::hello("you"), Greeting::VERSION
        list = [1, 5, 9]
        if (list.any? { |v| v > 8 }) { p :parens }
        while list.size > 2 { list.pop }
        p list
        h = {a: 1}
        if h[:a] { p :hash_index }
        if h == {a: 1} { p :hash_literal }
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#":pos"#,
        r#":neg"#,
        r#":zero"#,
        r#"1"#,
        r#"none"#,
        r#":unless_ran"#,
        r#"0"#,
        r#"[2, 4, 6]"#,
        r#"3"#,
        r#":small"#,
        r#":int"#,
        r#":other"#,
        r#""big 50""#,
        r#""ok 1""#,
        r#""err x""#,
        r#"Dog(rex): woof"#,
        r#"Animal(a): ..."#,
        r#"hello you"#,
        r#"1"#,
        r#":parens"#,
        r#"[1, 5]"#,
        r#":hash_index"#,
        r#":hash_literal"#,
    ]);
    assert!(err("if [1].any? { |v| v > 0 } { p 1 }").contains("a block in a condition needs parentheses"));
}

/// Closures: `|a, b| expr`, `|x| { stmts }`, `|| expr` are lambdas (strict
/// arity, `return` leaves the closure). As a call's last argument a closure
/// literal is the call's block (`map(|x| ..)`, `if xs.any?(|x| ..) {`), so
/// iterators see it as they see `{ |x| }`; calling a closure (`f.(g, |x| ..)`)
/// passes it as a value. `|` stays bitwise or after an operand.
#[test]
fn closures() {
    let out = run(r##"
        add = |a, b| a + b
        inc = |x| x + 1
        hello = || "hi"
        multi = |x| {
          y = x * 2
          y + 1
        }
        p add.(1, 2), inc.call(4), hello.(), multi[5], add.lambda?, add.arity
        p [1, 2, 3].map(|x| x * 10), [1, 2, 3].select(|x| x.odd?), [3, 1, 2].sort(|a, b| b <=> a)
        p({a: 1, b: 2}.map(|k, v| "#{k}=#{v}"), [[1, 2], [3, 4]].map(|(a, b)| a + b))
        total = 0
        [1, 2, 3].each(|x| total += x)
        p total
        fn find_big(xs) {
          xs.each(|x| { return x if x > 1 })
          none
        }
        p find_big([1, 5, 9]), find_big([0])
        p [1, 2, 3].inject(0, |s, x| s + x), [1, 2].reduce(|a, b| a * b)
        if [1, 9].any?(|x| x > 8) { p :closure_in_condition }
        p((1..3).map(|x| x * x))
        compose = |f, g| |x| f.(g.(x))
        p compose.(inc, |x| x * 3).(2)
        counter = 0
        bump = || counter += 1
        bump.(); bump.()
        p counter
        early = |x| { return :neg if x < 0; :pos }
        p early.(-1), early.(1)
        p catch_panic { add.(1) }.err.class
        p 5 | 3, true || false, [1].map(|v| v | 2)
        fn run(x, &f) = f.(x)
        fn twice(x) = yield(yield(x))
        p run(1, |x| x + 1), twice(3, |x| x * 2)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"3"#,
        r#"5"#,
        r#""hi""#,
        r#"11"#,
        r#"true"#,
        r#"2"#,
        r#"[10, 20, 30]"#,
        r#"[1, 3]"#,
        r#"[3, 2, 1]"#,
        r#"["a=1", "b=2"]"#,
        r#"[3, 7]"#,
        r#"6"#,
        r#"5"#,
        r#"none"#,
        r#"6"#,
        r#"2"#,
        r#":closure_in_condition"#,
        r#"[1, 4, 9]"#,
        r#"7"#,
        r#"2"#,
        r#":neg"#,
        r#":pos"#,
        r#"ArgumentError"#,
        r#"7"#,
        r#"true"#,
        r#"[3]"#,
        r#"2"#,
        r#"12"#,
    ]);
}

/// A module is a namespace, like a Rust `mod`: its `fn`s are its own
/// functions (`M.f`), calling each other without a receiver; `private`
/// ones are internal; constants, nested modules and classes as before.
/// Interfaces keep instance methods.
#[test]
fn modules_hold_functions() {
    let out = run(r##"
        module Temp {
          SCALE = 1.8
          fn to_f(c) = round1(c * SCALE + 32)
          fn self.to_c(f) = round1((f - 32) / SCALE)   # same thing, spelled out
          private fn round1(x) = x.round(1)
          module Format { fn show(c) = "#{c}°C = #{Temp::to_f(c)}°F" }
          class Reading {
            fn init(c) = @c = c
            fn hot? = @c * SCALE > 50
          }
        }
        module Temp { fn freezing = to_f(0) }    # reopened
        p Temp::to_f(100), Temp::to_c(212.0), Temp::freezing, Temp::Format::show(10)
        p Temp::Reading::new(30).hot?, Temp::singleton_methods.sort, Temp::instance_methods
        p catch_panic { Temp::round1(1.25) }.err.message
        interface Hello { fn hi() = "hi" }
        p Hello::instance_methods, Hello::singleton_methods
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "212.0",
        "100.0",
        "32.0",
        r#""10°C = 50.0°F""#,
        "true",
        "[:freezing, :to_c, :to_f]",
        "[]",
        r#""private method 'round1' called for module Temp""#,
        "[:hi]",
        "[]",
    ]);
    assert!(err("module M { module_function\n fn f = 1 }").contains("`module_function` was removed"));
}

/// `fn init(@name, @age = 0)`: a parameter written as a field is stored in
/// it (Crystal's shorthand); positional, optional and keyword forms, in any
/// fn; the local is there too.
#[test]
fn field_parameters() {
    let out = run(r##"
        class Person {
          prop name; prop age
          fn init(@name, @age = 0, @tags: []) { p [name, age] }
          fn rename(@name) = self
        }
        class Point { fn init(@x, @y) {} }
        a = Person::new("Ann", 3)
        p a, Person::new("Bob", tags: [:x]), a.rename("Cy").name, Point::new(1, 2)
        p catch_panic { Point::new(1) }.err.message
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"["Ann", 3]"#,
        r#"["Bob", 0]"#,
        r#"#<Person @name="Cy", @age=3, @tags=[]>"#,
        r#"#<Person @name="Bob", @age=0, @tags=[:x]>"#,
        r#""Cy""#,
        "#<Point @x=1, @y=2>",
        r#""wrong number of arguments (given 1, expected 2)""#,
    ]);
    assert!(err("fn f(@a, @a) = 1").contains("duplicate parameter `@a`"));
    assert!(err("interface I { fn f(@a) = 1 }").contains("interfaces have no state"));
    assert!(err("[1].each { |@a| a }").contains("unexpected token in parameter list"));
}

/// `Type::func(...)` is the style for constructors, class methods and module
/// functions; `Type.func` still works, and classes in variables use `.`.
/// After `::` a keyword is a method name (`P::class`).
#[test]
fn path_calls() {
    let out = run(r##"
        class P {
          fn init(@a = 1) {}
          prop a
          fn self.make(x) = yield(x)
          fn self.end = :end
        }
        module M { fn f(x) = x + 1 }
        cls = P
        p P::new(2).a, P.new(3).a, cls.new.a, P::make(4) { |x| x * 2 }, P::make(5, &|x| -x)
        p M::f(1), M.f(1), P::class, P::end, P::name, Hash::new(0)[:k], P::new::a
    "##);
    assert_eq!(out, "2\n3\n1\n8\n-5\n2\n2\nClass\n:end\n\"P\"\n0\n1\n");
}
