# Recursive method calls and integer arithmetic
fn fib(n) {
  n < 2 ? n : fib(n - 1) + fib(n - 2)
}

puts fib(25)
