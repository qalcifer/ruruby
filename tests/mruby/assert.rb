# Test harness for running mruby's test/t/*.rb with ruruby.
#
# Same API as mruby's test/assert.rb, but written in the subset of Ruby that
# ruruby supports today (no splats, no &block params, no alias, no
# `class << obj`), so that results measure the test files, not the harness.
# Once ruruby can load mruby's own assert.rb, switch to it.

$ok_test = 0
$ko_test = 0
$kill_test = 0
$skip_test = 0
$warning_test = 0
$asserts = []
$mrbtest_failures = nil
$mrbtest_count = nil
$mrbtest_verbose = false
$undefined_arg = Object.new

class MRubyTestSkip < StandardError
end

module Mrbtest
  FLOAT_TOLERANCE = 1e-10
end

GEMNAME = "mruby-test"

def t_print(a = nil, b = nil, c = nil)
  nil
end

def assert(str = "assert", iso = "")
  parent_failures = $mrbtest_failures
  parent_count = $mrbtest_count
  $mrbtest_failures = []
  $mrbtest_count = 0
  begin
    yield
    if $mrbtest_failures.size > 0
      $ko_test += 1
      $asserts << "Fail: #{str}\n#{$mrbtest_failures.join("\n")}"
    elsif $mrbtest_count == 0
      $warning_test += 1
    else
      $ok_test += 1
    end
  rescue MRubyTestSkip => e
    $skip_test += 1
  rescue Exception => e
    $kill_test += 1
    $asserts << "#{e.class}: #{str} => #{e.message}"
  ensure
    $mrbtest_failures = parent_failures
    $mrbtest_count = parent_count
    $mrbtest_count += 1 if $mrbtest_count
  end
  nil
end

def skip(cause = "")
  raise MRubyTestSkip.new(cause)
end

def assert_true(obj, msg = nil, diff = nil)
  if $mrbtest_failures
    $mrbtest_count += 1
    unless obj == true
      diff = "    Expected #{obj.inspect} to be true." unless diff
      $mrbtest_failures << " - Assertion[#{$mrbtest_count}] #{msg}\n#{diff}"
    end
  end
  obj
end

def assert_false(obj, msg = nil, diff = nil)
  diff = "    Expected #{obj.inspect} to be false." if !diff && obj != false
  assert_true(!obj, msg, diff)
end

def pass
  assert_true(true)
end

def flunk(msg = "Epic Fail!", diff = "")
  assert_true(false, msg, diff)
end

def assertion_diff(exp, act)
  "    Expected: #{exp.inspect}\n      Actual: #{act.inspect}"
end

def assert_equal(exp, act_or_msg = nil, msg = nil)
  if block_given?
    act = yield
    msg = act_or_msg
  else
    act = act_or_msg
  end
  ret = exp == act
  assert_true(ret, msg, ret ? nil : assertion_diff(exp, act))
end

def assert_not_equal(exp, act_or_msg = nil, msg = nil)
  if block_given?
    act = yield
    msg = act_or_msg
  else
    act = act_or_msg
  end
  ret = exp == act
  assert_true(!ret, msg, ret ? "    Expected #{act.inspect} to not be equal to #{exp.inspect}." : nil)
end

def assert_same(exp, act, msg = nil)
  ret = exp.equal?(act)
  assert_true(ret, msg, ret ? nil : "    Expected #{act.inspect} to be the same as #{exp.inspect}.")
end

def assert_not_same(exp, act, msg = nil)
  ret = !exp.equal?(act)
  assert_true(ret, msg, ret ? nil : "    Expected #{act.inspect} to not be the same as #{exp.inspect}.")
end

def assert_nil(obj, msg = nil)
  ret = obj.nil?
  assert_true(ret, msg, ret ? nil : "    Expected #{obj.inspect} to be nil.")
end

def assert_not_nil(obj, msg = nil)
  ret = !obj.nil?
  assert_true(ret, msg, ret ? nil : "    Expected #{obj.inspect} to not be nil.")
end

def assert_include(collection, obj, msg = nil)
  ret = collection.include?(obj)
  assert_true(ret, msg, ret ? nil : "    Expected #{collection.inspect} to include #{obj.inspect}.")
end

def assert_not_include(collection, obj, msg = nil)
  ret = !collection.include?(obj)
  assert_true(ret, msg, ret ? nil : "    Expected #{collection.inspect} to not include #{obj.inspect}.")
end

def assert_predicate(obj, op, msg = nil)
  ret = obj.__send__(op) == true
  assert_true(ret, msg, ret ? nil : "    Expected #{obj.inspect} to be #{op}.")
