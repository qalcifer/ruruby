# 10 — Collections: Array, Hash, Range

# Arrays
nums = [5, 3, 8, 1]
p nums.sort, nums.max, nums.min, nums.sum, nums.reverse, nums.include?(8)
p nums.map(|n| n * n), nums.select(|n| n > 2), nums.reject(&:odd?), nums.count(&:odd?)
p nums.sort_by(|n| -n), nums.min_by(|n| (n - 4).abs), nums.partition(&:even?)
pairs = []
nums.each_slice(2) { |pair| pairs << pair }
weighted = []
nums.each_with_index { |n, i| weighted << n * i }
p pairs, weighted, nums.zip(["a", "b", "c", "d"])
p nums.first(2), nums.last(2), nums.take_while(|n| n > 2), nums.find(|n| n > 4)
p [1, [2, [3, 4]]].flatten, [1, 1, 2, 3, 3].uniq, [3, 1, 2].sort.reverse, [1, 2, 3].join("-")
p nums.inject(|a, b| a * b), nums.group_by(&:even?)

list = []
list << 1 << 2
list.push(3)
list.unshift(0)
p list                                       # [0, 1, 2, 3]
last = list.pop
first = list.shift
p last, first, list, list.empty?, [].empty?

# Hashes keep insertion order
stock = {apples: 3, pears: 0, plums: 7}
stock[:kiwis] = 2
p stock.keys, stock.values, stock.size, stock.key?(:pears), stock[:figs]
p stock.select(|_, n| n > 0), stock.map(|name, n| "#{name}=#{n}"), stock.sum(|_, n| n)
p stock.sort_by(|_, n| -n).first, stock.min_by(|_, n| n), stock.to_a.first
p stock.transform_values(|n| n * 10), stock.filter_map(|name, n| n.zero? ? name : none)
stock.delete(:pears)
p stock, stock.invert, stock.any?(|_, n| n > 5)

counts = Hash.new(0)                         # a default for missing keys
"the cat saw the dog".split.each { |w| counts[w] += 1 }
p counts, counts.max_by(|_, n| n)

# Ranges
steps = []
(1..10).step(3) { |i| steps << i }
p (1..5).to_a, (1...5).size, ("a".."e").to_a, steps
p (1..10).select(&:even?), (1..4).map(|i| i * i), (1..100).sum, (5..1).to_a
