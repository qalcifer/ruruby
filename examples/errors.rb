# Errors (lang-fork): expected failures are values, Ok(v) / Err(e); `?`
# hands an Err (or none) back to the caller; bugs are panics, which end the
# program. catch_panic { } is for boundaries and tests.
fn check(n) {
  return Err("negative: #{n}") if n < 0
  return Err("zero") if n == 0
  Ok(n)
}

[5, -1, 0].each { |n|
  case check(n) {
  in Ok(v) { puts "ok #{v}" }
  in Err(e) { puts "invalid: #{e}" }
  }
}

fn doubled(n) {
  v = check(n)?
  Ok(v * 2)
}
p doubled(4), doubled(-2)

# none is the absent value; `?` returns it too
fn first_even(xs) = xs.find(&:even?)
fn half_of_first_even(xs) {
  n = first_even(xs)?
  n / 2
}
p half_of_first_even([1, 3, 8]), half_of_first_even([1, 3])

p check(7).map { |v| v + 1 }, check(0).unwrap_or(1), check(-3).map_err(&:upcase)
r = catch_panic { none.upcase }
puts "a bug panics: #{r.err.message}"
