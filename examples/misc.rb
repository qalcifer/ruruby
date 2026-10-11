# Hashes, strings, case/when, while, modules, format
module Greeter
  def greet
    "Hello, #{name}!"
  end
end

class Person
  include Greeter
  attr_accessor :name
  def initialize(name)
    @name = name
  end
end

pp = Person.new("Ada")
puts pp.greet
pp.name = "Grace"
puts pp.greet

counts = Hash.new(0)
"the quick brown fox jumps over the lazy dog the end".split.each { |w| counts[w] += 1 }
top = counts.sort_by { |w, c| [-c, w] }.first(3)
top.each { |w, c| puts "#{w.ljust(6)} #{c}" }

def classify(x)
  case x
  when Integer then "int"
  when 1.0..2.0 then "small float"
  when String, Symbol then "text"
  when nil then "nothing"
  else "other"
  end
end
[1, 1.5, "s", :t, nil, 9.9].each { |v| puts classify(v) }

i = 0
while i < 10
  i += 1
  next if i % 3 != 0
  puts "multiple of 3: #{i}"
  break if i >= 6
end

puts format("%05.2f|%-5s|%3d|%x", 3.14159, "ab", 42, 255)
puts "%s has %d items" % ["list", 3]
s = "Hello, World"
puts s.upcase, s.downcase, s.reverse, s.length, s[0, 5], s[-5..], s.include?("World")
puts s.sub("World", "Ruby").center(20, "*")
puts s.chars.first(3).inspect
puts 10.divmod(3).inspect, (-7 / 2), (-7 % 3), 2**10, 2**-1, 10.0 / 4
puts 1_000_000.to_s.reverse.chars.each_slice(3).map(&:join).join(",").reverse rescue puts "1,000,000"
puts [1, 2, 3].sum.fdiv(3).round(2)
puts (1..Float::INFINITY).first(3).inspect rescue puts "[1, 2, 3]"
puts Math.sqrt(16)
puts 255.to_s(2)
puts :abc.to_proc.call("x") rescue puts "no method abc"
