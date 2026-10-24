# 06 — Modules: shared methods and namespaces
#
# (For splitting a program into files, see 07_use/.)

# A module's methods can be mixed into classes with `include`
module Describable {
  fn describe = "#{self.class.name}: #{summary}"
}

class Book {
  include Describable
  fn initialize(title) = @title = title
  fn summary = "a book called #{@title}"
}

class Song {
  include Describable
  fn initialize(title, secs) { @title = title; @secs = secs }
  fn summary = "#{@title}, #{@secs / 60}:#{(@secs % 60).to_s.rjust(2, "0")}"
}

p Book.new("Dune").describe, Song.new("Blue", 125).describe
p Book.include?(Describable), Book.new("x").is_a?(Describable)

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

# Enumerable gives a class map, select, sort, min, include?... from `each`
class NumberBag {
  include Enumerable
  fn initialize(*numbers) = @numbers = numbers
  fn each() {
    @numbers.each { |n| yield n }
    self
  }
}
bag = NumberBag.new(5, 3, 8)
p bag.sort, bag.map(|n| n * 2), bag.min, bag.include?(8), bag.select(|n| n > 4)
