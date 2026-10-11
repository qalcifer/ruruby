# Classes, inheritance, super, attr_reader, Comparable, to_s, class methods
class Animal
  include Comparable
  attr_reader :name, :age

  def self.kingdom
    "Animalia"
  end

  def initialize(name, age)
    @name = name
    @age = age
  end

  def <=>(other)
    age <=> other.age
  end

  def speak
    "..."
  end

  def to_s
    "#{self.class.name}(#{@name}, #{@age}): #{speak}"
  end
end

class Dog < Animal
  def initialize(name, age, breed = "mutt")
    super(name, age)
    @breed = breed
  end

  def speak
    "Woof! I'm a #{@breed}"
  end
end

class Cat < Animal
  def speak
    super + " (meow)"
  end
end

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
