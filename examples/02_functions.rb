# 02 — Functions: fn, parameters, return values

# A function's last expression is its value, as in Rust.
fn add(a, b) {
  a + b
}
p add(2, 3)

# One-expression functions can use `=`
fn square(x) = x * x
p square(9)

# `return` leaves early
fn sign(n) {
  return :zero if n == 0
  n > 0 ? :positive : :negative
}
p sign(5), sign(0), sign(-2)

# Default values, keyword arguments, rest arguments
fn greet(name, greeting = "Hello", punct: "!") = "#{greeting}, #{name}#{punct}"
p greet("Ada"), greet("Grace", "Hi"), greet("Linus", punct: "?")

fn total(*numbers) = numbers.sum
p total, total(1, 2, 3)

fn describe(name, *tags, age:, **extra) = [name, tags, age, extra]
p describe("Ada", :math, :poetry, age: 36, born: 1815)

# Keyword arguments can be required (no default) or optional
fn connect(host:, port: 80) = "#{host}:#{port}"
p connect(host: "example.org"), connect(host: "localhost", port: 8080)

# Arrays can be spread into arguments, hashes into keywords
args = [3, 4]
opts = {port: 443}
p add(*args), connect(host: "secure", **opts)

# Functions without parameters may leave out the parentheses
fn answer = 42
p answer

# Recursion
fn fib(n) = n < 2 ? n : fib(n - 1) + fib(n - 2)
p (0..10).map(|n| fib(n))

# Calling a function with the wrong number of arguments is a bug: a panic
p catch_panic { add(1) }.err.message
