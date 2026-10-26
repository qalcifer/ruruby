# A window with a counter: cargo run --release -p ruruby-egui -- examples/ui/counter.rb
use ui::{Window, Column, Row, Text, Button}

component App {
  prop mut count = 0

  Window(title: "Counter", width: 260, height: 140) {
    Column(spacing: 8, padding: 8) {
      Text(text: "Count: #{count}", color: if count < 0 { :red } else { :black })
      Row(spacing: 6) {
        Button(text: "-1") { on(:click) { count -= 1 } }
        Button(text: "+1") { on(:click) { count += 1 } }
        Button(text: "reset", enabled: count != 0) { on(:click) { count = 0 } }
      }
    }
  }
}
