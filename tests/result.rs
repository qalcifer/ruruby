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

/// `expr?`: Err and none return from the method (from a block: from the
/// method around it), Ok(v) gives v. After a method name `?` belongs to
/// the name (`empty?`): `.first()?`.
#[test]
fn try_operator() {
    let out = run(r##"
        def half(n) = n.even? ? Ok(n / 2) : Err("#{n} is odd")
        def quarter(n)
          h = half(n)?
          Ok(half(h)?)
        end
        p quarter(8), quarter(6), quarter(5)
        def first_word(s)
          w = s.split(" ").first()?
          w.upcase
        end
        p first_word("hi there"), first_word("")
        def from_block(xs)
          xs.each { |x| half(x)? }
          :all_even
        end
        p from_block([2, 4]), from_block([2, 3, 4])
        def local_try(n)
          r = half(n)
          v = r?
          v + 100
        end
        p local_try(10), local_try(1)
        def chain(n) = Ok(half(n)?.to_s + "!")
        p chain(4), chain(3)
        x = 5
        p x > 3 ? :big : :small, [1].empty?, x.even? ? 1 : 2
        def plain(v) = Ok(v?)
        p plain(7), plain(nil)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"Ok(2)"#,
        r#"Err("3 is odd")"#,
        r#"Err("5 is odd")"#,
        r#""HI""#,
        r#"nil"#,
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
        r#"nil"#,
    ]);
}
