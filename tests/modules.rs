//! lang-fork: files as modules with `use` (src/modules.rs).

use ruruby::Interpreter;
use std::path::{Path, PathBuf};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ruruby-use-{}-{name}", std::process::id()));
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
class Person
  def initialize(name) = @name = name
  def to_s = \"Person(#{@name})\"
end
HELPER = 42
def shout(s) = s.upcase
$runs = ($runs || 0) + 1
";

#[test]
fn use_imports_named_constants() {
    let dir = project(
        "named",
        &[
            ("person.rb", PERSON),
            ("models/user.rb", "use person::Person\nclass User < Person\n  def admin? = false\nend\n"),
            ("shapes.rb", "class Circle; def area = 3; end\nclass Square; def area = 4; end\n"),
            (
                "main.rb",
                r#"
use person::Person
use models::user::User
use shapes::{Circle, Square}
puts Person.new("Ann"), User.new("Bob"), User.superclass, Circle.new.area + Square.new.area
p User.new("x").admin?, shout("hi"), $runs
p catch_panic { HELPER }.err.message
p Person.name, User.name
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
            ("a.rb", "class Thing; def who = :a; end\nLIMIT = 1\n"),
            ("b.rb", "class Thing; def who = :b; end\n"),
            ("main.rb", "use a::*\np Thing.new.who, LIMIT\nuse b::Thing\np Thing.new.who\n"),
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
        ("nested", &[("main.rb", "def f\n  use person::Person\nend\n")], "use is only allowed among a file's top-level statements"),
        ("syntax", &[("x.rb", "class X\n  def (\nend\n"), ("main.rb", "use x::X\n")], "x.rb: syntax error"),
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
module Shapes
  class Circle
    def initialize(r) = @r = r
    def area = 3 * @r * @r
    KIND = :round
  end
  class Square
    def initialize(s) = @s = s
    def area = @s * @s
  end
end
module Units
  CM = 100
  module Imperial
    INCH = 2.54
  end
end
module Shapes
  class Triangle; def area = 1; end
end
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
p Circle.new(2).area, Square.new(3).area, Triangle.new.area, INCH, KIND, Units::CM
p Circle.name, defined?(Shapes)
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

const BASE: &str = "$loads = ($loads || []) << :base\nclass Base; def kind = :base; end\n";

/// A file used by several others runs once (a diamond), a re-export passes a
/// class on, an imported class can be reopened, a class body sees its file's
/// other constants, and require'd files can `use` too.
#[test]
fn use_graphs_and_namespaces() {
    let dir = project(
        "graph",
        &[
            ("base.rb", BASE),
            ("left.rb", "use base::Base\n$loads << :left\nclass Left < Base; end\n"),
            ("right.rb", "use base::Base\n$loads << :right\nclass Right < Base; end\n"),
            ("reexport.rb", "use base::Base\n"),
            (
                "consts.rb",
                "LIMIT = 10\nclass Box\n  def self.limit = LIMIT\n  def inner = Helper.new.name\n  class Helper; def name = :helper; end\nend\n",
            ),
            ("lib/req.rb", "use base::Base\nREQ_BASE = Base\n"),
            (
                "main.rb",
                r#"
use left::Left
use right::Right
use reexport::Base
p $loads, Left.superclass.equal?(Right.superclass), Left.superclass.equal?(Base), Base.new.kind
class Base; def extra = :reopened; end
p Left.new.extra
use consts::Box
p Box.limit, Box.new.inner, defined?(LIMIT)
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
    let dir = project("load-panic", &[("boom.rb", "class Partial; end\npanic \"boom while loading\"\n"), ("main.rb", "use boom::Partial\np :never\n")]);
    assert_eq!(run(&dir).unwrap_err(), "boom while loading (Panic)");
    // one name, one origin; the same import twice is fine
    let dir = project(
        "clash",
        &[
            ("t1.rb", "class Thing; end\n"),
            ("t2.rb", "class Thing; end\n"),
            ("main.rb", "use t1::Thing\nuse t1::Thing\nuse t2::{Thing}\n"),
        ],
    );
    let err = run(&dir).unwrap_err();
    assert!(err.contains("line 3: use t2::Thing: Thing is already imported from t1 (line 1)"), "{err}");
    // `use` names a file and what to import from it
    for (src, expected) in [
        ("use person\n", "use: expected `::`"),
        ("use person::\n", "use: expected a constant, { } or * after `::`"),
        ("use person::{}\n", "use: expected a constant name in { }"),
        ("if true\n  use person::Person\nend\n", "use is only allowed among a file's top-level statements"),
    ] {
        let dir = project("syntax-errors", &[("person.rb", PERSON), ("main.rb", src)]);
        let err = run(&dir).unwrap_err();
        assert!(err.contains(expected), "{src:?}: {err}");
    }
}
