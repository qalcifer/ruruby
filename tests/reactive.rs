//! lang-fork-ui: effects, bindings, batches (src/reactive.rs, src/core/reactive.rs).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const MODEL: &str = r##"
    class Counter {
      prop mut count = 0
      prop mut step = 1
      fn inc() { @count += step }        # a direct write notifies too
    }
    class Label { prop mut text = "" }
"##;

/// An effect runs now and after each change of what it read; a binding keeps
/// a prop up to date; writing the same value changes nothing; stop ends it.
#[test]
fn effects_and_bindings() {
    let out = run(&format!("{MODEL}{}", r##"
        c = Counter::new
        l = Label::new
        e = effect { puts "effect #{c.count}" }
        bind(l, :text) { "Count: #{c.count}" }
        p l.text
        c.count = 5
        c.inc
        c.count = 6
        p l.text, e, e.active?
        e.stop
        c.count = 1
        p l.text, e, e.active?
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "effect 0",
        r#""Count: 0""#,
        "effect 5",
        "effect 6",
        r#""Count: 6""#,
        "#<Effect active>",
        "true",
        r#""Count: 1""#,
        "#<Effect stopped>",
        "false",
    ]);
}

/// Dependencies are what the last run read, so they follow branches;
/// untracked reads aren't dependencies; batch runs each dependent once.
#[test]
fn dependencies() {
    let out = run(&format!("{MODEL}{}", r##"
        class Flag { prop mut on = true; prop mut a = "A"; prop mut b = "B" }
        f = Flag::new
        effect { puts "shows #{if f.on { f.a } else { f.b }}" }
        f.b = "B2"                     # not read yet: nothing
        f.on = false
        f.a = "A2"                     # not read any more: nothing
        f.b = "B3"
        c = Counter::new
        effect { puts "count #{c.count}, step #{untracked { c.step }}" }
        c.step = 5                     # untracked: nothing
        c.count = 2
        batch { c.count = 3; c.step = 7; c.count = 4 }
        p batch { 42 }
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "shows A",
        "shows B2",
        "shows B3",
        "count 0, step 1",
        "count 2, step 5",
        "count 4, step 7",
        "42",
    ]);
}

/// Bindings feeding bindings, in creation order, once per change; an effect
/// made inside an effect belongs to it; cycles and bad bindings are errors.
#[test]
fn chains_ownership_and_errors() {
    let out = run(&format!("{MODEL}{}", r##"
        class Box { prop mut n = 0; prop mut double = 0; prop mut label = "" }
        b = Box::new
        bind(b, :double) { b.n * 2 }
        bind(b, :label) { "#{b.n} -> #{b.double}" }
        effect { puts b.label }
        b.n = 3
        outer = effect { x = b.n; effect { puts "inner #{x}" } }
        b.n = 4
        p Reactive::stats
        outer.stop
        p Reactive::stats
        p catch_panic { bind(b, :n) { b.n + 1 } }.err.message
        p catch_panic { bind(b, :nope) { 1 } }.err.message
        p catch_panic { effect { panic("in effect") } }.err.message, Reactive::stats
        p catch_panic { bind(b, :n) { "x" } }.ok.inspect
    "##));
    let lines: Vec<&str> = out.lines().filter(|l| !l.starts_with(|c: char| c.is_ascii_digit()) || l.contains("->")).collect();
    assert_eq!(lines[..4], ["0 -> 0", "3 -> 6", "inner 3", "4 -> 8"]);
    assert!(out.contains("inner 4\n[5, 3]\n[3, 3]\n"), "{out}");
    assert!(out.contains("\"reactive cycle: a computation ran 100 times for one change"), "{out}");
    assert!(out.contains("\"bind: #<Box @n="), "{out}");
    assert!(out.contains("has no writer nope=\"\n\"in effect\"\n"), "{out}");
    assert!(out.ends_with("\"#<Effect active>\"\n"), "{out}");
}

/// The GC keeps what effects use; objects nobody watches pay nothing.
#[test]
fn gc_and_unobserved_objects() {
    let out = run(&format!("{MODEL}{}", r##"
        GC.stress = true
        fn make() {
          c = Counter::new
          l = Label::new
          bind(l, :text) { "n=#{c.count}" }
          [c, l]
        }
        c, l = make
        GC.start
        c.count = 9
        p l.text
        plain = Counter::new
        i = 0
        while i < 1000 { plain.count += 1; i += 1 }
        p plain.count, Reactive::stats
    "##));
    assert_eq!(out, "\"n=9\"\n1000\n[1, 1]\n");
}
