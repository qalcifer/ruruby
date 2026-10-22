//! Syntax added in roadmap step 2 (parser completeness).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn parameters() {
    let src = r##"
        def m(a, b = 2, *rest, c, k:, opt: 5, **kw, &blk)
          [a, b, rest, c, k, opt, kw, blk ? blk.call : nil]
        end
        p m(1, 9, k: 3)
        p m(1, 2, 3, 4, 5, k: 0, z: 1) { :b }
        def d((a, (b, c)), d = a) = [a, b, c, d]
        p d([1, [2, 3]])
        def fwd(...) = m(...)
        p fwd(1, 2, k: 7)
        def kw_only(a:, b: a * 2) = [a, b]
        p kw_only(a: 4)
        p(begin; kw_only; rescue ArgumentError => e; e.message; end)
        p(begin; kw_only(a: 1, x: 2); rescue ArgumentError => e; e.message; end)
    "##;
    assert_eq!(
        run(src),
        // `c` (required, after the rest) is filled before the optional `b`;
        // defaults run before destructuring, so `d = a` sees nil
        "[1, 2, [], 9, 3, 5, {}, nil]\n[1, 2, [3, 4], 5, 0, 5, {z: 1}, :b]\n[1, 2, 3, nil]\n\
         [1, 2, [], 2, 7, 5, {}, nil]\n[4, 8]\n\"missing keyword: :a\"\n\"unknown keyword: :x\"\n"
    );
}

#[test]
fn blocks_and_lambdas() {
    let src = r##"
        p [[1, [2, 3]], [4, [5, 6]]].map { |a, (b, c)| a + b + c }
        p [[1, 2], [3, 4]].map { |a, | a }
        p [1, 2, 3].map { _1 * 10 }, [4, 5].map { it + 1 }
        add = ->(a, b = 1) { a + b }
        p add.(2), add[2, 3], add.lambda?, add.arity
        early = lambda { return 7; 8 }
        p early.call
        p(begin; add.call; rescue ArgumentError => e; e.message; end)
        p proc { |a, b| [a, b] }.call([1, 2]), proc { |x, *y, z| }.arity
    "##;
    assert_eq!(
        run(src),
        "[6, 15]\n[1, 3]\n[10, 20, 30]\n[5, 6]\n3\n5\ntrue\n-2\n7\n\
         \"wrong number of arguments (given 0, expected 1..2)\"\n[1, 2]\n-3\n"
    );
}

#[test]
fn multiple_assignment_and_splats() {
    let src = r##"
        a, (b, *c), d = 1, [2, 3, 4], 5
        p [a, b, c, d]
        *init, last = [1, 2, 3]
        p init, last
        h = {}
        h[:x], h[:y] = 1, 2
        p h
        def args(*a, **k) = [a, k]
        list = [1, 2]
        opts = {z: 1}
        p args(*list, 3, **opts), [*list, *nil, 0], {**opts, y: 2}
        x = 1, 2
        p x
        p args(0, * %w[a b], ** opts)
    "##;
    assert_eq!(
        run(src),
        "[1, 2, [3, 4], 5]\n[1, 2]\n3\n{x: 1, y: 2}\n[[1, 2, 3], {z: 1}]\n[1, 2, 0]\n{z: 1, y: 2}\n[1, 2]\n[[0, \"a\", \"b\"], {z: 1}]\n"
    );
}

