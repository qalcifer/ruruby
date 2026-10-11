# begin/rescue/ensure, custom exceptions, retry-free error flow
class ValidationError < StandardError
end

def check(n)
  raise ValidationError, "negative: #{n}" if n < 0
  raise ArgumentError.new("zero") if n == 0
  n
end

[5, -1, 0].each do |n|
  begin
    puts "ok #{check(n)}"
  rescue ValidationError => e
    puts "validation failed: #{e.message}"
  rescue ArgumentError => e
    puts "argument error: #{e.message} (#{e.class})"
  ensure
    puts "checked #{n}"
  end
end

def risky
  yield
rescue ZeroDivisionError => e
  puts "rescued in method: #{e.message}"
  :recovered
end

puts risky { 1 / 0 }
value = Integer("42") rescue -1
puts value
bad = Integer("nope") rescue -1
puts bad

begin
  nil.upcase
rescue NoMethodError => e
  puts e.message
end

begin
  begin
    raise "inner"
  ensure
    puts "inner ensure"
  end
rescue => e
  puts "outer caught: #{e.message}"
end
