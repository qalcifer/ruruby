//! lang-fork: syntax that is gone from the language. Each removed form is a
//! compile error that names what to use instead; the constructs that only
//! look alike keep working.

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

const ERR_ADVICE: &str = "return Err(...) for expected failures, panic(...) for bugs";

#[test]
fn exception_forms_are_compile_errors() {
    for src in ["fn f() { 1; rescue; 2 }", "x = 1 rescue 2", "a, b = 1, 2 rescue 3", "fn f() = 1 rescue 2", "fn f() { 1 } rescue 2"] {
        let e = err(src);
        assert!(e.contains("`rescue` was removed") && e.contains(ERR_ADVICE), "{src}: {e}");
    }
    for (src, what) in [("retry", "retry"), ("raise 'x'", "raise"), ("raise ArgumentError, 'x'", "raise")] {
        let e = err(src);
        assert!(e.contains(&format!("`{what}` was removed")) && e.contains(ERR_ADVICE), "{src}: {e}");
    }
    let e = err("fn m() {\n  yield\nensure\n  p 1\n}");
    assert!(e.contains("`ensure` was removed: there are no exceptions to clean up after"), "{e}");
    // nothing runs when compiling fails
    assert!(run_capture("puts 1\nx = 2 rescue 3").is_err());
}

/// The Ruby block syntax: def/end, do/end, then, elsif, begin, `->`, and a
/// construct without its `{`.
#[test]
fn ruby_block_forms_are_compile_errors() {
    let cases: &[(&str, &str)] = &[
        ("def f = 1", "`def` was removed: use `fn name(args) { ... }`"),
        ("def f(x)\n  x\nend", "`def` was removed"),
        ("p 1; end", "`end` was removed: close the block with `}`"),
        ("[1].each do |v| p v end", "`do` was removed: use `{ |x| ... }` or a closure `|x| ...`"),
        ("f = ->(x) { x }", "`->` was removed: use a closure `|x| ...`"),
        ("begin; 1; end", "`begin` was removed: group with parentheses `( ... )`"),
        ("if 1 { 2 } elsif 3 { 4 }", "`elsif` was removed: use `} else if cond {`"),
        ("if true then 1 end", "if: expected `{` to start the body"),
        ("x = 1\nif x\n  2\nend", "if: expected `{` to start the body"),
        ("unless false\n 1\nend", "unless: expected `{`"),
        ("while true do break end", "while: expected `{`"),
        ("until true\nend", "until: expected `{`"),
        ("for x in [1]\nend", "for: expected `{`"),
        ("case 1\nwhen 1 then 2\nend", "case: expected `{`"),
        ("class A; end", "class: expected `{` to start the body"),
        ("module M\nend", "module: expected `{`"),
        ("class << self; end", "class << obj: expected `{`"),
        ("fn f x { x }", "fn: expected `(parameters)`"),
        ("x = nil", "`nil` was removed: use `none`"),
        ("x = 1 > 0 ? :a : :b", "`? :` was removed: use `if cond { a } else { b }`"),
        ("p(true ? 1 : 2)", "`? :` was removed"),
        ("class C { fn initialize(a) = a }", "`fn initialize` was removed: the constructor hook is `fn init(...)`"),
        ("class C { fn initialize_copy(o) = o }", "`fn init_copy(...)`"),
        ("class C { attr_reader :a }", "`attr_reader` was removed: use `prop name` (`prop mut name` to write it from outside)"),
        ("class C { attr_accessor :a, :b }", "`attr_accessor` was removed"),
        ("class C { include Comparable }", "`include` was removed: implement an interface with `impl I { ... }`"),
        ("class C { prepend M::N }", "`prepend` was removed"),
        ("o = 1\nextend Enumerable", "`extend` was removed"),
    ];
    for (src, expected) in cases {
        let e = err(src);
        assert!(e.contains(expected), "{src:?}: {e}");
    }
}

#[test]
fn look_alikes_still_work() {
    let out = run(r#"
        x = if false { 1 } else { 2 }
        y = case 3 { when 1 { :a } else { :b } }
        z = (5)
        n = 0
        loop { n += 1; break unless n < 3 }
        p x, y, z, n, (1; 2), [1].map { |v| v }
        # `raise` is only removed as a function: a method of that name is fine
        obj = Object::new
        fn obj.raise(x) = "custom #{x}"
        p obj.raise(1)
        # `end` and `begin` stay method names (Range#end)
        p (1..4).end, (1..4).begin
        # an unmatched case/in still panics with its kind
        p catch_panic { case 5 { in String { 1 } } }.err.class
        p loop { break :out }
        # statement modifiers are unchanged; `if` is the expression form
        p((1 if true), (2 unless true), if true { :t } else { :f })
        i = 0
        i += 1 while i < 5
        p i
        # modules are namespaces: no include/extend/prepend methods, but
        # Array#prepend and String#prepend are unrelated and stay
        p [2].prepend(1), "b".prepend("a"), Module::respond_to?(:include), 1.respond_to?(:extend)
        m = Module::new
        p catch_panic { Class::new { include m } }.err.class
    "#);
    assert_eq!(
        out,
        "2\n:b\n5\n3\n2\n[1]\n\"custom 1\"\n4\n1\nNoMatchingPatternError\n:out\n1\nnone\n:t\n5\n[1, 2]\n\"ab\"\nfalse\nfalse\nNoMethodError\n"
    );
}
