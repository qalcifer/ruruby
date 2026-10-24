# Results (lang-fork): a computation either worked, `Ok(value)`, or failed,
# `Err(error)` — Rust's Result. Loaded after prelude.rb.
#
#   def parse_port(s)
#     n = Integer(s)?               # an Err goes back to the caller (`?`)
#     n < 65536 ? Ok(n) : Err("port out of range: #{n}")
#   end
#
#   case parse_port(arg)
#   in Ok(port) then listen(port)
#   in Err(e)   then puts e
#   end
#
# Absence is `none` (no Some wrapper); a bug is a `panic` (src/core/panic.rs),
# which ends the program.

class Result
  def ok? = false
  def err? = false
end

class Ok < Result
  def initialize(value)
    @value = value
    freeze
  end

  def ok? = true
  # the value; for an Err: panics
  def unwrap = @value
  def expect(_message) = @value
  def unwrap_or(_default) = @value
  def unwrap_or_else = @value
  def unwrap_err = panic("called unwrap_err on #{inspect}")
  # the value, or none for an Err
  def ok = @value
  def err = none
  def map = Ok(yield(@value))
  def map_err = self
  # the block returns the next Result
  def and_then = yield(@value)
  def or_else = self
  def deconstruct = [@value]
  def ==(other) = other.is_a?(Ok) && @value == other.ok
  def eql?(other) = other.is_a?(Ok) && @value.eql?(other.ok)
  def hash = [:ok, @value].hash
  def inspect = "Ok(#{@value.inspect})"
  def to_s = inspect
end

class Err < Result
  def initialize(error)
    @error = error
    freeze
  end

  def err? = true
  def unwrap = panic("called unwrap on #{inspect}")
  def expect(message) = panic("#{message}: #{@error.inspect}")
  def unwrap_or(default) = default
  def unwrap_or_else = yield(@error)
  def unwrap_err = @error
  def ok = none
  def err = @error
  def map = self
  def map_err = Err(yield(@error))
  def and_then = self
  # the block returns the Result to use instead
  def or_else = yield(@error)
  def deconstruct = [@error]
  def ==(other) = other.is_a?(Err) && @error == other.err
  def eql?(other) = other.is_a?(Err) && @error.eql?(other.err)
  def hash = [:err, @error].hash
  def inspect = "Err(#{@error.inspect})"
  def to_s = inspect
end

def Ok(value) = Ok.new(value)
def Err(error) = Err.new(error)

# Lookups that can miss and conversions that can fail give a Result instead
# of panicking (a default or a block can't miss: Ok of it).
class Hash
  def fetch(key, *default)
    return Ok(self[key]) if key?(key)
    return Ok(yield(key)) if block_given?
    return Ok(default[0]) unless default.empty?
    Err("key not found: #{key.inspect}")
  end
end

class Array
  def fetch(index, *default)
    i = index < 0 ? index + size : index
    return Ok(self[i]) if i >= 0 && i < size
    return Ok(yield(index)) if block_given?
    return Ok(default[0]) unless default.empty?
    Err("index #{index} outside of array bounds: #{-size}...#{size}")
  end
end

class Object
  alias_method :__integer, :Integer
  alias_method :__float, :Float
  private :__integer, :__float

  # Ok(n), or Err("invalid value for Integer(): \"x\"")
  def Integer(value)
    r = catch_panic { __integer(value) }
    r.ok? ? r : Err(r.err.message)
  end

  def Float(value)
    r = catch_panic { __float(value) }
    r.ok? ? r : Err(r.err.message)
  end
end
