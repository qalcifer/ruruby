//! lang-fork: Rust-like enums (src/core/enums.rs).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const SHAPE: &str = r##"
    enum Align { Start, Center, End }

    enum Shape {
      Circle(r)
      Rect { w, h }
      Empty

      fn area() = case self {
        in Circle(r) { 3 * r * r }
        in Rect(w:, h:) { w * h }
        in Empty { 0 }
      }
      fn grow() = case self {
        in Circle(r) { Circle(r + 1) }      # bare: the enum's own constructor
        else { self }
      }
    }
"##;

/// Unit, tuple and struct variants: creating, printing (like Rust's
/// Debug), fields, methods of the enum, types.
#[test]
fn variants() {
    let out = run(&format!("{SHAPE}{}", r##"
        c = Shape::Circle(2)
        r = Shape::Rect(w: 2, h: 3)
        p c, r, Shape::Empty, Align::Center, "#{c}"
        p c.area, r.area, Shape::Empty.area, c.grow, c.r, r.w, r.h
        p c.is_a?(Shape), c.is_a?(Shape::Circle), Shape::Empty.is_a?(Shape), c.class, c.variant, Align::End.variant
        p c.frozen?, Shape::Empty.frozen?, Shape::Circle::new(5), Align::variants, Shape::variants.size
        p Shape::Circle([1, 2]), Shape::Rect(h: Shape::Empty, w: "x")
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "Circle(2)",
        "Rect(w: 2, h: 3)",
        "Empty",
        "Center",
        r#""Circle(2)""#,
        "12",
        "6",
        "0",
        "Circle(3)",
        "2",
        "2",
        "3",
        "true",
        "true",
        "true",
        "Shape::Circle",
        ":Circle",
        ":End",
        "true",
        "true",
        "Circle(5)",
        "[Start, Center, End]",
        "3",
        "Circle([1, 2])",
        r#"Rect(w: "x", h: Empty)"#,
    ]);
}

/// Values compare by value, work as Hash keys, order by variant then
/// fields; `from` finds a unit variant by name.
#[test]
fn equality_and_order() {
    let out = run(&format!("{SHAPE}{}", r##"
        c = Shape::Circle(2)
        p c == Shape::Circle(2), c == Shape::Circle(3), c == Shape::Empty, c.eql?(Shape::Circle(2)), c.equal?(Shape::Circle(2))
        p({c => 1}[Shape::Circle(2)], [c, Shape::Circle(2), Shape::Empty].uniq.size, Shape::Empty == Shape::Empty)
        p Align::Start < Align::End, Align::Center.ordinal, [Align::End, Align::Start].sort, [Align::Center, Align::End].max
        p Shape::Circle(1) < Shape::Circle(2), Shape::Circle(9) < Shape::Rect(w: 1, h: 1), (Align::Start <=> Shape::Empty)
        p Align::from(:center), Align::from("END"), Align::from(:middle), Shape::from(:circle)
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "true",
        "false",
        "false",
        "true",
        "false",
        "1",
        "2",
        "true",
        "true",
        "1",
        "[Start, End]",
        "End",
        "true",
        "true",
        "none",
        "Ok(Center)",
        "Ok(End)",
        r#"Err("Align has no variant :middle without fields")"#,
        r#"Err("Shape has no variant :circle without fields")"#,
    ]);
}

/// Patterns: by variant, with positional or named fields, in case/when.
#[test]
fn matching() {
    let out = run(&format!("{SHAPE}{}", r##"
        fn describe(s) = case s {
          in Shape::Circle(r) if r > 5 { "big circle" }
          in Shape::Circle(r) { "circle #{r}" }
          in Shape::Rect(w:, h:) if w == h { "square #{w}" }
          in Shape::Rect(w, h) { "rect #{w}x#{h}" }
          in Shape::Empty { "nothing" }
        }
        p [Shape::Circle(9), Shape::Circle(1), Shape::Rect(w: 2, h: 2), Shape::Rect(w: 1, h: 3), Shape::Empty].map(|s| describe(s))
        a = case Align::End { when Align::Start { 1 } when Align::End { 2 } }
        p a, (Shape::Circle(1) in Shape::Circle), (Shape::Empty in Shape::Circle)
        p catch_panic { describe(Align::Start) }.err.class
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"["big circle", "circle 1", "square 2", "rect 1x3", "nothing"]"#,
        "2",
        "true",
        "false",
        "NoMatchingPatternError",
    ]);
}

/// An enum body is a class body too: constants, interfaces, overriding the
/// generated methods; Result is an enum.
#[test]
fn enums_are_classes() {
    let out = run(r##"
        interface Describe { fn describe() }
        enum Dir {
          N, E, S, W
          ALL = 4
          fn turn() = Dir::variants[(ordinal + 1) % ALL]
          fn to_s() = variant.to_s.downcase
          impl Describe { fn describe() = "heading #{self}" }
        }
        p Dir::N.turn, Dir::W.turn, Dir::S.describe, "#{Dir::E}", Dir::N.inspect, Dir::implements?(Describe)
        p Ok(1), Err("x"), Ok(1).is_a?(Result), Ok(1).class, Ok(2) == Ok(2), Result::variants
        r = Result::Ok(3)
        p r.unwrap, (case r { in Ok(v) { v } })
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "E",
        "N",
        r#""heading s""#,
        r#""e""#,
        r#""N""#,
        "true",
        "Ok(1)",
        r#"Err("x")"#,
        "true",
        "Result::Ok",
        "true",
        "[Result::Ok, Result::Err]",
        "3",
        "3",
    ]);
}

#[test]
fn errors() {
    let cases = [
        ("Shape::new", "Shape is an enum: create one of its variants (Shape::Variant(...))"),
        ("Shape::Circle(1, 2)", "Shape::Circle takes 1 field, given 2"),
        ("Shape::Circle()", "Shape::Circle takes 1 field, given 0"),
        ("Shape::Rect(w: 1)", "Shape::Rect is missing h"),
        ("Shape::Rect(w: 1, h: 2, d: 3)", "Shape::Rect has no field :d"),
        ("Shape::Rect(1, 2)", "Shape::Rect takes its fields by name"),
        ("Shape::Empty.class.new", "Shape::Empty is a value without fields: use Shape::Empty itself"),
        ("enum Shape { X }", "enum Shape is already defined"),
        ("class Point {}\nenum Point { A }", "Point is already a class"),
        ("class Point2 { fn x = 1 }\nenum Point2 { A }", "Point2 is already a class"),
        ("enum Twice { A, A }", "enum Twice: variant A is defined twice"),
        ("enum Twice { A(x, x) }", "enum Twice: A has the field x twice"),
        ("Shape::Circle(1).instance_variable_set(:@r, 2)", "can't modify frozen"),
        ("Shape::Empty.Circle(1)", "private method 'Circle' called"),
    ];
    for (src, want) in cases {
        let err = run_capture(&format!("{SHAPE}{src}")).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
    let err = run_capture("enum E { A(1) }").unwrap_err().to_string();
    assert!(err.contains("enum: a field name of A"), "{err}");
}
