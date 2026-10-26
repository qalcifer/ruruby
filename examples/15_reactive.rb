# 15 — Reactivity: effects and bindings
#
# The core of the declarative UI to come: code that reads props re-runs by
# itself when they change. Nothing lists its dependencies — what a block
# reads is what it depends on.

class Counter {
  prop mut count = 0
  prop mut step = 1
  fn inc() { @count += step }        # writing the field notifies too
}

class Label {
  prop mut text = ""
}

counter = Counter::new
label = Label::new

# effect { } runs now, then again after each change of what it read
log = effect { puts "count is #{counter.count}" }

# bind(object, :prop) { } keeps a prop equal to the block's value
bind(label, :text) { "Count: #{counter.count} (step #{counter.step})" }
p label.text

counter.inc
counter.step = 5
counter.inc
p label.text

# Writing the same value changes nothing
counter.count = counter.count

# batch { } holds the updates until the end: each dependent runs once
batch {
  counter.count = 100
  counter.step = 10
}
p label.text

# Dependencies are what the last run read: they follow the branches
class Settings {
  prop mut metric = true
  prop mut km = 5.0
  prop mut miles = 3.1
}
s = Settings::new
effect { puts(if s.metric { "#{s.km} km" } else { "#{s.miles} mi" }) }
s.miles = 3.2            # not read now: no update
s.metric = false
s.km = 6.0               # not read any more: no update

# untracked { } reads without depending
effect { puts "count #{counter.count}, step was #{untracked { counter.step }}" }
counter.step = 2         # no update
counter.count = 1

# An effect stops when told to
log.stop
counter.count = 2
p log, Reactive::stats   # [live computations, watched fields]

# A binding that feeds its own input is a bug, caught
loop_counter = Counter::new
p catch_panic { bind(loop_counter, :count) { loop_counter.count + 1 } }.err.message
