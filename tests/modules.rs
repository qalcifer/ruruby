//! lang-fork: files as modules with `use` (src/modules.rs).

use ruruby::Interpreter;
use std::path::{Path, PathBuf};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    // unique per call: tests run in parallel and may share a name
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ruruby-use-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, src) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    dir
}

/// Compiles and runs `main.rb` of the project: its output, or the error.
fn run(dir: &Path) -> Result<String, String> {
    let main = dir.join("main.rb");
    let src = std::fs::read_to_string(&main).unwrap();
    let mut interp = Interpreter::new();
    interp.capture_output();
    let irep = interp.compile_file(&src, &main.to_string_lossy(), 1).map_err(|e| e.to_string())?;
    interp.run(irep).map_err(|e| e.to_string())?;
    Ok(interp.take_output())
}

const PERSON: &str = "
class Person {
  fn init(name) = @name = name
  fn to_s = \"Person(#{@name})\"
}
HELPER = 42
fn shout(s) = s.upcase
$runs = ($runs || 0) + 1
";

#[test]
fn use_imports_named_constants() {
    let dir = project(
        "named",
        &[
            ("person.rb", PERSON),
            ("models/user.rb", "use person::Person\nclass User < Person {\n  fn admin? = false\n}\n"),
            ("shapes.rb", "class Circle { fn area = 3 }\nclass Square { fn area = 4 }\n"),
            (
                "main.rb",
                r#"
use person::Person
use models::user::User
use shapes::{Circle, Square}
puts Person::new("Ann"), User::new("Bob"), User::superclass, Circle::new.area + Square::new.area
p User::new("x").admin?, shout("hi"), $runs
p catch_panic { HELPER }.err.message
p Person::name, User::name
"#,
            ),
        ],
    );
    assert_eq!(
        run(&dir).unwrap(),
        "Person(Ann)\nPerson(Bob)\nperson::Person\n7\nfalse\n\"HI\"\n1\n\"uninitialized constant HELPER\"\n\"person::Person\"\n\"models::user::User\"\n"
    );
}

#[test]
fn use_glob_and_namespaces_stay_apart() {
    let dir = project(
        "glob",
        &[
            ("a.rb", "class Thing { fn who = :a }\nLIMIT = 1\n"),
            ("b.rb", "class Thing { fn who = :b }\n"),
            ("main.rb", "use a::*\np Thing::new.who, LIMIT\nuse b::Thing\np Thing::new.who\n"),
        ],
    );
    assert_eq!(run(&dir).unwrap(), ":a\n1\n:b\n");
}

