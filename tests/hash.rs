//! Hash storage: small hashes scan their pairs, big ones (more than 8)
//! use an index. Both must find exactly the same keys.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn index_finds_what_a_scan_finds() {
    // each probe looked up in small hashes of the keys (scanned) and in one
    // with 100 filler keys too (indexed); `bad` lists probes they disagree on
    let out = run(r#"
        class Obj; end
        o = Obj.new
        keys = [1, -1, 0, 2**50, -(2**60), 1.5, 0.0, nil, true, false, :sym, "str",
                "", [1, [2, "x"]], [], 1..3, o, {a: 1}]
        probes = keys + [1.0, -0.0, 2.0**50, "st" + "r", [1, [2, "x"]], (1..3), Obj.new,
                         [1.0, [2, "x"]], {a: 1}, :other, "other", 2, 2**50 + 1]
        smalls = [{}, {}, {}]   # 6 keys each: scanned
        keys.each_with_index { |k, i| smalls[i / 6][k] = i }
        scan = ->(k) { s = smalls.find { |h| h.key?(k) }; s ? s[k] : nil }
        big = {}
        100.times { |i| big["filler#{i}"] = -i }
        keys.each_with_index { |k, i| big[k] = i }
        bad = probes.reject { |k| scan.(k) == big[k] && smalls.any? { |h| h.key?(k) } == big.key?(k) }
        p bad, smalls.map(&:size), big.size
        p probes.map { |k| big[k] }
    "#);
    assert_eq!(
        out,
        "[]\n[6, 6, 6]\n118\n\
         [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, \
         none, 6, none, 11, 13, 15, none, none, 17, none, none, none, none]\n"
    );
}

#[test]
fn big_hash_keeps_order_through_updates_and_deletes() {
    let out = run(r#"
        h = {}
        1000.times { |i| h[i] = i * i }
        1000.times { |i| h[i] = -i if i % 3 == 0 }
        (0...1000).each { |i| h.delete(i) if i % 2 == 1 }
        p h.size, h[0], h[6], h[4], h[5], h.keys.first(4), h.keys.last
        p h.delete(12345), h.key?(998), h.key?(999)
        (0...1000).each { |i| h.delete(i) }
        p h.size, h[0]
        20.times { |i| h["k#{i}"] = i }
        p h["k7"], h.size
        h.clear
        h[:a] = 1
        p h
    "#);
    assert_eq!(
        out,
        "500\n0\n-6\n16\nnone\n[0, 2, 4, 6]\n998\nnone\ntrue\nfalse\n0\nnone\n7\n20\n{a: 1}\n"
    );
}

#[test]
fn mutated_keys_and_rehash() {
    // like Ruby: a key mutated after insertion isn't found until rehash
    let out = run(r#"
        h = {}
        20.times { |i| h[[i]] = i }
        k = h.keys[5]
        k << :x
        p h[[5]], h[[5, :x]]
        h.rehash
        p h[[5, :x]], h.size, h.keys[5]
        s = {}
        s[s] = 1
        20.times { |i| s[i] = i }
        p s[s], s.size
    "#);
    assert_eq!(out, "none\nnone\n5\n20\n[5, :x]\n1\n21\n");
}

#[test]
fn big_hash_survives_collection() {
    let out = run(r#"
        h = {}
        5000.times { |i| h["k#{i}"] = [i] }
        GC.start
        GC.stress = true
        x = h["k4321"]
        h["new"] = "v"
        p x, h["new"], h.size
    "#);
    assert_eq!(out, "[4321]\n\"v\"\n5001\n");
}

#[test]
fn deletes_leave_no_trace() {
    // deleted entries stay as dead slots until compaction; lookups,
    // order and size must not see them, across the scan/index threshold
    let out = run(r#"
        h = {}
        20.times { |i| h[i] = i }
        i = 20
        10_000.times { h.delete(i - 20); h[i] = i; i += 1 }
        p h.size, h.keys.first, h.keys.last, h[i - 1], h[i - 21], h.key?(i - 20)
        (i - 20...i - 3).each { |k| h.delete(k) }
        p h.size, h.keys
        30.times { |k| h[-k] = k }
        p h.size, h[-29], h[i - 1], h.keys.first(4)
        h.delete(10_018)
        p h.keys.first(3), h.values.last
    "#);
    assert_eq!(
        out,
        "20\n10000\n10019\n10019\nnone\ntrue\n3\n[10017, 10018, 10019]\n33\n29\n10019\n\
         [10017, 10018, 10019, 0]\n[10017, 10019, 0]\n29\n"
    );
}

const POINT: &str = r#"
    class Pt
      attr_reader :x, :y
      def initialize(x, y) = (@x = x; @y = y)
      def eql?(o) = o.is_a?(Pt) && x == o.x && y == o.y
      def hash = [x, y].hash
    end
"#;

#[test]
fn keys_with_their_own_hash_and_eql() {
    let out = run(&(POINT.to_owned()
        + r#"
        small = {Pt.new(1, 2) => :a, "s" => 1}
        small[Pt.new(1, 2)] = :b
        p small[Pt.new(1, 2)], small.key?(Pt.new(2, 1)), small.size, small["s"]
        big = {}
        100.times { |i| big[Pt.new(i, -i)] = i }
        p big[Pt.new(42, -42)], big.fetch(Pt.new(3, -3)), big[Pt.new(42, 42)]
        p big.delete(Pt.new(7, -7)), big.key?(Pt.new(7, -7)), big.size
        big[Pt.new(42, -42)] = :x
        p big.size, big[Pt.new(42, -42)], big.values_at(Pt.new(0, 0), Pt.new(9, -9))
        p [Pt.new(1, 1), Pt.new(1, 1), Pt.new(2, 2)].tally.values
        k = Pt.new(5, 5)
        m = {k => 1}
        k.instance_variable_set(:@x, 6)
        p m[Pt.new(6, 5)], m.rehash[Pt.new(6, 5)]
    "#));
    assert_eq!(
        out,
        ":b\nfalse\n2\n1\n42\n3\nnone\n7\nfalse\n99\n99\n:x\n[0, 9]\n[2, 1]\nnone\n1\n"
    );
}

#[test]
fn eql_callbacks_that_change_the_hash() {
    // a lookup that ran Ruby code checks the hash didn't change under it
    let out = run(r#"
        class Evil
          def initialize(h) = @h = h
          def eql?(o) = (@h.delete("b"); false)
          def hash = 0
        end
        h = {"a" => 1, "b" => 2, "c" => 3}
        r = begin; h[Evil.new(h)]; rescue RuntimeError => e; e.message; end
        p r, h.keys, h.keys.all? { |k| h.key?(k) }
        class Bad; def hash = "x"; end
        p((begin; {Bad.new => 1}; rescue TypeError => e; e.message; end))
        class Same; def eql?(o) = raise("not called for the same object"); end
        s = Same.new
        p({s => 1}[s])
    "#);
    assert_eq!(
        out,
        "\"hash modified during lookup\"\n[\"a\", \"c\"]\ntrue\n\
         \"hash must return an Integer, not String\"\n1\n"
    );
}
