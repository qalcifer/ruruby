# Core iterators written in Ruby (mruby: mrblib/*.rb), loaded after the
# native core and replacing native versions of the same methods.
#
# As bytecode, an iterator's loop runs in the VM's dispatch loop: it calls
# its block without native recursion (so deep recursion through blocks
# needs no native stack) and garbage is collected while it runs. A native
# iterator would keep the loop on the Rust stack until it returns.

class Integer
  def times
    i = 0
    n = self # a local: the loop test compiles to one compare-and-branch
    while i < n
      yield i
      i += 1
    end
    self
  end

  def upto(limit)
    i = self
    while i <= limit
      yield i
      i += 1
    end
    self
  end

  def downto(limit)
    i = self
    while i >= limit
      yield i
      i -= 1
    end
    self
  end
end

class Array
  def each
    i = 0
    while i < size
      yield self[i]
      i += 1
    end
    self
  end

  def each_index
    i = 0
    while i < size
      yield i
      i += 1
    end
    self
  end

  def each_with_index
    i = 0
    while i < size
      yield self[i], i
      i += 1
    end
    self
  end

  def map
    out = []
    i = 0
    while i < size
      out << yield(self[i])
      i += 1
    end
    out
  end
  alias collect map

  def select
    out = []
    i = 0
    while i < size
      x = self[i]
      out << x if yield(x)
      i += 1
    end
    out
  end
  alias filter select

  def reject
    out = []
    i = 0
    while i < size
      x = self[i]
      out << x unless yield(x)
      i += 1
    end
    out
  end
end

class Hash
  # iterates over a snapshot, so the block may change the hash
  def each
    to_a.each { |pair| yield pair }
    self
  end
  alias each_pair each

  def each_key
    keys.each { |k| yield k }
    self
  end

  def each_value
    values.each { |v| yield v }
    self
  end
end

class Range
  def each
    lo = self.begin
    hi = self.end
    if lo.is_a?(Integer) && hi.is_a?(Integer)
      hi -= 1 if exclude_end?
      i = lo
      while i <= hi
        yield i
        i += 1
      end
    else
      to_a.each { |x| yield x }
    end
    self
  end
end

module Kernel
  # lang-fork: no StopIteration to end it; `break` does
  def loop
    while true
      yield
    end
  end
end
