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
        h = {a: 1, b: none, c: 3}
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
        p big.empty?, (catch_panic { {a: 1}.freeze.shift }.err && :frozen)
    "##);
    assert_eq!(
        out,
        "[:a, 1]\n2\nnone\n[:b, none]\n[:c, 3]\ntrue\n{c: 3}\n{b: none}\n2\nnone\n[:c]\n{}\n\
         \"no zz\"\nnone\n{y: 2}\nProc\nnone\n{a: 1}\n{}\n\
         [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]\ntrue\n:frozen\n"
    );
}

#[test]
fn string_bang_methods_and_assignment() {
    let out = run(r##"
        s = "Hello"
        p s.upcase!, s, s.upcase!, "abc".reverse!, "ab".capitalize!, "x\n".chomp!, "x".chomp!
        p "abc".sub!("b", "B"), "abc".gsub!("z", "Z"), (catch_panic { "x".freeze.upcase! }.err && :frozen)
        a = "abc"; a[0] = "X"
        b = "abc"; b[-1] = "Z"
        c = "abc"; c[1, 3] = "Q"
        d = "abc"; d[0, 0] = "_"
        e = "hello"; e["ll"] = "LL"
        f = "abc"; f[1..] = "!"
        p a, b, c, d, e, f
        p catch_panic { "abc"[10] = "x" }.err.message
        p catch_panic { "a"[0] = 1 }.err.message
        y = "hello"
        p y.slice!(1..2), y, "abc".insert(1, "_"), "abc".insert(-1, "_"), "b".prepend("a"), "x".clear
    "##);
    assert_eq!(
        out,
        "\"HELLO\"\n\"HELLO\"\nnone\n\"cba\"\n\"Ab\"\n\"x\"\nnone\n\"aBc\"\nnone\n:frozen\n\
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
        p catch_panic { "héllo".bytesplice(2, 1, "") }.err.message
        x = "abc"; x.setbyte(0, 65)
        r = []; "ab".each_byte { r << _1 }
        p x, r
        p "a".to_i(16), "100".to_i(2), "1_2__3".to_i, "_68".to_i, " 6 8 ".to_i, "-42x".to_i
        s = "abXcdXef"
        p s.sub("X", "<\\\\>"), s.sub("X", "<\\&>"), s.sub("X", "[\\`]"), s.gsub("X") { |m| m.downcase }
        p s.gsub("X", "X" => "!"), "abc".gsub("", "."), "abc".sub("", "."), "abc"[1.1], "a" * 2.1
        p (catch_panic { "" * Float::INFINITY }.err && :range)
    "##);
    assert_eq!(
        out,
        "3\n0\n6\nnone\n3\n3\nnone\n3\n195\n\"bc\"\n\"bc\"\n3\n3\n\"aZZc\"\n\
         \"offset doesn't land on character boundary\"\n\"Abc\"\n[97, 98]\n10\n4\n12\n0\n6\n-42\n\
         \"ab<\\\\>cdXef\"\n\"ab<X>cdXef\"\n\"ab[ab]cdXef\"\n\"abxcdxef\"\n\"ab!cd!ef\"\n\".a.b.c.\"\n\
         \".abc\"\n\"b\"\n\"aa\"\n:range\n"
    );
}

#[test]
fn array_methods() {
    let out = run(r##"
        p [].send(:initialize, 2), [1, 2, 3].shift(2), [1, 2, 3].pop(2), [].shift(1)
        a = ["a", "b"]
        p a.delete("x") { _1 + "!" }, a.delete("a"), a
        p [1, 2].eql?([1, 2]), [1, 2].eql?([1.0, 2.0]), [1, 2] == [1.0, 2.0], {[1] => :a}[[1.0]]
        p catch_panic { [1].first(-1) }.err.message
        p (catch_panic { [1].last(1, 2) }.err && :argc), (catch_panic { [].[]() }.err && :argc)
        p [3, 1, 2].sort { |x, y| (x - y).to_f }, catch_panic { [1, 2].sort { none } }.err.message
        p [1, 2, 1].rindex(1), [1, 2, 3].rindex { _1 < 3 }, [1, :a, "s", 2].grep(Integer), [1, :a].grep_v(Integer)
        p (1..6).find_all(&:even?), [1, 2].grep(Integer) { _1 * 10 }
        x = [1]
        x.send(:initialize_copy, [7, 8])
        p x, (catch_panic { [1][Float::NAN] }.err && :range)
    "##);
    assert_eq!(
        out,
        "[none, none]\n[1, 2]\n[2, 3]\n[]\n\"x!\"\n\"a\"\n[\"b\"]\ntrue\nfalse\ntrue\nnone\n\
         \"negative array size\"\n:argc\n:argc\n[1, 2, 3]\n\"comparison of Integer with Integer failed\"\n\
         2\n1\n[1, 2]\n[:a]\n[2, 4, 6]\n[10, 20]\n[7, 8]\n:range\n"
    );
}

#[test]
fn errors_ranges_and_numbers() {
    let out = run(r##"
        e = NameError.new("a", :foo)
        p e.message, e.name
        p catch_panic { class X; alias foo bar; end }.err.name
        p catch_panic { NoSuchConst }.err.name
        e = catch_panic { nosuch(3, 1, 4) }.err
        p e.name, e.args, e.instance_variables
        p NoMethodError.new("t", :t, [1, 2]).args
        p Range.new(1, 10, true), Range.new(1, none), Range.new(1, 3) == (1..3)
        p catch_panic { Range.new(1, "a") }.err.message
        p ~0, ~2, 7.quo(2), 5.75.quo(-2), none.__id__ == none.object_id
    "##);
    assert_eq!(
        out,
        "\"a\"\n:foo\n:bar\n:NoSuchConst\n:nosuch\n[3, 1, 4]\n[:@message]\n[1, 2]\n1...10\n1..\ntrue\n\
         \"bad value for range\"\n-1\n-3\n3.5\n-2.875\ntrue\n"
    );
}
