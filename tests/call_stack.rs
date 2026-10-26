//! The explicit call stack (roadmap step 3): Ruby→Ruby calls run in one
//! dispatch loop without native recursion, and control flow still crosses
//! frames correctly.

use ruruby::{Interpreter, run_capture};

fn run(src: &str) -> String {
    match run_capture(src) {
        Ok(out) => out,
        Err(e) => panic!("error running {src:?}: {e}"),
    }
}

/// Runs `f` on a thread with a small native stack, like wasm's.
fn small_stack<T: Send + 'static>(kib: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(kib << 10)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn deep_ruby_recursion_needs_no_native_stack() {
    let out = small_stack(512, || {
        run(r#"
            fn down(n) = if n == 0 { 0 } else { 1 + down(n - 1) }
            fn via_yield(n) = if n == 0 { 0 } else { twice { 1 + via_yield(n - 1) } }
            fn twice = yield
            class Num {
              fn init(n) = @n = n
              fn +(o) = if @n == 0 { o } else { Num.new(@n - 1) + (o + 1) }
            }
            # each/map/times are written in Ruby (prelude.rb), so recursing
            # through them needs no native stack either
            fn via_each(n) = if n == 0 { 0 } else { [n].map { |k| 1 + via_each(k - 1) }.first }
            p down(9_000), via_yield(3_000), Num.new(4_000) + 0, via_each(2_000)
        "#)
    });
    assert_eq!(out, "9000\n3000\n4000\n2000\n");
}

#[test]
fn runaway_recursion_is_a_system_stack_error() {
    let out = small_stack(512, || {
        run(r#"
            fn forever(n) = forever(n + 1)
            p(catch_panic { forever(0) }.err.message)
            p(catch_panic { forever(0) }.err.class)
        "#)
    });
    assert_eq!(out, "\"stack level too deep\"\nSystemStackError\n");
}

#[test]
fn natives_calling_back_into_ruby_stay_within_the_stack_budget() {
    // each level recurses natively through Enumerable#inject, a native that
    // calls its block (~3 KiB in release, ~24 KiB in debug); past the budget
    // it's a SystemStackError, not a crash
    let out = small_stack(2048, || {
        let mut interp = Interpreter::new();
        interp.vm.stack_budget = 256 << 10;
        interp.capture_output();
        let src = r#"
            fn deep(n) = if n == 0 { 0 } else { [n].inject(0) { |_, k| 1 + deep(k - 1) } }
            p deep(5)
            p(catch_panic { deep(100_000) }.err && :caught)
            p deep(5)
        "#;
        interp.eval(src).map_err(|e| e.to_string())?;
        Ok::<_, String>(interp.take_output())
    });
    assert_eq!(out.unwrap(), "5\n:caught\n5\n");
}

#[test]
fn control_flow_across_frames() {
    let src = r#"
        log = []
        fn each_twice(a) {
          a.each { |x| yield x; yield x }
        }
        # break leaves two Ruby frames and a native one
        r = each_twice([1, 2]) { |x| break x * 10 if x == 2; log << x }
        p r, log
        # return from a block inside a method called from a block
        fn find_first(list) = list.each { |x| return x if yield(x) } && none
        p find_first([1, 4, 9]) { |x| x > 3 }
        # panics unwind through in-loop frames to catch_panic, again and again
        fn boom(n) = if n == 0 { panic("deep") } else { boom(n - 1) }
        tries = 0
        r = none
        while tries < 3 {
          tries += 1
          r = catch_panic { boom(50) }
        }
        p [tries, r.err.message]
        # super chains, method_missing, define_method lambdas and class bodies
        class A { fn hi(x) = "A#{x}" }
        class B < A { fn hi(x) = "B" + super }
        class C < B { fn hi(x) = "C" + super(x + 1) }
        class Ghost { fn method_missing(name, *args) = [name, args] }
        class D {
          define_method(:stop) { |x| [1, 2, 3].each { |y| return y * x if y == 2 }; :never }
        }
        v = class E { 42 }
        p C.new.hi(1), Ghost.new.zap(1, 2), D.new.stop(5), v
        # a lambda's break and return stay inside it
        l = || { [1, 2].each { |x| return x + 100 }; :no }
        m = lambda { break 7 }
        p l.call, m.call
    "#;
    assert_eq!(
        run(src),
        "20\n[1, 1]\n4\n[3, \"deep\"]\n\
         \"CBA2\"\n[:zap, [1, 2]]\n10\n42\n101\n7\n"
    );
}

#[test]
fn closures_outlive_their_frames() {
    // registers live on the VM stack while a frame runs; a captured env
    // takes a copy when the frame exits, however it exits
    let src = r#"
        fn counter() {
          n = 0
          [|| { n += 1 }, || { n }]
        }
        inc, get = counter
        inc.(); inc.()
        p get.()
        fn nested() {
          a = 1
          proc { b = 10; proc { a + b } }
        }
        p nested.call.call
        $saved = none
        fn leaves_by_raise() {
          x = 5
          $saved = || { x * 2 }
          panic "boom"
        }
        catch_panic { leaves_by_raise }
        p $saved.()
        fn leaves_by_break() {
          y = 7
          [1].each { $saved = || { y + 1 }; break }
          y = 8
          :done
        }
        p leaves_by_break, $saved.()
        fs = (1..3).map { |i| || { i * 10 } }
        p fs.map(&:call)
        GC.stress = true
        p inc.(), get.(), nested.call.call
    "#;
    assert_eq!(run(src), "2\n11\n10\n:done\n9\n[10, 20, 30]\n3\n3\n11\n");
}

#[test]
fn method_caches_see_every_change() {
    // `call(o)` is one Send whose inline cache gets filled, then each kind
    // of change must make it look the method up again
    let src = r#"
        class A { fn hi = "A" }
        fn call(o) = catch_panic { o.hi }.unwrap_or("none")
        a = A.new
        out = [call(a)]
        class A { fn hi = "A2" }
        out << call(a)
        interface M { fn hi() = "M" }
        impl M for A {}
        out << call(a)
        class A { remove_method :hi }
        out << call(a)
        fn a.hi = "singleton"
        out << call(a)
        b = A.new
        out << call(b)
        class << b { fn hi = "extended" }
        out << call(b)
        class B { fn hi = "B" }
        out << call(B.new)
        class B { alias_method :hello, :hi; undef_method :hi }
        out << call(B.new)
        class B { define_method(:hi) { "defined" } }
        out << call(B.new)
        B.send(:remove_method, :hi)
        out << call(B.new) << B.new.hello
        p out
        # the same site sees receivers of different classes
        p [1, "s", :y, none, 2.0].map { |x| x.to_s }
    "#;
    assert_eq!(
        run(src),
        "[\"A\", \"A2\", \"A2\", \"M\", \"singleton\", \"M\", \"extended\", \"B\", \"none\", \"defined\", \"none\", \"B\"]\n\
         [\"1\", \"s\", \"y\", \"\", \"2.0\"]\n"
    );
}
