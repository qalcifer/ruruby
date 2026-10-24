# Results: a computation either worked, `Ok(value)`, or failed,
# `Err(error)` — Rust's Result. Loaded after prelude.rb.
#
#   fn parse_port(s) {
#     n = Integer(s)?               # an Err goes back to the caller (`?`)
#     n < 65536 ? Ok(n) : Err("port out of range: #{n}")
#   }
#
#   case parse_port(arg) {
#     in Ok(port) { listen(port) }
#     in Err(e)   { puts e }
#   }
#
# Absence is `none` (no Some wrapper); a bug is a `panic` (src/core/panic.rs),
# which ends the program.

class Result {
  fn ok? = false
  fn err? = false
}

class Ok < Result {
  fn initialize(value) {
    @value = value
    freeze
  }

  fn ok? = true
  # the value; for an Err: panics
  fn unwrap = @value
  fn expect(_message) = @value
  fn unwrap_or(_default) = @value
  fn unwrap_or_else = @value
  fn unwrap_err = panic("called unwrap_err on #{inspect}")
  # the value, or none for an Err
  fn ok = @value
  fn err = none
  fn map = Ok(yield(@value))
  fn map_err = self
  # the block returns the next Result
  fn and_then = yield(@value)
  fn or_else = self
  fn deconstruct = [@value]
  fn ==(other) = other.is_a?(Ok) && @value == other.ok
  fn eql?(other) = other.is_a?(Ok) && @value.eql?(other.ok)
  fn hash = [:ok, @value].hash
  fn inspect = "Ok(#{@value.inspect})"
  fn to_s = inspect
}

class Err < Result {
  fn initialize(error) {
    @error = error
    freeze
  }

  fn err? = true
  fn unwrap = panic("called unwrap on #{inspect}")
  fn expect(message) = panic("#{message}: #{@error.inspect}")
  fn unwrap_or(default) = default
  fn unwrap_or_else = yield(@error)
  fn unwrap_err = @error
  fn ok = none
  fn err = @error
  fn map = self
  fn map_err = Err(yield(@error))
  fn and_then = self
  # the block returns the Result to use instead
  fn or_else = yield(@error)
  fn deconstruct = [@error]
  fn ==(other) = other.is_a?(Err) && @error == other.err
  fn eql?(other) = other.is_a?(Err) && @error.eql?(other.err)
  fn hash = [:err, @error].hash
  fn inspect = "Err(#{@error.inspect})"
  fn to_s = inspect
}

fn Ok(value) = Ok.new(value)
fn Err(error) = Err.new(error)

# Lookups that can miss and conversions that can fail give a Result instead
# of panicking (a default or a block can't miss: Ok of it).
class Hash {
  fn fetch(key, *default) {
    return Ok(self[key]) if key?(key)
    return Ok(yield(key)) if block_given?
    return Ok(default[0]) unless default.empty?
    Err("key not found: #{key.inspect}")
  }
}

class Array {
  fn fetch(index, *default) {
    i = index < 0 ? index + size : index
    return Ok(self[i]) if i >= 0 && i < size
    return Ok(yield(index)) if block_given?
    return Ok(default[0]) unless default.empty?
    Err("index #{index} outside of array bounds: #{-size}...#{size}")
  }
}

class Object {
  alias_method :__integer, :Integer
  alias_method :__float, :Float
  private :__integer, :__float

  # Ok(n), or Err("invalid value for Integer(): \"x\"")
  fn Integer(value) {
    r = catch_panic { __integer(value) }
    r.ok? ? r : Err(r.err.message)
  }

  fn Float(value) {
    r = catch_panic { __float(value) }
    r.ok? ? r : Err(r.err.message)
  }
}
