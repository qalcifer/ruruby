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
# Absence is `none` (no Some wrapper); a bug is a `panic`, which ends the
# program.

# A failure that isn't an expected outcome. (Becomes uncatchable once
# rescue goes away.)
class Panic < Exception
end

def panic(message = "explicit panic")
  raise Panic, message
end

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
  def err = nil
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
  def ok = nil
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
