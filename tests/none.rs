//! lang-fork: `none` is the absent value (Ruby's nil, which is gone).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

/// none is falsy and empty, prints as none, is its own NoneClass; `none?`
/// is true only for it (Enumerable#none? still answers for collections);
/// ||=, &&=, &., defaults, patterns and errors see it.
#[test]
fn none_value() {
    let out = run(r##"
        p none.to_s, none.to_a, none.inspect, none.class, none.none?, 0.none?, "".none?, [].none?, [none].none?, [1].none?
        p(if none { 1 } else { 2 }, !none, none == false, none.equal?(none), [1, none, 2].compact, {a: none}.compact)
        x = none
        x ||= 5
        y = 3
        y &&= none
        p x, y, none&.length, "ab"&.length, none.to_a.empty?
        h = Hash.new
        p h[:missing], [][3], [none].first, "#{none}|", [none].to_s, {none => 1}
        fn f(a = none, b: none) = [a, b]
        p f, f(1, b: 2), defined?(none), none.frozen?, none.object_id == none.object_id
        case none {
        in none { p :matched_none }
        }
        case 5 {
        when none { p :no }
        else { p :else_branch }
        }
        p catch_panic { none.upcase }.err.message, catch_panic { 1 + none }.err.message
        p defined?(NilClass), 1.respond_to?(:nil?), none.respond_to?(:none?)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""""#,
        r#"[]"#,
        r#""none""#,
        r#"NoneClass"#,
        r#"true"#,
        r#"false"#,
        r#"false"#,
        r#"true"#,
        r#"true"#,
        r#"false"#,
        r#"2"#,
        r#"true"#,
        r#"false"#,
        r#"true"#,
        r#"[1, 2]"#,
        r#"{}"#,
        r#"5"#,
        r#"none"#,
        r#"none"#,
        r#"2"#,
        r#"true"#,
        r#"none"#,
        r#"none"#,
        r#"none"#,
        r#""|""#,
        r#""[none]""#,
        r#"{none => 1}"#,
        r#"[none, none]"#,
        r#"[1, 2]"#,
        r#""expression""#,
        r#"true"#,
        r#"true"#,
        r#":matched_none"#,
        r#":else_branch"#,
        r#""undefined method 'upcase' for none""#,
        r#""none can't be coerced into Integer""#,
        r#"none"#,
        r#"false"#,
        r#"true"#,
    ]);
}

/// `nil` is a compile error anywhere, with the replacement in the message.
#[test]
fn nil_is_gone() {
    for src in ["x = nil", "fn f(a = nil) = a", "p({a: nil})", "[nil]", "y = 1 if nil"] {
        let err = run_capture(src).unwrap_err().to_string();
        assert!(err.contains("`nil` was removed: use `none`"), "{src}: {err}");
    }
}
