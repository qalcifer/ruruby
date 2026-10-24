# 01 — Basics: printing, variables, values
#
# Run any example with:  cargo run --release -- examples/01_basics.rb

# `puts` prints a value's text, `p` its inspected form
puts "Hello, world!"
p "Hello, world!"

# variables need no declaration; they're local to the function or file
name = "Ada"
year = 1815
puts "#{name} was born in #{year}"      # interpolation runs any expression: #{...}

# numbers
p 7 / 2, 7 % 2, 7.0 / 2, 2**10, 1_000_000, -5.abs
p 10.divmod(3), 3.14159.round(2), 255.to_s(2), "42".to_i + 1

# strings
s = "hello"
p s.upcase, s.length, s.reverse, s[0], s[1..3], s + " world", s * 2
p s.include?("ell"), s.start_with?("he"), s.sub("l", "L"), s.center(11, "*")

# symbols: immutable names, often used as keys and tags
p :ok, :ok == :ok, :ok.to_s

# booleans and `none`, the absent value (there is no nil)
p true && false, true || false, !true
nothing = none
p nothing, nothing.none?, 5.none?
# only `false` and `none` are falsy: 0 and "" are true
p (0 ? :truthy : :falsy), ("" ? :truthy : :falsy), (none ? :truthy : :falsy)

# arrays, hashes and ranges
list = [1, 2, 3]
ages = {ada: 36, grace: 85}
p list, list.first, list.last, list.size, list + [4], list.sum
p ages[:ada], ages.keys, ages.size, ages[:nobody]
p (1..5).to_a, (1...5).to_a, (1..10).include?(7)

# several assignments at once, and swapping
a, b = 1, 2
a, b = b, a
p [a, b]
