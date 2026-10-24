# 04 — Closures and blocks

# A closure is a function value: |params| expression, or |params| { statements }
double = |x| x * 2
add = |a, b| a + b
greet = || "hello"
clamp = |x| {
  return 0 if x < 0       # return leaves the closure
  if x > 10 { 10 } else { x }
}
p double.(4), add.call(2, 3), greet.(), clamp.(-5), clamp.(50), clamp.(7)

# Closures capture the variables around them
count = 0
bump = || count += 1
bump.()
bump.()
p count

fn make_counter() {
  n = 0
  [|| n += 1, || n]        # both share `n`, which outlives the call
}
inc, get = make_counter
3.times { inc.() }
p get.()

# Functions that take closures: a closure written last is the call's block
p [1, 2, 3].map(|x| x * 10), [1, 2, 3, 4].select(|x| x.even?)
p [3, 1, 2].sort(|a, b| b <=> a), (1..4).inject(0, |sum, x| sum + x)

# ... and so is a `{ |x| }` block after the call
p [1, 2, 3].map { |x| x + 1 }
{a: 1, b: 2}.each { |key, value| puts "#{key} = #{value}" }

# A function calls its block with `yield`, or receives it as `&f`
fn twice() {
  yield 1
  yield 2
}
twice { |x| puts "got #{x}" }

fn apply(x, &f) = f.(x)
p apply(5, |x| x * x)

fn maybe() = if block_given? { yield } else { "no block" }
p maybe, maybe { "with a block" }

# break ends the call that took the block, next ends one run of it
p [1, 2, 3, 4].each { |x| break x * 100 if x == 3 }
p [1, 2, 3].map { |x| next 0 if x == 2; x }

# A closure stored in a variable can be passed as the block with &
shout = |s| s.upcase + "!"
p ["hi", "bye"].map(&shout)

# Closures returning closures
adder = |n| |x| x + n
add10 = adder.(10)
p add10.(5)

# Symbols turn into closures that call that method
p ["a", "b"].map(&:upcase)
