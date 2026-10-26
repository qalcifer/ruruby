# 13 — Enums: a value that is one of several variants (Rust's enum)

# Variants without fields are named values: compared, ordered, printed
enum Align { Start, Center, End }

p Align::Center, Align::Start < Align::End, Align::variants
p Align::from(:center), Align::from(:middle)          # from a name: a Result

# Variants can carry fields: positional `Circle(r)` or named `Rect { w, h }`.
# The enum body is a class body: methods, constants, impl blocks.
enum Shape {
  Circle(r)
  Rect { w, h }
  Empty

  fn area() = case self {
    in Circle(r) { (3.14159 * r * r).round(2) }
    in Rect(w:, h:) { w * h }
    in Empty { 0 }
  }

  fn scale(k) = case self {
    in Circle(r) { Shape::Circle(r * k) }
    in Rect(w:, h:) { Shape::Rect(w: w * k, h: h * k) }
    in Empty { self }
  }
}

c = Shape::Circle(1)
r = Shape::Rect(w: 2, h: 3)
p c, r, Shape::Empty                                  # printed like Rust's Debug
p c.area, r.scale(2), r.w, c.is_a?(Shape), c.variant

# Values are frozen and compare by value, so they work as Hash keys
p c == Shape::Circle(1), {c => "unit circle"}[Shape::Circle(1)], c.frozen?

# Pattern matching takes variants apart; a guard refines a branch. As in
# Rust, a `case` must handle every variant (or have an `else`): leaving one
# out is an error before the program runs, e.g.
#   line 34: this case on Shape doesn't handle Empty: add an `in` branch for it or an `else`
fn describe(s) = case s {
  in Shape::Circle(r) if r > 10 { "a big circle" }
  in Shape::Circle(r) { "a circle of radius #{r}" }
  in Shape::Rect(w:, h:) if w == h { "a square of side #{w}" }
  in Shape::Rect(w, h) { "a #{w}x#{h} rectangle" }      # fields in order
  in Shape::Empty { "nothing" }
}
[c, Shape::Circle(20), Shape::Rect(w: 2, h: 2), r, Shape::Empty].each { |s| puts describe(s) }

# A state machine: each event moves to the next state
enum Light {
  Off
  On(brightness)

  fn press() = case self {
    in Off { Light::On(100) }
    in On(_) { Light::Off }
  }
}
state = Light::Off
[1, 2, 3].each { state = state.press; p state }

# Result is an enum too: Ok(value) | Err(error)
p Result::variants, Ok(1).is_a?(Result), (case Err("no") { in Ok(v) { v } in Err(e) { e } })

# Mistakes are caught when the value is made
p catch_panic { Shape::Circle(1, 2) }.err.message
p catch_panic { Shape::Rect(w: 1) }.err.message
p catch_panic { Shape::new }.err.message
