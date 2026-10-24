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
        ("missing-file", &[("main.rb", "use nofile::X\n")], "use nofile: can't read"),
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
