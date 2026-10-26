# Two modules in one file; `use shapes::Shapes::Circle` reaches into them.

module Shapes {
  class Circle {
    fn init(r) = @r = r
    fn area = (3.14159 * @r * @r).round(2)
  }

  class Square {
    fn init(side) = @side = side
    fn area = @side * @side
  }
}

module Units {
  CM_PER_INCH = 2.54

  fn to_cm(inches) = round2(inches * CM_PER_INCH)
  fn to_inches(cm) = round2(cm / CM_PER_INCH)
  private fn round2(x) = x.round(2)
}

module Text {
  fn shout(s) = s.upcase + "!"
}

SECRET = "only for this file"
