//! lang-fork: exception handling is gone from the language. Each removed
//! form is a compile error that names what to use instead; the constructs
//! that only look alike keep working.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const ERR_ADVICE: &str = "return Err(...) for expected failures, panic(...) for bugs";

#[test]
fn exception_forms_are_compile_errors() {
    let cases: &[(&str, &str)] = &[
        ("begin; 1; rescue; 2; end", "`rescue` was removed"),
        ("begin; 1; rescue ArgumentError => e; 2; end", "`rescue` was removed"),
        ("def f; 1; rescue => e; 2; end", "`rescue` was removed"),
        ("[1].each do |x| p x; rescue; end", "`rescue` was removed"),
        ("x = 1 rescue 2", "`rescue` was removed"),
        ("a, b = 1, 2 rescue 3", "`rescue` was removed"),
        ("def f = 1 rescue 2", "`rescue` was removed"),
        ("begin; 1; else; 2; end", "`begin/else` was removed"),
        ("retry", "`retry` was removed"),
        ("raise 'x'", "`raise` was removed"),
        ("raise ArgumentError, 'x'", "`raise` was removed"),
    ];
    for (src, expected) in cases {
        let err = run_capture(src).unwrap_err().to_string();
        assert!(err.contains(expected) && err.contains(ERR_ADVICE), "{src}: {err}");
    }
    let err = run_capture("def m; yield; ensure; p 1; end").unwrap_err().to_string();
    assert!(
        err.contains("`ensure` was removed: there are no exceptions to clean up after"),
        "{err}"
    );
    // nothing runs when compiling fails
    assert!(run_capture("puts 1\nx = 2 rescue 3").is_err());
}

#[test]
fn look_alikes_still_work() {
    let out = run(r#"
        x = if false then 1 else 2 end
        y = case 3 when 1 then :a else :b end
        z = begin 5 end
        n = 0
        begin n += 1 end while n < 3
        p x, y, z, n, (begin; 1; 2; end), [1].map { |v| v }
        # `raise` is only removed as a function: a method of that name is fine
        obj = Object.new
        def obj.raise(x) = "custom #{x}"
        p obj.raise(1)
        # an unmatched case/in still panics with its kind
        p catch_panic { case 5; in String then 1; end }.err.class
        p loop { break :out }
    "#);
    assert_eq!(out, "2\n:b\n5\n3\n2\n[1]\n\"custom 1\"\nNoMatchingPatternError\n:out\n");
}