end

def assert_not_predicate(obj, op, msg = nil)
  ret = obj.__send__(op) == false
  assert_true(ret, msg, ret ? nil : "    Expected #{obj.inspect} to not be #{op}.")
end

def assert_operator(obj1, op, obj2 = $undefined_arg, msg = nil)
  return assert_predicate(obj1, op, msg) if obj2.equal?($undefined_arg)
  ret = obj1.__send__(op, obj2) == true
  assert_true(ret, msg, ret ? nil : "    Expected #{obj1.inspect} to be #{op} #{obj2.inspect}.")
end

def assert_not_operator(obj1, op, obj2 = $undefined_arg, msg = nil)
  return assert_not_predicate(obj1, op, msg) if obj2.equal?($undefined_arg)
  ret = obj1.__send__(op, obj2) == false
  assert_true(ret, msg, ret ? nil : "    Expected #{obj1.inspect} to not be #{op} #{obj2.inspect}.")
end

# Simplified File.fnmatch: supports `*` and `?` only.
def _str_match?(pattern, str)
  _glob_match(pattern, 0, str, 0)
end

def _glob_match(pat, pi, str, si)
  while pi < pat.size
    c = pat[pi]
    if c == "*"
      k = si
      while k <= str.size
        return true if _glob_match(pat, pi + 1, str, k)
        k += 1
      end
      return false
    end
    return false if si >= str.size
    return false if c != "?" && c != str[si]
    pi += 1
    si += 1
  end
  si == str.size
end

def assert_match(pattern, str, msg = nil)
  ret = _str_match?(pattern, str)
  assert_true(ret, msg, ret ? nil : "    Expected #{pattern.inspect} to match #{str.inspect}.")
end

def assert_not_match(pattern, str, msg = nil)
  ret = !_str_match?(pattern, str)
  assert_true(ret, msg, ret ? nil : "    Expected #{pattern.inspect} to not match #{str.inspect}.")
end

def assert_kind_of(cls, obj, msg = nil)
  ret = obj.kind_of?(cls)
  assert_true(ret, msg, ret ? nil : "    Expected #{obj.inspect} to be a kind of #{cls}, not #{obj.class}.")
end

def assert_float(exp, act, msg = nil)
  e = exp.to_f
  a = act.to_f
  if e.finite? && a.finite? && (e - a).abs > Mrbtest::FLOAT_TOLERANCE
    flunk(msg, "    Expected |#{exp} - #{act}| to be <= #{Mrbtest::FLOAT_TOLERANCE}.")
  elsif ((e.infinite? || a.infinite?) && e != a) || (e.nan? && !a.nan?) || (!e.nan? && a.nan?)
    flunk(msg, "    Expected #{act} to be #{exp}.")
  else
    pass
  end
end

# assert_raise(ExcClass, ..., "optional message") { ... }
def assert_raise(c1 = nil, c2 = nil, c3 = nil, c4 = nil)
  classes = [c1, c2, c3, c4].compact
  msg = nil
  msg = classes.pop if classes.size > 0 && classes.last.is_a?(String)
  classes = [StandardError] if classes.empty?
  begin
    yield
  rescue Exception => e
    if classes.any? { |c| e.is_a?(c) }
      pass
      return e
    end
    flunk(msg, "    #{classes.inspect} exception expected, not\n    Class: <#{e.class}>\n    Message: <#{e.message}>")
    return nil
  end
  flunk(msg, "    #{classes.inspect} expected but nothing was raised.")
  nil
end

def assert_nothing_raised(msg = nil)
  begin
    yield
  rescue Exception => e
    flunk(msg, "    Exception raised:\n    Class: <#{e.class}>\n    Message: <#{e.message}>")
    return
  end
  pass
end

def assert_raise_with_message(exc, exp_msg, msg = nil)
  e = assert_raise(exc) { yield }
  return e unless e
  ret = exp_msg == e.message
  assert_true(ret, msg, ret ? nil : assertion_diff(exp_msg, e.message))
end

def assert_raise_with_message_pattern(exc, exp_msg, msg = nil)
  e = assert_raise(exc) { yield }
  return e unless e
  ret = _str_match?(exp_msg, e.message)
  assert_true(ret, msg, ret ? nil : "    Expected #{exp_msg.inspect} to match #{e.message.inspect}.")
end

def _mrbtest_stats
  [$ok_test, $ko_test, $kill_test, $skip_test, $warning_test]
end
