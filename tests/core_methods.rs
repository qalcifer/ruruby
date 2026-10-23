//! Core methods added after step 5 (Hash, String, Array, …).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn hash_methods() {
    let out = run(r##"
        h = {a: 1, b: nil, c: 3}
        p h.shift, h.size, {}.shift, h.assoc(:b), h.rassoc(3), h.has_value?(3), h.compact, h.except(:c)
        g = {a: 1, b: 2, c: 3}
        p g.select! { |k, v| v > 1 }.size, g.select! { true }, g.reject! { |k, v| v == 2 }.keys
        p g.keep_if { false }, g.delete(:zz) { |k| "no #{k}" }, g.delete(:zz)
        r = {x: 1}
        p r.replace({y: 2}), Hash.new { |hh, k| k }.default_proc.class, {}.default_proc
        p({a: 1, b: 2}.select(&->((k, v)) { v == 1 }), {a: 1}.reject { |k, v| v == 1 })
        big = {}
        20.times { |i| big[i] = i }
        p (0...20).map { big.shift[0] }
        p big.empty?, (begin; {a: 1}.freeze.shift; rescue FrozenError; :frozen; end)
    "##);
    assert_eq!(
        out,
        "[:a, 1]\n2\nnil\n[:b, nil]\n[:c, 3]\ntrue\n{c: 3}\n{b: nil}\n2\nnil\n[:c]\n{}\n\
         \"no zz\"\nnil\n{y: 2}\nProc\nnil\n{a: 1}\n{}\n\
         [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]\ntrue\n:frozen\n"
    );
}

#[test]
fn string_bang_methods_and_assignment() {
    let out = run(r##"
        s = "Hello"
        p s.upcase!, s, s.upcase!, "abc".reverse!, "ab".capitalize!, "x\n".chomp!, "x".chomp!
        p "abc".sub!("b", "B"), "abc".gsub!("z", "Z"), (begin; "x".freeze.upcase!; rescue FrozenError; :frozen; end)
        a = "abc"; a[0] = "X"
        b = "abc"; b[-1] = "Z"
        c = "abc"; c[1, 3] = "Q"
        d = "abc"; d[0, 0] = "_"
        e = "hello"; e["ll"] = "LL"
        f = "abc"; f[1..] = "!"
        p a, b, c, d, e, f
        p (begin; "abc"[10] = "x"; rescue IndexError => e; e.message; end)
        p (begin; "a"[0] = 1; rescue TypeError => e; e.message; end)
        y = "hello"
        p y.slice!(1..2), y, "abc".insert(1, "_"), "abc".insert(-1, "_"), "b".prepend("a"), "x".clear
    "##);
    assert_eq!(
        out,
        "\"HELLO\"\n\"HELLO\"\nnil\n\"cba\"\n\"Ab\"\n\"x\"\nnil\n\"aBc\"\nnil\n:frozen\n\
         \"Xbc\"\n\"abZ\"\n\"aQ\"\n\"_abc\"\n\"heLLo\"\n\"a!\"\n\"index 10 out of string\"\n\
         \"no implicit conversion of Integer into String\"\n\"el\"\n\"hlo\"\n\"a_bc\"\n\"abc_\"\n\"ab\"\n\"\"\n"
    );
}

#[test]
fn string_search_bytes_and_substitution() {
    let out = run(r##"
        p "abcabc".rindex("a"), "abcabc".rindex("a", 1), "abcabc".rindex(""), "abc".rindex("a", -4)
        p "あいうあ".rindex("あ", -1), "abcabc".index("a", 1), "hello".index("", 6), "hello".index("l", -2)
        p "héllo".getbyte(1), "abc".byteslice(1, 2), "abc".byteslice(1..), "héllo".byteindex("l")
        p "abcab".byterindex("ab"), "abc".bytesplice(1, 1, "ZZ")
        p (begin; "héllo".bytesplice(2, 1, ""); rescue IndexError => e; e.message; end)
        x = "abc"; x.setbyte(0, 65)
        r = []; "ab".each_byte { r << _1 }
        p x, r
        p "a".to_i(16), "100".to_i(2), "1_2__3".to_i, "_68".to_i, " 6 8 ".to_i, "-42x".to_i
        s = "abXcdXef"
        p s.sub("X", "<\\\\>"), s.sub("X", "<\\&>"), s.sub("X", "[\\`]"), s.gsub("X") { |m| m.downcase }
        p s.gsub("X", "X" => "!"), "abc".gsub("", "."), "abc".sub("", "."), "abc"[1.1], "a" * 2.1
        p (begin; "" * Float::INFINITY; rescue RangeError; :range; end)
    "##);
    assert_eq!(
        out,
        "3\n0\n6\nnil\n3\n3\nnil\n3\n195\n\"bc\"\n\"bc\"\n3\n3\n\"aZZc\"\n\
         \"offset doesn't land on character boundary\"\n\"Abc\"\n[97, 98]\n10\n4\n12\n0\n6\n-42\n\
         \"ab<\\\\>cdXef\"\n\"ab<X>cdXef\"\n\"ab[ab]cdXef\"\n\"abxcdxef\"\n\"ab!cd!ef\"\n\".a.b.c.\"\n\
         \".abc\"\n\"b\"\n\"aa\"\n:range\n"
    );
}
