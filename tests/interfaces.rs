//! lang-fork: `interface` and `impl` (src/core/interface.rs).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

const SHAPES: &str = r##"
    interface Shape {
      fn area()
      fn name() = self.class.name
      fn describe() = "#{name}: area #{area}"
    }

    interface Solid: Shape {
      fn volume()
    }
"##;

/// Required methods, defaults, overriding a default, both impl forms,
/// super-interfaces, `super` into a default, built-in classes, reflection.
#[test]
fn implementing_interfaces() {
    let out = run(&format!("{SHAPES}{}", r##"
        class Circle {
          fn init(r) = @r = r
        }
        impl Shape for Circle {
          fn area() = 3 * @r * @r
        }
        class Square {
          fn init(side) = @side = side
          impl Shape {
            fn area() = @side * @side
            fn name() = "square"
          }
        }
        impl Shape for Integer {
          fn area() = self * self
        }
        class Cube {
          fn init(s) = @s = s
          impl Solid {
            fn area() = 6 * @s * @s
            fn volume() = @s * @s * @s
            fn describe() = "cube: #{super}"
          }
        }
        p Circle::new(1).describe, Square::new(2).describe, 4.describe, Cube::new(1).describe
        p Square::new(1).is_a?(Shape), Cube::new(1).is_a?(Shape), Cube::implements?(Shape), Circle::implements?(Solid)
        p Shape::interface?, Circle::interface?, Cube::ancestors.take(3), [Circle::new(1), 2].sum(&:area)
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""Circle: area 3""#,
        r#""square: area 4""#,
        r#""Integer: area 16""#,
        r#""cube: Cube: area 6""#,
        "true",
        "true",
        "true",
        "false",
        "true",
        "false",
        "[Cube, Solid, Shape]",
        "7",
    ]);
}

/// A requirement can be met by the class body, a superclass, an accessor
/// or another impl; required class methods; constants in interfaces; a
/// method defined in two interfaces is fine when the class defines it.
#[test]
fn meeting_requirements() {
    let out = run(&format!("{SHAPES}{}", r##"
        interface Named {
          fn name()
          fn greet() = "hi #{name}"
        }
        class Person {
          fn init(name) = @name = name
          impl Named { attr_reader :name }
        }
        class Base { fn area() = 9 }
        class Tile < Base { impl Shape {} }
        interface Parse {
          LIMIT = 3
          fn self.parse(s)
          fn twice() = [self, self]
        }
        class Num {
          fn init(n) = @n = n
          attr_reader :n
          impl Parse {
            fn self.parse(s) = new(s.to_i)
          }
        }
        interface Labeled { fn name() = "labeled" }
        class Both {
          fn area() = 1
          fn name() = "both"
          impl Shape {}
          impl Labeled {}
        }
        p Person::new("ann").greet, Tile::new.describe, Num::parse("5").n, Parse::LIMIT, Num::new(1).twice.size
        p Both::new.describe, Both::implements?(Labeled)
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""hi ann""#,
        r#""Tile: area 9""#,
        "5",
        "3",
        "2",
        r#""both: area 1""#,
        "true",
    ]);
}

/// With inheritance: subclasses inherit their parent's interfaces and can
/// override or re-impl; a method a class inherits beats an interface
/// default ("class wins", as in Java and Kotlin); interfaces come after the
/// classes in the ancestors, before Object.
#[test]
fn inheritance() {
    let out = run(&format!("{SHAPES}{}", r##"
        class Rect {
          fn init(w, h) { @w = w; @h = h }
          impl Shape { fn area() = @w * @h }
        }
        class Square < Rect {
          fn init(s) = super(s, s)
          fn area() = super + 1
        }
        class Big < Rect {
          impl Shape { fn name() = "big #{super}" }
        }
        class Base { fn name() = "base" }
        class Tile < Base {
          fn area() = 1
          impl Shape {}
        }
        interface Named { fn name() = "named" }
        class Both < Base {
          fn area() = 2
          impl Shape, Named {}
        }
        p Square::new(2).describe, Square::implements?(Shape), Square::ancestors.take(4)
        p Big::new(1, 1).describe, Tile::new.describe, Tile::ancestors.take(4), Both::new.name
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""Square: area 5""#,
        "true",
        "[Square, Rect, Shape, Object]",
        r#""big Big: area 1""#,
        r#""base: area 1""#,
        "[Tile, Base, Shape, Object]",
        r#""base""#,
    ]);
}

