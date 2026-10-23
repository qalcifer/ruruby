//! Values of the built-in types carrying more than their contents (roadmap
//! step 5, part E): instance variables, and a class (a subclass of Array,
//! Hash or String).

use ruruby::run_capture;

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

#[test]
fn instance_variables_on_builtin_values() {
    let out = run(r##"
        GC.stress = true
        a = [1]; s = "x"; h = {}
        a.instance_variable_set(:@n, [1, 2])
        s.instance_variable_set(:@t, "tag")
        h.instance_variable_set(:@h, {k: 1})
        GC.start
        p a.instance_variable_get(:@n), s.instance_variable_get(:@t), h.instance_variables
        p a.instance_variable_defined?(:@n), a.remove_instance_variable(:@n), a.instance_variables
        GC.stress = false
        3000.times { |i| x = [i]; x.instance_variable_set(:@i, i) }
        GC.start
        fresh = Array.new(3000) { [] }
        p fresh.all? { |x| x.instance_variables.empty? }, 1.instance_variables
    "##);
    assert_eq!(out, "[1, 2]\n\"tag\"\n[:@h]\ntrue\n[1, 2]\n[]\ntrue\n[]\n");
}
