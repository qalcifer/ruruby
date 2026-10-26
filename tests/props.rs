//! lang-fork-ui: `prop` (src/core/props.rs).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const PERSON: &str = r##"
    class Person {
      prop name
      prop mut age: Integer = 0
      prop tags = []
      prop mut email: String = "" {
        set(v) {
          panic("bad email: #{v}") unless v.empty? || v.include?("@")
          @email = v
        }
      }
      prop mut score = 0 {
        changed(old) { puts "score #{old} -> #{score}" }
      }
      prop label { get() = "#{name} (#{age})" }
      fn adult?() = age >= 18
      fn rename(n) { self.name = n; self }
    }
"##;

/// Declaring, reading and writing; defaults (fresh per object); the init
/// a class with props gets; computed props; uniform access.
#[test]
fn props() {
    let out = run(&format!("{PERSON}{}", r##"
        a = Person::new(name: "Ann", age: 30)
        b = Person::new(name: "Bob")
        p a, b, a.label, a.adult?, a.tags.equal?(b.tags), Person::props
        a.age += 1
        a.tags << :x
        p a.age, a.rename("Cy").name, a.tags, b.tags
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#"#<Person @name="Ann", @age=30, @tags=[], @email="", @score=0>"#,
        r#"#<Person @name="Bob", @age=0, @tags=[], @email="", @score=0>"#,
        r#""Ann (30)""#,
        "true",
        "false",
        "[:name, :age, :tags, :email, :score, :label]",
        "31",
        r#""Cy""#,
        "[:x]",
        "[]",
    ]);
}

/// `set` validates, `changed` runs after a real change (not for the same
/// value, not while `new` sets the props).
#[test]
fn accessors() {
    let out = run(&format!("{PERSON}{}", r##"
        a = Person::new(name: "Ann", score: 3, email: "a@x.org")
        a.score = 5
        a.score = 5
        a.score = 7
        p a.email, catch_panic { a.email = "nope" }.err.message, a.email
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "score 3 -> 5",
        "score 5 -> 7",
        r#""a@x.org""#,
        r#""bad email: nope""#,
        r#""a@x.org""#,
    ]);
}

#[test]
fn errors() {
    let cases = [
        ("Person::new", "Person::new is missing name"),
        ("Person::new(name: \"x\", zzz: 1)", "Person has no prop :zzz to set"),
        ("Person::new(name: \"x\", label: 1)", "Person has no prop :label to set"),
        ("Person::new(\"x\")", "Person::new takes its props by name: Person::new(prop: value, ...)"),
        ("Person::new(name: \"x\", age: 1.5)", "Person#age expects Integer, got 1.5"),
        ("Person::new(name: \"x\").name = \"y\"", "private method 'name=' called"),
        ("Person::new(name: \"x\").age = \"old\"", "Person#age expects Integer, got \"old\""),
        ("class C { prop a; prop a }", "C declares prop a twice"),
        ("module M { prop a }", "M is a module: props are for classes"),
        ("X = 5\nclass C { prop n: X }", "prop n: 5 is not a class or interface"),
        ("class C { prop n: String = 1 }\nC::new", "C#n expects String, got 1"),
    ];
    for (src, want) in cases {
        let err = run_capture(&format!("{PERSON}{src}")).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
    for (src, want) in [
        ("class C { prop a { get() = 1\n get() = 2 } }", "prop a: `get` given twice"),
        ("class C { prop a { set() { } } }", "prop a: write `set(value)`"),
        ("class C { prop a { size() = 1 } }", "prop: expected `get()`, `set(v)` or `changed(old)`"),
        ("class C { prop mut a { get() = 1 } }", "a prop with only `get` is computed"),
        ("class C { prop ok? }", "`ok?` can't be a prop name"),
        ("interface I { prop a = 1 }", "a prop there is a requirement"),
        ("class C { prop n: 5 }", "prop n: the type is a class or interface name"),
    ] {
        let err = run_capture(src).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
}

/// An explicit init still works (with field parameters); subclasses add
/// props; interfaces require props.
#[test]
fn with_init_inheritance_and_interfaces() {
    let out = run(r##"
        class Point {
          prop x
          prop y
          fn init(@x, @y) {}
        }
        class Point3 < Point { prop z = 0 }
        class Tagged { prop mut tag = :none }
        class Item < Tagged { prop name }
        p Point::new(1, 2), Point3::new(1, 2), Point3::props, Item::new(name: "a", tag: :t)
        interface Named {
          prop name
          prop mut nick
          fn greet() = "hi #{nick}"
        }
        class Dog {
          prop name
          prop mut nick = "rex"
          impl Named {}
        }
        p Dog::new(name: "Rex").greet, Dog::implements?(Named)
        class Cat { prop name }
        p catch_panic { impl Named for Cat {} }.err.message
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        "#<Point @x=1, @y=2>",
        "#<Point3 @x=1, @y=2, @z=0>",
        "[:x, :y, :z]",
        r#"#<Item @tag=:t, @name="a">"#,
        r#""hi rex""#,
        "true",
        "\"line 23: Cat doesn't implement Named: missing prop mut nick\"",
    ]);
}
