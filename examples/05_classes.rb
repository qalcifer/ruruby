# 05 — Classes and objects

class Animal {
  # attr_reader makes `name` and `age` readable methods
  attr_reader :name, :age

  # `Animal::new(...)` calls init; @variables belong to the object
  fn init(name, age) {
    @name = name
    @age = age
  }

  fn speak = "..."

  # to_s is how `puts` and interpolation show the object
  fn to_s = "#{self.class.name} #{name} (#{age}): #{speak}"

  # class methods are defined on `self`
  fn self.kingdom = "Animalia"
}

class Dog < Animal {
  # a parameter written `@breed` is stored in @breed: no `@breed = breed`
  fn init(name, age, @breed = "mutt") {
    super(name, age)           # the parent's init
  }

  fn speak = "Woof! I'm a #{@breed}"
}

class Cat < Animal {
  fn speak = super + " (meow)"   # the parent's speak, extended
}

animals = [Dog.new("Rex", 5, "beagle"), Cat.new("Tom", 3), Dog.new("Fido", 7)]
animals.each { |a| puts a }
p Dog.kingdom, Dog.superclass, Dog.new("x", 1).is_a?(Animal), Cat.ancestors.first(2)

# Operators are methods; implementing the Comparable interface (its one
# required method is <=>) gives <, >, between?, clamp... (see 06_interfaces.rb)
class Version {
  attr_reader :major, :minor

  fn init(@major, @minor) {}

  impl Comparable {
    fn <=>(other) = [major, minor] <=> [other.major, other.minor]
  }
  fn to_s = "#{major}.#{minor}"
  fn +(other) = Version.new(major + other.major, minor + other.minor)
}

v1 = Version.new(1, 2)
v2 = Version.new(1, 10)
p v1 < v2, [v2, v1].max.to_s, v1.between?(v1, v2), (v1 + v2).to_s

# Visibility: private methods are called without a receiver
class Account {
  attr_reader :balance

  fn init = @balance = 0

  fn deposit(amount) {
    check(amount)
    @balance += amount
    self                         # returning self allows chaining
  }

  private fn check(amount) {
    panic("deposits must be positive, got #{amount}") if amount <= 0
  }
}

acct = Account.new.deposit(50).deposit(25)
p acct.balance, acct.respond_to?(:check)
p catch_panic { acct.deposit(-1) }.err.message

# Classes stay open: a later `class` body adds methods
class Account {
  fn empty? = balance == 0
}
p acct.empty?, Account.new.empty?

# Objects can get methods of their own
special = Object.new
fn special.greet = "only me"
p special.greet

# attr_accessor makes a reader and a writer
class Point {
  attr_accessor :x, :y
  fn init(@x, @y) {}
}
pt = Point.new(1, 2)
pt.x = 10
p [pt.x, pt.y]
