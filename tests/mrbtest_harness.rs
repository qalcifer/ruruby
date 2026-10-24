//! Guards the harness used to run mruby's test suite (`cargo run --bin mrbtest`):
//! it must keep loading on ruruby and classify results correctly.

use ruruby::Interpreter;

#[test]
#[ignore = "lang-fork: mruby's harness (assert.rb) is built on rescue"]
fn harness_classifies_results() {
    let mut interp = Interpreter::new();
    interp.capture_output();
    interp
        .eval(include_str!("mruby/assert.rb"))
        .expect("harness loads");
    let src = r#"
        assert('passes') do
          assert_equal(2, 1 + 1)
          assert_raise(ZeroDivisionError) { 1 / 0 }
          assert_raise_with_message(RuntimeError, "boom") { raise "boom" }
          assert_operator(1, :<, 2)
          assert_float(0.3, 0.1 + 0.2)
          assert_match("ab*", "abcdef")
        end
        assert('fails') { assert_equal(3, 1 + 1) }
        assert('crashes') { none.nope }
        assert('skips') { skip "later" }
        assert('no assertions') { 1 }
        assert('nested') do
          assert('inner') { assert_true(true) }
        end
        _mrbtest_stats
    "#;
    let stats = interp.eval(src).expect("tests run");
    // [ok, ko, crash, skip, warn]
    assert_eq!(interp.inspect(&stats), "[3, 1, 1, 1, 1]");
}
