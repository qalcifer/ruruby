# Blocks, closures, yield, procs, break/next, Enumerable
fn twice() {
  yield 1
  yield 2
}

total = 0
twice { |x| total += x * 10 }
puts total

fn make_counter() {
  count = 0
  increment = proc { count += 1 }
  get = proc { count }
  [increment, get]
}

inc, get = make_counter
3.times { inc.call }
puts get.call

fn with_block_check() {
  if block_given? {
    yield
  } else {
    "no block"
  }
}
puts with_block_check
puts with_block_check { "got block" }

r = [1, 2, 3, 4, 5].each { |x|
  break x * 100 if x == 3
}
puts r

evens = (1..10).select { |x| x.even? }
puts evens.inspect
puts (1..10).map { |x| next 0 if x.odd?; x }.inspect
puts [3, 1, 2].sort_by { |x| -x }.inspect
puts [1, 2, 3, 4].inject(:+)
puts (1..4).reduce(1) { |acc, x| acc * x }
words = ["apple", "banana", "cherry", "avocado"]
puts words.group_by { |w| w[0] }.map { |k, v| "#{k}: #{v.join('/')}" }.join("; ")
words.each_with_index { |w, i| puts "#{i}: #{w}" }
puts words.max_by(&:length)
puts words.sum("") { |w| w[0] }
nested = [[1, 2], [3, [4, 5]]]
puts nested.flatten.inspect
a, b = 1, 2
a, b = b, a
puts "#{a} #{b}"
x = 10
add_x = lambda { |y| x + y }
x = 20
puts add_x.call(1)
