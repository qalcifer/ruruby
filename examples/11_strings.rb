# 11 — Strings and symbols

s = "Hello, World"
p s.length, s.upcase, s.downcase, s.swapcase, s.reverse
p s[0], s[-1], s[0, 5], s[7..], s.index("o"), s.count("l")
p s.include?("World"), s.start_with?("Hell"), s.end_with?("!"), s.empty?, "".empty?
p s.sub("World", "there"), s.gsub("o", "0"), s.tr("lo", "01"), s.delete("l")
p s.split(", "), "a,b,,c".split(","), "  padded  ".strip, "x".ljust(3, "."), "7".rjust(3, "0")
p s.chars.first(3), s.bytes.first(2), s.chars.size
p "ab" * 3, "abc" <=> "abd", "a" + "b", "%05.1f|%-4s|%x" % [3.14159, "ab", 255]
p "snake_case_name".split("_").map(&:capitalize).join, "Title Case".downcase.tr(" ", "_")

# Strings are mutable; << appends in place, frozen ones refuse
buf = ""
buf << "con" << "cat"
p buf, buf.frozen?, "lit".frozen?
p catch_panic { "x".freeze << "y" }.err.class

# Interpolation calls to_s
class Temp {
  fn init(@deg) {}
  fn to_s = "#{@deg}°C"
}
puts "It is #{Temp.new(21)} with #{3 + 4} clouds and #{none.inspect} wind"

# Multi-line strings: heredocs (<<~ strips the indentation)
text = <<~TEXT
  Dear #{"Ada"},
    thank you!
TEXT
puts text

# Symbols are interned names
p :name.to_s, "name".to_sym, :a.object_id == :a.object_id, %i[x y z]
p "word count of this sentence".split.size, %w[apples pears].map(&:length)
