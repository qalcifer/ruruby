# 12 — A small program: an inventory driven by text commands
#
# Ties the tour together: classes, closures, Results with `?`, pattern
# matching, hashes and string formatting.

class Inventory {
  fn init = @items = Hash.new(0)

  fn add(name, count) {
    return Err("count must be positive") unless count > 0
    @items[name] += count
    Ok(@items[name])
  }

  fn remove(name, count) {
    have = @items.fetch(name).map_err(|_| "no #{name} in stock")?
    return Err("only #{have} #{name} left") if count > have
    @items[name] = have - count
    @items.delete(name) if @items[name].zero?
    Ok(have - count)
  }

  fn report = @items.sort.map(|name, n| "#{name.ljust(8)}#{n.to_s.rjust(4)}").join("\n")
}

# "add apples 5" → Ok([:add, "apples", 5]); anything else → Err(reason)
fn parse(line) {
  words = line.split
  return Err("empty line") if words.empty?
  case words {
    in ["add" | "remove" => verb, name, count] {
      n = Integer(count).map_err(|_| "#{count.inspect} is not a number")?
      Ok([verb.to_sym, name, n])
    }
    in ["report"] { Ok([:report]) }
    else { Err("unknown command: #{line.inspect}") }
  }
}

fn run(inventory, line) {
  command = parse(line)?
  case command {
    in [:add, name, n] { inventory.add(name, n).map(|total| "#{name}: #{total}") }
    in [:remove, name, n] { inventory.remove(name, n).map(|left| "#{name}: #{left} left") }
    in [:report] { Ok(inventory.report) }
  }
}

script = <<~COMMANDS
  add apples 5
  add pears 2
  remove apples 3
  remove plums 1
  add kiwis many
  remove pears 9
  launch rockets
  add plums 4
  report
COMMANDS

inventory = Inventory.new
script.lines.each { |line|
  case run(inventory, line.strip) {
    in Ok(text) { puts text }
    in Err(why) { puts "  ! #{why}" }
  }
}
