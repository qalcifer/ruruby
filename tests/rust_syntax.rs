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
