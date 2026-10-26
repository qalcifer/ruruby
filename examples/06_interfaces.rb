# 06 — Interfaces: shared behaviour; modules as namespaces
#
# (For splitting a program into files, see 07_use/.)

# An interface names a contract. A `fn` without a body is required; one with
# a body is a default that implementors get for free.
interface Shape {
  fn area()
  fn name() = self.class.name.downcase
  fn describe() = "#{name} with area #{area}"
}

# `impl Interface for Class { }` holds the contract's methods together
class Circle {
  fn init(@r) {}
}

impl Shape for Circle {
  fn area() = (3.14159 * @r * @r).round(2)
}

# ...or, inside the class, `impl Interface { }`
class Rect {
  fn init(@w, @h) {}

  impl Shape {
    fn area() = @w * @h
    fn name() = "rectangle"      # overrides the default
  }
}

shapes = [Circle::new(1), Rect::new(2, 3)]
shapes.each { |s| puts s.describe }
p Rect::new(1, 1).is_a?(Shape), Rect::implements?(Shape), shapes.sum(&:area)

# Built-in classes can implement your interfaces too
impl Shape for Integer {
  fn area() = self * self        # a number as a square's side
}
p 4.describe

# An interface can build on others: implementing Solid needs Shape's methods
interface Solid: Shape {
  fn volume()
  fn describe() = "#{super}, volume #{volume}"   # super: Shape's default
}

class Cube {
  fn init(@side) {}

  impl Solid {
    fn area() = 6 * @side * @side
    fn volume() = @side ** 3
  }
}
p Cube::new(2).describe, Cube::new(2).is_a?(Shape)

# Subclasses inherit their parent's interfaces. A method a class has (its
# own or inherited) beats an interface default: "class wins"
class Square < Rect {
  fn init(side) = super(side, side)
}
class Labeled { fn name() = "labeled" }
class Badge < Labeled {
  fn area() = 1
  impl Shape {}
}
p Square::new(3).describe, Badge::new.describe   # Badge keeps Labeled's name

# An abstract class is a base to finish: no `new`; a `fn` without a body is
# an abstract method, and its impl blocks may leave required methods to the
# subclasses. Each concrete subclass is checked when its body ends.
abstract class Widget {
  fn init(@text) {}
  fn draw()                              # abstract
  fn render() = "<#{draw}>"
  impl Shape {}                          # area() is the subclasses' job
}

class Button < Widget {
  fn draw() = "button #{@text}"
  fn area() = 4
}
p Button::new("ok").render, Button::new("ok").describe, Widget::abstract?
p catch_panic { Widget::new("x") }.err.message

# The contract is checked when the impl block ends, so a mistake stops the
# program at once, with the reason:
class Draft { }
p catch_panic { impl Shape for Draft { } }.err.message

# Interfaces hold no state: the class keeps its fields. To reuse a field's
# methods, `impl Interface by @field` forwards the required methods to it
interface Queue {
  fn push(item)
  fn shift()
  fn size()
  fn empty() = size == 0
}

class Jobs {
  fn init() = @list = []
  impl Queue by @list
}
jobs = Jobs::new
jobs.push(:build)
jobs.push(:test)
p jobs.shift, jobs.size, jobs.empty

# The core interfaces: Comparable requires <=>, Enumerable requires each
class NumberBag {
  fn init(*numbers) = @numbers = numbers

  impl Enumerable {
    fn each() {
      @numbers.each { |n| yield n }
      self
    }
  }
}
bag = NumberBag::new(5, 3, 8)
p bag.sort, bag.map(|n| n * 2), bag.min, bag.include?(8), bag.select(|n| n > 4)

# Modules group constants, functions and classes under a name, like a
# Rust `mod`: a module's `fn`s are called on it (`Geometry::circle_area`)
module Geometry {
  PI = 3.14159

  fn circle_area(r) = round2(PI * r * r)
  private fn round2(x) = x.round(2)   # only for the module's own functions

  class Square {
    fn init(@side) {}
    fn area = @side * @side
  }
}
p Geometry::PI, Geometry::circle_area(2), Geometry::Square::new(3).area
