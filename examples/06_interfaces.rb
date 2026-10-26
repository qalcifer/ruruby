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
  fn initialize(r) = @r = r
}

impl Shape for Circle {
  fn area() = (3.14159 * @r * @r).round(2)
}

# ...or, inside the class, `impl Interface { }`
class Rect {
  fn initialize(w, h) { @w = w; @h = h }

  impl Shape {
    fn area() = @w * @h
    fn name() = "rectangle"      # overrides the default
  }
}

shapes = [Circle.new(1), Rect.new(2, 3)]
shapes.each { |s| puts s.describe }
p Rect.new(1, 1).is_a?(Shape), Rect.implements?(Shape), shapes.sum(&:area)

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
  fn initialize(side) = @side = side

  impl Solid {
    fn area() = 6 * @side * @side
    fn volume() = @side ** 3
  }
}
p Cube.new(2).describe, Cube.new(2).is_a?(Shape)

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
  fn initialize() = @list = []
  impl Queue by @list
}
jobs = Jobs.new
jobs.push(:build)
jobs.push(:test)
p jobs.shift, jobs.size, jobs.empty

# The core interfaces: Comparable requires <=>, Enumerable requires each
class NumberBag {
  fn initialize(*numbers) = @numbers = numbers

  impl Enumerable {
    fn each() {
      @numbers.each { |n| yield n }
      self
    }
  }
}
bag = NumberBag.new(5, 3, 8)
p bag.sort, bag.map(|n| n * 2), bag.min, bag.include?(8), bag.select(|n| n > 4)

# Modules group constants and functions under a name
module Geometry {
  PI = 3.14159

  fn self.circle_area(r) = PI * r * r

  class Square {
    fn initialize(side) = @side = side
    fn area = @side * @side
  }
}
p Geometry::PI, Geometry.circle_area(2).round(2), Geometry::Square.new(3).area
