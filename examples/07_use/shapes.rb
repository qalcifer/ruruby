# Two modules in one file; `use shapes::Shapes::Circle` reaches into them.

module Shapes {
  class Circle {
    fn initialize(r) = @r = r
    fn area = (3.14159 * @r * @r).round(2)
  }

  class Square {
    fn initialize(side) = @side = side
    fn area = @side * @side
  }
}

module Units {
  CM_PER_INCH = 2.54
}

SECRET = "only for this file"
