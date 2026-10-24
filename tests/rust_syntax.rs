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
          blk ? blk.call(s) : [s, rest]
        }
        fn none_args { :no_params }
        fn empty() {}
        class V
          def initialize(n) = @n = n
          fn self.make(n) { new(n) }
          fn ==(o) { o.is_a?(V) && o.n == n }
          fn n = @n
          fn [](i) { @n * i }
          fn -@ = V.new(-@n)
          private fn hidden() { :h }
          fn call_hidden = hidden
        end
        p add(1, 2), square(4), greet, greet("Ann", 1, 2), greet(loud: true) { |s| s + "!" }, none_args, empty
        p V.make(3) == V.new(3), V.new(2)[5], (-V.new(4)).n, V.new(1).call_hidden, V.new(1).respond_to?(:hidden)
        fn multi(x) {
          if x > 1
            :big
          else
            :small
          end
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
          fn initialize(name) { @name = name }
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
        puts Dog.create("rex"), Animal.new("a"), Greeting.hello("you"), Greeting::VERSION
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
