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
