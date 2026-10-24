# Classes, inheritance, super, attr_reader, Comparable, to_s, class methods
class Animal {
  include Comparable
  attr_reader :name, :age

  fn self.kingdom() {
    "Animalia"
  }

  fn initialize(name, age) {
    @name = name
    @age = age
  }

  fn <=>(other) {
    age <=> other.age
  }

  fn speak() {
    "..."
  }

  fn to_s() {
    "#{self.class.name}(#{@name}, #{@age}): #{speak}"
  }
}

class Dog < Animal {
  fn initialize(name, age, breed = "mutt") {
    super(name, age)
    @breed = breed
  }

  fn speak() {
    "Woof! I'm a #{@breed}"
  }
}

class Cat < Animal {
  fn speak() {
    super + " (meow)"
  }
}

pets = [Dog.new("Rex", 5, "beagle"), Cat.new("Tom", 3), Dog.new("Fido", 7)]
pets.each { |p| puts p }
puts pets.min.name
puts pets.max.name
puts pets.sort.map(&:name).join(", ")
puts pets[0] > pets[1]
puts Animal.kingdom
puts Dog.kingdom
puts Dog.ancestors.inspect
puts Dog.superclass
puts pets[1].is_a?(Animal), pets[1].is_a?(Dog)
