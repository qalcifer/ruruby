# Core iterators written in the language itself (mruby: mrblib/*.rb),
# loaded after the native core and replacing native versions of the same
# methods.
#
# As bytecode, an iterator's loop runs in the VM's dispatch loop: it calls
# its block without native recursion (so deep recursion through blocks
# needs no native stack) and garbage is collected while it runs. A native
# iterator would keep the loop on the Rust stack until it returns.

class Integer {
  fn times() {
    i = 0
    n = self # a local: the loop test compiles to one compare-and-branch
    while i < n {
      yield i
      i += 1
    }
    self
  }

  fn upto(limit) {
    i = self
    while i <= limit {
      yield i
      i += 1
    }
    self
  }

  fn downto(limit) {
    i = self
    while i >= limit {
      yield i
      i -= 1
    }
    self
  }
}

class Array {
  fn each() {
    i = 0
    while i < size {
      yield self[i]
      i += 1
    }
    self
  }

  fn each_index() {
    i = 0
    while i < size {
      yield i
      i += 1
    }
    self
  }

  fn each_with_index() {
    i = 0
    while i < size {
      yield self[i], i
      i += 1
    }
    self
  }

  fn map() {
    out = []
    i = 0
    while i < size {
      out << yield(self[i])
      i += 1
    }
    out
  }
  alias collect map

  fn select() {
    out = []
    i = 0
    while i < size {
      x = self[i]
      out << x if yield(x)
      i += 1
    }
    out
  }
  alias filter select

  fn reject() {
    out = []
    i = 0
    while i < size {
      x = self[i]
      out << x unless yield(x)
      i += 1
    }
    out
  }
}

class Hash {
  # iterates over a snapshot, so the block may change the hash
  fn each() {
    to_a.each { |pair| yield pair }
    self
  }
  alias each_pair each

  fn each_key() {
    keys.each { |k| yield k }
    self
  }

  fn each_value() {
    values.each { |v| yield v }
    self
  }
}

class Range {
  fn each() {
    lo = self.begin
    hi = self.end
    if lo.is_a?(Integer) && hi.is_a?(Integer) {
      hi -= 1 if exclude_end?
      i = lo
      while i <= hi {
        yield i
        i += 1
      }
    } else {
      to_a.each { |x| yield x }
    }
    self
  }
}

module Kernel {
  # no StopIteration to end it: `break` does
  fn loop() {
    while true {
      yield
    }
  }
}
