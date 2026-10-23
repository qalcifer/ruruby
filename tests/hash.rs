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
         nil, 6, nil, 11, 13, 15, nil, 13, 17, nil, nil, nil, nil]\n"
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
        "500\n0\n-6\n16\nnil\n[0, 2, 4, 6]\n998\nnil\ntrue\nfalse\n0\nnil\n7\n20\n{a: 1}\n"
    );
}

#[test]
fn a_hash_as_its_own_key() {
    let out = run(r#"
        s = {}
        s[s] = 1
        20.times { |i| s[i] = i }
        p s[s], s.size
    "#);
    assert_eq!(out, "1\n21\n");
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
