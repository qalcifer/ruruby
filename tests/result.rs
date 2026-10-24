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
        p (begin; half(3).unwrap; rescue Panic => e; e.message; end)
        p (begin; half(3).expect("need even"); rescue Panic => e; e.message; end)
        p (begin; begin; panic("boom"); rescue => e; :caught; end; rescue Panic => e; [:not_standard, e.message]; end)
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
        r#"nil"#,
        r#"ok 2"#,
        r#"err 5 is odd"#,
        r#""called unwrap on Err(\"3 is odd\")""#,
        r#""need even: \"3 is odd\"""#,
        r#"[:not_standard, "boom"]"#,
        r#"true"#,
        r#"true"#,
    ]);
}
