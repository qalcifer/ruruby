# 08 — Errors: Result values, `?`, none, and panics
#
# There are no exceptions. A failure the caller should handle is a value,
# Ok(value) or Err(error); a bug is a panic, which stops the program.

fn parse_age(text) {
  n = Integer(text)?                    # Integer() gives Ok(n) or Err(message)
  return Err("age can't be negative: #{n}") if n < 0
  Ok(n)
}

p parse_age("36"), parse_age("-1"), parse_age("abc")

# `?` unwraps an Ok, or returns the Err from the function right away
fn birth_year(text, now: 2026) {
  age = parse_age(text)?
  Ok(now - age)
}
p birth_year("36"), birth_year("old")

# Look at a Result with case/in ...
["20", "7", "x"].each { |text|
  case parse_age(text) {
    in Ok(age) if age >= 18 { puts "#{age}: adult" }
    in Ok(age) { puts "#{age}: minor" }
    in Err(message) { puts "error: #{message}" }
  }
}

# ... or with its methods
r = parse_age("12")
p r.ok?, r.unwrap, r.map(|a| a + 1), parse_age("x").unwrap_or(0)
p parse_age("x").map_err(&:upcase), r.and_then(|a| if a > 10 { Ok(:teen) } else { Err(:child) })

# Lookups that can miss give a Result too
config = {port: "8080"}
p config.fetch(:port), config.fetch(:host), config.fetch(:host, "localhost")
p [10, 20].fetch(5), [10, 20].fetch(-1)

fn server_port(config) {
  raw = config.fetch(:port)?
  port = Integer(raw)?
  Ok(port)
}
p server_port(config), server_port({}), server_port({port: "eighty"})

# none is the absent value; `?` returns it as well
fn first_even_half(numbers) {
  n = numbers.find(&:even?)?
  n / 2
}
p first_even_half([3, 8, 5]), first_even_half([1, 3])

# A bug panics: calling a method on none, a wrong type, unwrap on an Err...
# Nothing in normal code catches a panic. catch_panic exists for tests and
# boundaries (like Rust's catch_unwind): it gives Ok(value) or Err(panic).
p catch_panic { none.upcase }.err.message
p catch_panic { parse_age("x").unwrap }.err.message
p catch_panic { panic("invariant broken") }.err.message
p catch_panic { 1 + 1 }
