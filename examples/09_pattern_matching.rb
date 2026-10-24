# 09 — Pattern matching with case/in

fn describe(value) {
  case value {
    in Integer | Float => n if n > 100 { "a big number (#{n})" }
    in Integer { "an integer" }
    in String => s { "the text #{s.inspect}" }
    in [] { "an empty list" }
    in [x] { "a list with just #{x}" }
    in [first, *rest] { "a list starting with #{first}, #{rest.size} more" }
    in {name: String => name, age: Integer => age} { "#{name}, #{age}" }
    in {type: :circle, radius:} { "a circle of radius #{radius}" }
    in none { "nothing" }
    else { "something else" }
  }
}

p describe(500), describe(7), describe("hi"), describe([]), describe([9])
p describe([1, 2, 3]), describe({name: "Ada", age: 36}), describe({type: :circle, radius: 2})
p describe(none), describe(:sym)

# Results are matched the same way
fn report(result) {
  case result {
    in Ok([status, body]) { "#{status}: #{body}" }
    in Ok(value) { "ok: #{value}" }
    in Err(error) { "failed: #{error}" }
  }
}
p report(Ok([200, "hello"])), report(Ok(3)), report(Err("timeout"))

# Find a pattern anywhere in an array
case [1, 42, "x", 7] {
  in [*, String => found, *after] { p [found, after] }
}

# `=>` destructures (and panics if the value doesn't match)
config = {db: {user: "admin", port: 5432}}
config => {db: {user:, port:}}
p user, port

# `in` tests a pattern and gives true or false
p((5 in Integer), ([1, [2, 3]] in [_, [_, _]]), ({a: 1} in {b:}))

# A value no pattern matches is a panic
p catch_panic { case 5 { in String { 1 } } }.err.class
