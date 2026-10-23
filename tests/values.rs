//! The NaN-boxed `Value`: Integers are inline in 48 bits and boxed in the
//! heap beyond that, which must not be observable; Floats are their own
//! bits, with one canonical NaN.

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn integers_across_the_inline_limit() {
    let out = run(r#"
        m = 140737488355327   # 2**47 - 1, the largest inline Integer
        p m + 1, -m - 2, (m + 1) - 1, m * 4, 2**62, -(2**62)
        x = m + 1
        p x.class, x == m + 1, x.equal?(m + 1), x.eql?(m + 1), x > m, x <=> m
        p x.hash == (m + 1).hash, x.to_s, x / 2, x % 7, -x
    "#);
    assert_eq!(
        out,
        "140737488355328\n-140737488355329\n140737488355327\n562949953421308\n\
         4611686018427387904\n-4611686018427387904\nInteger\ntrue\ntrue\ntrue\ntrue\n1\n\
         true\n\"140737488355328\"\n70368744177664\n4\n-140737488355328\n"
    );
}

#[test]
fn boxed_integers_as_keys_and_in_loops() {
    let out = run(r#"
        m = 140737488355327
        h = {m + 1 => :big, 1 => :one}
        p h[2**47], [m + 1, 2**47].uniq.size
        i = m - 3
        while i < m + 3
          i += 1
        end
        p i, (m..m + 2).to_a
        j = -m
        j -= 1 while j > -m - 3
        p j
    "#);
    assert_eq!(
        out,
        ":big\n1\n140737488355330\n[140737488355327, 140737488355328, 140737488355329]\n\
         -140737488355330\n"
    );
}

#[test]
fn boxed_integers_survive_collection() {
    let out = run(r#"
        GC.stress = true
        a = [2**50, {k: 2**51}, 2**52..2**52 + 1]
        def big = 2**60 + 1
        b = big
        GC.start
        p a[0] + 1, a[1][:k] - 1, a[2].to_a.size, b
    "#);
    assert_eq!(out, "1125899906842625\n2251799813685247\n2\n1152921504606846977\n");
}

#[test]
fn floats_and_nan() {
    let out = run(r#"
        n = 0.0 / 0.0
        p n, n.nan?, n == n, n.equal?(n), -0.0, -0.0 == 0.0, 1.0 / 0, -1.0 / 0
        p 2.5.equal?(2.5), 0.1 + 0.2, 1e300 * 1e10, 1.5.class
    "#);
    assert_eq!(
        out,
        "NaN\ntrue\nfalse\nfalse\n-0.0\ntrue\nInfinity\n-Infinity\ntrue\n\
         0.30000000000000004\nInfinity\nFloat\n"
    );
}