#[test]
fn literals() {
    let src = r##"
        p %w[a b\ c], %i(x y), %W(#{1 + 1}x y)
        s = <<~EOS
          indented
            more
        EOS
        p s
        p ?a, "\x41\u{42 43}", 0b101, 0o17, 0x1f, 0_10, 1_000
        name = "x"
        p :"sym_#{name}", {"key": 1, name:}
        def show(s) = p(s)
        show <<EOS
__END__
EOS
        a = [1]
        b = 2
        a <<b
        p a
    "##;
    assert_eq!(
        run(src),
        "[\"a\", \"b c\"]\n[:x, :y]\n[\"2x\", \"y\"]\n\"indented\\n  more\\n\"\n\"a\"\n\"ABC\"\n5\n15\n31\n8\n1000\n\
         :sym_x\n{key: 1, name: \"x\"}\n\"__END__\\n\"\n[1, 2]\n"
    );
}

#[test]
fn classes_and_singletons() {
    let src = r##"
        class Counter
          @@total = 0
          class << self
            attr_accessor :label
            def create = new.tap { @@total += 1 }
            def total = @@total
          end
        end
        Counter.label = "c"
        2.times { Counter.create }
        p Counter.total, Counter.label
        module Outer
          LIMIT = 3
          class Inner
            def limit = LIMIT
          end
        end
        class Outer::Other; end
        p Outer::Inner.new.limit, Outer::Other.name, ::Outer::LIMIT
        o = Object.new
        def o.hi = "hi from o"
        p o.hi, o.singleton_methods
        Named = Class.new(StandardError) { def extra = 1 }
        p Named.name, Named.new.extra, Named.ancestors.include?(StandardError)
        class String
          alias shout upcase
        end
        p "a".shout
    "##;
    assert_eq!(
        run(src),
        "2\n\"c\"\n3\n\"Outer::Other\"\n3\n\"hi from o\"\n[:hi]\n\"Named\"\n1\ntrue\n\"A\"\n"
    );
}

#[test]
fn control_flow() {
    let src = r##"
        r = []
        for i, j in [[1, 2], [3, 4]] do r << i + j end
        p r, i
        n = 0
        begin n += 1 end while n < 3
        p n
        tries = 0
        begin
          tries += 1
          raise "x" if tries < 3
        rescue
          retry
        else
          p [:ok, tries]
        ensure
          p :ensure
        end
        def f
          [1, 2].each { |x| return x * 10 }
        ensure
          p :cleanup
        end
        p f
        p(begin; Proc.new { break }.call; rescue LocalJumpError => e; e.class; end)
        p defined?(foo), defined?(p), defined?(String), defined?(@x), defined?(r), defined?(nil)
        p nil&.length, "ab"&.length
        n = 2
        out = []
        [:a, :b, :c].each { |v| n == 0 ? out << v : n -= 1 }
        p out, n, (true ? m = 1 : 2), m
    "##;
    assert_eq!(
        run(src),
        "[3, 7]\n3\n3\n[:ok, 3]\n:ensure\n:cleanup\n10\nLocalJumpError\nnil\n\"method\"\n\"constant\"\nnil\n\
         \"local-variable\"\n\"nil\"\nnil\n2\n[:c]\n0\n1\n1\n"
    );
}

#[test]
fn pattern_matching() {
    let src = r##"
        def describe(v)
          case v
          in Integer | Float => n if n > 100 then "big #{n}"
          in Integer then "int"
          in [] then "empty"
          in [first, *rest] then "list #{first} +#{rest.size}"
          in {name: String => name, age:} then "#{name}/#{age}"
          in {type: :circle, r:} then "circle #{r}"
          else "other"
          end
        end
        p describe(500), describe(5), describe([]), describe([1, 2, 3])
        p describe({name: "Ada", age: 36}), describe({type: :circle, r: 2}), describe("s")
        config = {db: {user: "admin", port: 5432}}
        config => {db: {user:}}
        p user
        p((1 in Integer), ([1, [2, 3]] in [_, [_, x]]), x)
        case [1, 42, "x", 7]
        in [*, String => s, *post] then p [s, post]
        end
        p(begin; 5 => String; rescue NoMatchingPatternError => e; e.class; end)
    "##;
    assert_eq!(
        run(src),
        "\"big 500\"\n\"int\"\n\"empty\"\n\"list 1 +2\"\n\"Ada/36\"\n\"circle 2\"\n\"other\"\n\"admin\"\n\
         true\ntrue\n3\n[\"x\", [7]]\nNoMatchingPatternError\n"
    );
}

#[test]
fn line_continuation() {
    let src = r##"
        a = [1, 2]
        x = (a.size +
          3)
        p x
        y = a.size -
          1
        p y
        p :mod if
          y == 1
        z = 0
        z += 1 while
          z < 3
        p z
    "##;
    assert_eq!(run(src), "5\n1\n:mod\n3\n");
}

#[test]
fn interpolation_is_lexed_as_code() {
    let src = r##"
        x = 3
        p "a#{ "}" }b", "#{ ?} }", "#{ {k: x}[:k] }", "o#{ "i#{x}i" }o"
        s = "#{ x + # a } in a comment
          1 }"
        p s
        $10 = :ten
        p $10, $1
    "##;
    assert_eq!(run(src), "\"a}b\"\n\"}\"\n\"3\"\n\"oi3io\"\n\"4\"\n:ten\nnil\n");
    let err = run_capture("p \"#{<<A}\"\nx\nA\n").unwrap_err().to_string();
    assert!(err.contains("heredoc can't start inside string interpolation"), "{err}");
}

#[test]
fn operator_precedence() {
    let src = r##"
        p 1 + 2 * 3 - 4 / 2, 5 - 2 - 1, 2 * 3 % 4, 1 << 2 + 1
        p 1 < 2 == true, 1 | 2 & 3, 6 ^ 3 | 8, 2 ** 3 ** 2, -2 ** 2
        p !true && false, 1 == 1 && 2 != 3 || nil, nil || false && 1
        p "ab" != "x" ? 1 : 2, (a = 1 || 2), a
    "##;
    assert_eq!(
        run(src),
        "5\n2\n2\n8\ntrue\n3\n13\n512\n-4\nfalse\ntrue\nfalse\n1\n1\n1\n"
    );
}

#[test]
fn deep_nesting_is_a_syntax_error_not_a_crash() {
    // within the limit, debug builds need a few MiB of stack
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let deep = |open: &str, close: &str, n: usize| {
                format!("p({}1{})", open.repeat(n), close.repeat(n))
            };
            assert_eq!(run(&deep("(", ")", 50)), "1\n");
            assert_eq!(run(&deep("[", "]", 3)), "[[[1]]]\n");
            for src in [
                deep("(", ")", 10_000),
                deep("[", "]", 10_000),
                deep("\"#{", "}\"", 10_000),
                deep("-> { ", " }", 10_000),
                deep("!", "", 10_000),
                format!("p 1{}", " + 1".repeat(10_000)),
                format!("p 1{}", ".abs".repeat(10_000)),
            ] {
                let err = run_capture(&src).unwrap_err().to_string();
                assert!(err.contains("nesting too deep"), "{err}");
            }
            // long but flat code is fine
            assert_eq!(run(&"x = 1\n".repeat(10_000)), "");
            assert_eq!(run(&format!("p [{}].size", "1, ".repeat(10_000))), "10000\n");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn operations_never_clobber_locals() {
    // codegen computes an operation directly in its destination register
    // only when that is a temporary; a local must be read before it's written
    let src = r##"
        x = 2
        x = 1 + x
        y = 5
        y = [y, y * 2, x - y]
        def f(a) = (a = 10 - a; a * a)
        s = "ab"
        s = s + s.upcase
        z = 3
        z = z.times.map { |i| i + z } rescue z = [z, z]
        p x, y, f(3), s, z
    "##;
    assert_eq!(run(src), "3\n[5, 10, -2]\n49\n\"abAB\"\n[3, 3]\n");
}