#[test]
fn use_errors_are_compile_errors() {
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
    let cases: &[Case] = &[
        ("missing-name", &[("person.rb", PERSON), ("main.rb", "puts 1\nuse person::Persn\n")], "line 2: use person::Persn: person.rb doesn't define Persn"),
        ("missing-file", &[("main.rb", "use nofile::X\n")], "use nofile::X: can't read"),
        ("cycle", &[("a.rb", "use b::B\nA = 1\n"), ("b.rb", "use a::A\nB = 1\n"), ("main.rb", "use a::A\n")], "cyclic use (a -> b -> a)"),
        ("nested", &[("main.rb", "fn f() {\n  use person::Person\n}\n")], "use is only allowed among a file's top-level statements"),
        ("syntax", &[("x.rb", "class X {\n  fn (() {\n  }\n"), ("main.rb", "use x::X\n")], "x.rb: syntax error"),
    ];
    for (name, files, expected) in cases {
        let dir = project(name, files);
        // nothing runs when compiling fails: `puts 1` printed nothing
        let err = run(&dir).unwrap_err();
        assert!(err.contains(expected), "{name}: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

const GEOMETRY: &str = "
module Shapes {
  class Circle {
    fn init(r) = @r = r
    fn area = 3 * @r * @r
    KIND = :round
  }
  class Square {
    fn init(s) = @s = s
    fn area = @s * @s
  }
}
module Units {
  CM = 100
  module Imperial {
    INCH = 2.54
  }
}
module Shapes {
  class Triangle { fn area = 1 }
}
";

#[test]
fn use_reaches_into_modules_of_a_file() {
    let dir = project(
        "nested",
        &[
            ("geometry.rb", GEOMETRY),
            (
                "main.rb",
                "use geometry::Shapes::Circle
use geometry::Shapes::{Square, Triangle}
use geometry::Units::Imperial::*
use geometry::Shapes::Circle::KIND
use geometry::{Units}
p Circle::new(2).area, Square::new(3).area, Triangle::new.area, INCH, KIND, Units::CM
p Circle::name, defined?(Shapes)
",
            ),
        ],
    );
    assert_eq!(run(&dir).unwrap(), "12\n9\n1\n2.54\n:round\n100\n\"geometry::Shapes::Circle\"\nnone\n");
    type Case<'a> = (&'a str, &'a str);
    let cases: &[Case] = &[
        ("use geometry::Shapes::Sqare", "geometry.rb doesn't define Shapes::Sqare"),
        ("use geometry::Shape::Circle", "geometry.rb doesn't define Shape"),
        ("use geometry::Units::Imperial::{INCH, FOOT}", "doesn't define Units::Imperial::FOOT"),
        ("use geometry::Units::CM::X", "defines Units::CM as a value, not a module or class"),
    ];
    for (src, expected) in cases {
        let dir = project("nested-bad", &[("geometry.rb", GEOMETRY), ("main.rb", src)]);
        let err = run(&dir).unwrap_err();
        assert!(err.contains(expected), "{src}: {err}");
    }
}

const BASE: &str = "$loads = ($loads || []) << :base\nclass Base { fn kind = :base }\n";

/// A file used by several others runs once (a diamond), a re-export passes a
/// class on, an imported class can be reopened, a class body sees its file's
/// other constants, and require'd files can `use` too.
#[test]
fn use_graphs_and_namespaces() {
    let dir = project(
        "graph",
        &[
            ("base.rb", BASE),
            ("left.rb", "use base::Base\n$loads << :left\nclass Left < Base {  }\n"),
            ("right.rb", "use base::Base\n$loads << :right\nclass Right < Base {  }\n"),
            ("reexport.rb", "use base::Base\n"),
            (
                "consts.rb",
                "LIMIT = 10\nclass Box {\n  fn self.limit = LIMIT\n  fn inner = Helper::new.name\n  class Helper { fn name = :helper }\n}\n",
            ),
            ("lib/req.rb", "use base::Base\nREQ_BASE = Base\n"),
            (
                "main.rb",
                r#"
use left::Left
use right::Right
use reexport::Base
p $loads, Left::superclass.equal?(Right::superclass), Left::superclass.equal?(Base), Base::new.kind
class Base { fn extra = :reopened }
p Left::new.extra
use consts::Box
p Box::limit, Box::new.inner, defined?(LIMIT)
require_relative "lib/req"
p REQ_BASE.equal?(Base)
"#,
            ),
        ],
    );
    assert_eq!(
        run(&dir).unwrap(),
        "[:base, :left, :right]\ntrue\ntrue\n:base\n:reopened\n10\n:helper\nnone\ntrue\n"
    );
}

#[test]
fn use_failures() {
    // a panic while a used file runs ends the program, there
    let dir = project("load-panic", &[("boom.rb", "class Partial {  }\npanic \"boom while loading\"\n"), ("main.rb", "use boom::Partial\np :never\n")]);
    assert_eq!(run(&dir).unwrap_err(), "boom while loading (Panic)");
    // one name, one origin; the same import twice is fine
    let dir = project(
        "clash",
        &[
            ("t1.rb", "class Thing {  }\n"),
            ("t2.rb", "class Thing {  }\n"),
            ("main.rb", "use t1::Thing\nuse t1::Thing\nuse t2::{Thing}\n"),
        ],
    );
    let err = run(&dir).unwrap_err();
    assert!(err.contains("line 3: use t2::Thing: Thing is already imported from t1 (line 1)"), "{err}");
    // `use` names a file and what to import from it
    for (src, expected) in [
        ("use person\n", "use: expected `::`"),
        ("use person::\n", "use: expected a constant, { } or * after `::`"),
        ("use person::{}\n", "use: expected a constant name (or a module's function) in { }"),
        ("use person::shout\n", "a function is imported from a module: file::Module::shout"),
        ("use person::Person::{}\n", "use: expected a constant name"),
        ("if true {\n  use person::Person\n}\n", "use is only allowed among a file's top-level statements"),
    ] {
        let dir = project("syntax-errors", &[("person.rb", PERSON), ("main.rb", src)]);
        let err = run(&dir).unwrap_err();
        assert!(err.contains(expected), "{src:?}: {err}");
    }
}

const UNITS: &str = "
module Units {
  CM = 2.54
  fn to_cm(i) = round2(i * CM)
  fn to_in(c) = round2(c / CM)
  fn self.twice(x) = x * 2
  private fn round2(x) = x.round(2)
  private
  fn hidden(x) = x
}
module Text { fn shout(s) = s.upcase + \"!\" }
";

/// `use file::Module::func` imports a module's function as a bare name, for
/// the whole file (methods and blocks too); `::*` brings its functions with
/// its constants. Imports are per file, like Rust's.
#[test]
fn use_imports_module_functions() {
    let dir = project(
        "fns",
        &[
            ("units.rb", UNITS),
            ("other.rb", "use units::Text::shout\nclass Loud { fn say(s) = shout(s) }\n"),
            (
                "main.rb",
                r##"
use units::Units::{to_cm, CM}
use units::Text::*
use other::Loud
class Ruler {
  fn mark(i) = "#{to_cm(i)} cm"
}
p to_cm(10), CM, shout("hi"), Ruler::new.mark(1), [1, 2].map(|i| to_cm(i)), Loud::new.say("x")
p catch_panic { to_in(1) }.err.class, catch_panic { Units }.err.class
"##,
            ),
        ],
    );
    assert_eq!(
        run(&dir).unwrap(),
        "25.4\n2.54\n\"HI!\"\n\"2.54 cm\"\n[2.54, 5.08]\n\"X!\"\nNoMethodError\nNameError\n"
    );
}

#[test]
fn use_function_errors() {
    let cases = [
        ("use units::Units::nope", "line 1: use units::Units::nope: units.rb doesn't define Units::nope (a public fn of the module)"),
        ("use units::Units::round2", "doesn't define Units::round2"),
        ("use units::Units::hidden", "doesn't define Units::hidden"),
        ("use units::Units::to_cm::X", "expected"),
        ("use units::Text::shout\nfn shout(s) = s", "use units::Text::shout: shout is also defined in this file"),
        ("use units::Text::shout\nuse units::Units::{shout}", "shout is already imported from units::Text"),
    ];
    for (i, (main, want)) in cases.iter().enumerate() {
        let dir = project(&format!("fnerr{i}"), &[("units.rb", UNITS), ("main.rb", main)]);
        let err = run(&dir).unwrap_err();
        assert!(err.contains(want), "{main}:\n  {err}\n  wanted: {want}");
    }
}

/// An enum's variants are imported like constants; a variant with fields
/// also as its constructor: `use shapes::Shape::*` then `Circle(2)`.
#[test]
fn use_imports_enum_variants() {
    let shapes = "
enum Shape {
  Circle(r)
  Rect { w, h }
  Empty
  fn area() = case self { in Circle(r) { 3 * r * r } in Rect(w:, h:) { w * h } in Empty { 0 } }
}
";
    let dir = project(
        "enums",
        &[
            ("shapes.rb", shapes),
            ("other.rb", "use shapes::Shape::{Circle, Empty}\nfn small = [Circle(1), Empty]\n"),
            (
                "main.rb",
                r#"
use shapes::Shape::*
use shapes::Shape
use other::*
fn kind(s) = case s { in Circle(_) { :circle } in Empty { :empty } else { :other } }
p Circle(2).area, Rect(w: 2, h: 3), Empty, kind(Circle(1)), kind(Empty), kind(Shape::Rect(w: 1, h: 1)), small
"#,
            ),
        ],
    );
    assert_eq!(run(&dir).unwrap(), "12\nRect(w: 2, h: 3)\nEmpty\n:circle\n:empty\n:other\n[Circle(1), Empty]\n");
    let dir = project("enumerr", &[("shapes.rb", shapes), ("main.rb", "use shapes::Shape::Square\n")]);
    assert!(run(&dir).unwrap_err().contains("shapes.rb doesn't define Shape::Square"));
    let dir = project("enumerr2", &[("shapes.rb", shapes), ("main.rb", "use shapes::Shape::Circle::X\n")]);
    assert!(run(&dir).unwrap_err().contains("defines Shape::Circle as an enum variant"));
}

/// The exhaustiveness check knows the enums a file imports, and checks the
/// used files themselves.
#[test]
fn use_checks_cases_on_imported_enums() {
    let shapes = "enum Shape {\n  Circle(r)\n  Empty\n}\n";
    let cases = [
        ("use shapes::Shape\nfn f(s) = case s { in Shape::Circle(r) { r } }\n", "line 2: this case on Shape doesn't handle Empty"),
        ("use shapes::Shape::*\nfn f(s) = case s { in Empty { 0 } }\n", "line 2: this case on Shape doesn't handle Circle"),
        ("use shapes::Shape::{Circle}\nfn f(s) = case s { in Circle(r) { r } }\n", "line 2: this case on Shape doesn't handle Empty"),
        ("use other::Thing\n", "other.rb:3: this case on Shape doesn't handle Empty"),
    ];
    let other = "use shapes::Shape\nclass Thing {\n  fn f(s) = case s { in Shape::Circle(r) { r } }\n}\n";
    for (i, (main, want)) in cases.iter().enumerate() {
        let dir = project(&format!("exh{i}"), &[("shapes.rb", shapes), ("other.rb", other), ("main.rb", main)]);
        let err = run(&dir).unwrap_err();
        assert!(err.contains(want), "{main}:\n  {err}\n  wanted: {want}");
    }
    let dir = project("exhok", &[("shapes.rb", shapes), ("main.rb", "use shapes::Shape::*\nfn f(s) = case s { in Circle(r) { r } in Empty { 0 } }\np f(Circle(4))\n")]);
    assert_eq!(run(&dir).unwrap(), "4\n");
}
