# 14 — Props: an object's properties

# `prop name` can be read from outside and set inside the class; `prop mut`
# can be set from outside too. A class with props and no `fn init` gets
# one that takes them by name; a prop without a default must be given.
class Task {
  prop title                         # required
  prop mut done = false              # a default
  prop mut priority: Integer = 3     # typed: checked when set
  prop tags = []                     # a fresh [] for each task

  fn finish() { self.done = true; self }
}

t = Task::new(title: "write docs")
p t, t.title, t.done, Task::props
t.priority = 1
t.tags << :docs
p t.finish.done, t.priority, Task::new(title: "x").tags      # tags not shared

# Mistakes are caught when they happen
p catch_panic { Task::new }.err.message
p catch_panic { Task::new(title: "x", owner: "me") }.err.message
p catch_panic { t.priority = "high" }.err.message
p catch_panic { t.title = "renamed" }.err.message           # not `mut`

# Accessors of its own: `set` checks or converts, `changed` reacts to a
# change, `get` alone makes a computed prop
class Temperature {
  prop mut celsius: Float = 0.0 {
    set(v) {
      panic("below absolute zero: #{v}") if v < -273.15
      @celsius = v
    }
    changed(old) { puts "  #{old} -> #{celsius}" }
  }
  prop fahrenheit { get() = celsius * 9 / 5 + 32 }
}

temp = Temperature::new(celsius: 20.0)    # `changed` isn't run by new
temp.celsius = 25.0
temp.celsius = 25.0                         # the same value: no change
p temp.fahrenheit, catch_panic { temp.celsius = -300.0 }.err.message

# A class with its own init still declares its props; field parameters
# set them
class Point {
  prop x
  prop y
  fn init(@x, @y) {}
  fn +(o) = Point::new(x + o.x, y + o.y)
}
p Point::new(1, 2) + Point::new(3, 4)

# Interfaces can require props: the class decides how to store them
interface Named {
  prop name
  fn greet() = "hello, #{name}"
}
class User {
  prop name
  prop mut email = ""
  impl Named {}
}
p User::new(name: "Ada").greet
