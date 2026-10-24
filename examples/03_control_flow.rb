# 03 — Control flow: every construct takes a `{ }` body

temperature = 23

# if / else if / else — an expression with a value, as in Rust (there is no
# `c ? a : b`: write `if c { a } else { b }`)
if temperature > 30 {
  puts "hot"
} else if temperature > 15 {
  puts "mild"
} else {
  puts "cold"
}
label = if temperature > 20 { "warm" } else { "cool" }
p label

# unless is `if not`
unless temperature < 0 { puts "no frost" }

# a statement modifier; `if` as a value anywhere an expression goes
puts "warm day" if temperature > 20
p(if temperature.even? { :even } else { :odd })
parity = [1, 2, 3].map(|n| if n.even? { "even" } else { "odd" })
p parity

# while / until
i = 0
while i < 3 {
  puts "while #{i}"
  i += 1
}
until i == 0 { i -= 1 }

# for ... in
for fruit in ["apple", "pear"] {
  puts "for #{fruit}"
}

# loop runs until `break`, which may give it a value
n = 0
found = loop {
  n += 1
  break n if n * n > 50
}
p found

# next skips to the next iteration
evens = []
for x in 1..10 {
  next if x.odd?
  evens << x
}
p evens

# case / when compares with ===: values, classes, ranges
fn kind(v) {
  case v {
    when 0 { "zero" }
    when 1..9 { "digit" }
    when Integer { "number" }
    when String, Symbol { "text" }
    else { "something else" }
  }
}
p kind(0), kind(7), kind(42), kind("hi"), kind(:sym), kind(42.5)

# a block in a condition goes in parentheses (or use a closure argument)
words = ["tea", "coffee"]
if words.any?(|w| w.size > 5) { puts "a long word" }
if (words.all? { |w| w.size > 2 }) { puts "all longer than 2" }