/// `abstract class`: no `new`; bodiless `fn`s are abstract methods; its
/// impl blocks may leave required methods to subclasses. A concrete
/// subclass must provide them all by the end of its class body.
#[test]
fn abstract_classes() {
    let out = run(&format!("{SHAPES}{}", r##"
        abstract class Widget {
          fn init(label) = @label = label
          attr_reader :label
          fn draw()                          # abstract
          fn self.kind()                     # abstract class method
          fn render() = "[#{draw}]"
          impl Shape {}                      # area() left to subclasses
        }
        class Button < Widget {
          fn draw() = "button #{label}"
          fn area() = 4
          fn self.kind() = :control
        }
        abstract class Panel < Widget {      # still abstract: provides some
          fn area() = 0
          fn self.kind() = :container
        }
        class Grid < Panel { fn draw() = "grid" }
        b = Button::new("ok")
        p b.render, b.describe, Grid::new("g").describe, Grid::kind, b.is_a?(Widget), b.is_a?(Shape)
        p Widget::abstract?, Panel::abstract?, Button::abstract?, Grid::implements?(Shape)
        p catch_panic { Widget::new("x") }.err.message, catch_panic { Panel::new("x") }.err.message
    "##));
    assert_eq!(out.lines().collect::<Vec<_>>(), [
        r#""[button ok]""#,
        r#""Button: area 4""#,
        r#""Grid: area 0""#,
        ":container",
        "true",
        "true",
        "true",
        "true",
        "false",
        "true",
        r#""Widget is abstract: create one of its subclasses""#,
        r#""Panel is abstract: create one of its subclasses""#,
    ]);
    let head = format!("{SHAPES}{}", "
        abstract class W {
          fn draw()
          impl Shape {}
        }
");
    let cases = [
        ("class L < W { fn area() = 1 }", "line 16: L doesn't implement abstract class W: missing draw()"),
        ("class L < W { fn draw() = 1 }", "L doesn't implement Shape (from W): missing area()"),
        ("class L < W {
 fn draw(x) = 1
 fn area() = 1
}", "L#draw takes 1 argument, abstract class W declares draw()"),
        ("abstract class V < W {}
class L < V { fn area() = 1 }", "L doesn't implement abstract class W: missing draw()"),
        ("abstract class X {
 fn area(a, b) = 1
 impl Shape {}
}", "X#area takes 2 arguments, Shape declares area()"),
    ];
    for (src, want) in cases {
        let err = run_capture(&format!("{head}{src}")).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
    for (src, want) in [
        ("abstract module M {}", "only classes can be abstract"),
        ("abstract interface I {}", "only classes can be abstract"),
        ("class C { fn f() }", "methods without a body go in an `interface` or `abstract class`"),
    ] {
        let err = run_capture(src).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
}

/// `impl I by @field`: the required methods go to the field; the
/// defaults come from the interface and use them; the class can still
/// define some itself.
#[test]
fn delegation() {
    let out = run(r##"
        interface Stack {
          fn push(x)
          fn size()
          fn last()
          fn empty() = size == 0
          fn top_or(d) = if empty { d } else { last }
        }
        class History {
          fn init() = @items = []
          impl Stack by @items {
            fn size() = @items.size * 10
          }
        }
        h = History::new
        p h.top_or(:none), h.empty
        h.push(1)
        h.push(2)
        p h.top_or(:none), h.size, h.empty, h.respond_to?(:pop)
    "##);
    assert_eq!(out.lines().collect::<Vec<_>>(), [":none", "true", "2", "20", "false", "false"]);
}

/// Broken contracts stop the program when the impl block ends, with the
/// line of the `impl`.
#[test]
fn contract_errors() {
    let cases = [
        ("impl Shape for C { }", "line 14: C doesn't implement Shape: missing area()"),
        ("impl Shape for C { fn area(x) = 1 }", "C#area takes 1 argument, Shape declares area()"),
        ("impl Shape for C {\n fn area() = 1\n fn draw() = 2\n}", "draw() is not part of Shape: define it in the class"),
        ("impl Solid for C { fn volume() = 1 }", "C doesn't implement Shape: missing area()"),
        ("impl Shape, Named for C { fn area() = 1 }", "C gets name() from both Shape and Named: define it in the class"),
        ("impl Shape for C { fn area() = 1 }\nimpl Shape for C {}", "line 15: C already implements Shape"),
        ("impl Shape { fn area() = 1 }", "impl without `for` belongs in a class body"),
        ("impl Shape for Kernel {}", "Kernel is a module; impl is for classes"),
        ("impl C for C {}", "impl C: C is not an interface"),
        ("interface Bad: C {}", "interface Bad: C is not an interface"),
        ("interface Shape {}", "interface Shape is already defined"),
        ("interface Display { fn to_s() }\nimpl Display for C {}", "C doesn't implement Display: missing to_s()"),
        ("interface P { fn self.make(a, b) }\nimpl P for C {}", "C doesn't implement P: missing self.make(a, b)"),
        ("interface Q { fn f(a, *r, k:, &b) }\nimpl Q for C {}", "missing f(a, *r, k:, &b)"),
        ("impl Shape for C {\n fn area() = 1\n fn self.x() = 1\n}", "self.x() is not part of Shape"),
    ];
    let head = format!("{SHAPES}{}", "
        interface Named { fn name() = \"n\" }
        class C { fn init() = none }
");
    for (src, want) in cases {
        let err = run_capture(&format!("{head}{src}")).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
}

/// What an interface or impl block may contain is checked when parsing.
#[test]
fn syntax_errors() {
    let cases = [
        ("interface Bad { @x = 1 }", "`@x` in an interface: interfaces have no state"),
        ("interface Bad { fn f() = @x + 1 }", "`@x` in an interface"),
        ("interface Bad { fn init() }", "interfaces have no `init`"),
        ("interface Bad { fn init(a) = a }", "interfaces have no `init`"),
        ("interface Bad { p 1 }", "an interface holds `fn` declarations and constants"),
        ("interface Bad { fn self.f() = 1 }", "default class methods aren't supported yet"),
        ("interface bad {}", "interface name must be a CONSTANT"),
        ("class C {}\nimpl Shape for C { p 1 }", "an impl block holds the interface's `fn`s"),
        ("impl Shape for C by x", "expected an instance variable"),
        ("impl Shape for C", "impl: expected `{` to start the body"),
        ("fn f()", "fn: expected `(parameters)`"),
    ];
    for (src, want) in cases {
        let err = run_capture(src).unwrap_err().to_string();
        assert!(err.contains(want), "{src}:\n  {err}\n  wanted: {want}");
    }
}
