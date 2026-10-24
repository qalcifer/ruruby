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
            def down(n) = n == 0 ? 0 : 1 + down(n - 1)
            def via_yield(n) = n == 0 ? 0 : twice { 1 + via_yield(n - 1) }
            def twice = yield
            class Num
              def initialize(n) = @n = n
              def +(o) = @n == 0 ? o : Num.new(@n - 1) + (o + 1)
            end
            # each/map/times are written in Ruby (prelude.rb), so recursing
            # through them needs no native stack either
            def via_each(n) = n == 0 ? 0 : [n].map { |k| 1 + via_each(k - 1) }.first
            p down(9_000), via_yield(3_000), Num.new(4_000) + 0, via_each(2_000)
        "#)
    });
    assert_eq!(out, "9000\n3000\n4000\n2000\n");
}

#[test]
fn runaway_recursion_is_a_system_stack_error() {
    let out = small_stack(512, || {
        run(r#"
            def forever(n) = forever(n + 1)
            p(begin; forever(0); rescue SystemStackError => e; e.message; end)
            # not a StandardError, so a bare rescue doesn't catch it
            p(begin; forever(0); rescue; :no; rescue Exception => e; e.class; end)
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
            def deep(n) = n == 0 ? 0 : [n].inject(0) { |_, k| 1 + deep(k - 1) }
            p deep(5)
            p(begin; deep(100_000); rescue SystemStackError; :caught; end)
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
        def each_twice(a)
          a.each { |x| yield x; yield x }
        ensure
          log_it(:each_twice_ensure)
        end
        def log_it(x) = $log << x
        $log = log
        # break leaves two Ruby frames and a native one, running ensure on the way
        r = each_twice([1, 2]) { |x| break x * 10 if x == 2; log << x }
        p r, log
        # return from a block inside a method called from a block
        def find_first(list) = list.each { |x| return x if yield(x) } && none
        p find_first([1, 4, 9]) { |x| x > 3 }
        # exceptions unwind through in-loop frames to an outer rescue, then retry
        def boom(n) = n == 0 ? raise("deep") : boom(n - 1)
        tries = 0
        begin
          tries += 1
          boom(50) if tries < 3
        rescue => e
          retry
        end
        p [tries, e.message]
        # super chains, method_missing, define_method lambdas and class bodies
        class A; def hi(x) = "A#{x}"; end
        class B < A; def hi(x) = "B" + super; end
        class C < B; def hi(x) = "C" + super(x + 1); end
        class Ghost; def method_missing(name, *args) = [name, args]; end
        class D
          define_method(:stop) { |x| [1, 2, 3].each { |y| return y * x if y == 2 }; :never }
        end
        v = class E; 42; end
        p C.new.hi(1), Ghost.new.zap(1, 2), D.new.stop(5), v
        # a lambda's break and return stay inside it
        l = -> { [1, 2].each { |x| return x + 100 }; :no }
        m = lambda { break 7 }
        p l.call, m.call
    "#;
    assert_eq!(
        run(src),
        "20\n[1, 1, :each_twice_ensure]\n4\n[3, \"deep\"]\n\
         \"CBA2\"\n[:zap, [1, 2]]\n10\n42\n101\n7\n"
    );
}

#[test]
fn closures_outlive_their_frames() {
    // registers live on the VM stack while a frame runs; a captured env
    // takes a copy when the frame exits, however it exits
    let src = r#"
        def counter
          n = 0
          [-> { n += 1 }, -> { n }]
        end
        inc, get = counter
        inc.(); inc.()
        p get.()
        def nested
          a = 1
          proc { b = 10; proc { a + b } }
        end
        p nested.call.call
        $saved = none
        def leaves_by_raise
          x = 5
          $saved = -> { x * 2 }
          raise "boom"
        end
        begin; leaves_by_raise; rescue; end
        p $saved.()
        def leaves_by_break
          y = 7
          [1].each { $saved = -> { y + 1 }; break }
          y = 8
          :done
        end
        p leaves_by_break, $saved.()
        fs = (1..3).map { |i| -> { i * 10 } }
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
        class A; def hi = "A"; end
        def call(o) = (o.hi rescue "none")
        a = A.new
        out = [call(a)]
        class A; def hi = "A2"; end
        out << call(a)
        module M; def hi = "M"; end
        class A; include M; end
        out << call(a)
        module P; def hi = "P"; end
        class A; prepend P; end
        out << call(a)
        def a.hi = "singleton"
        out << call(a)
        b = A.new
        out << call(b)
        b.extend(Module.new { define_method(:hi) { "extended" } })
        out << call(b)
        class B; def hi = "B"; end
        out << call(B.new)
        class B; alias_method :hello, :hi; undef_method :hi; end
        out << call(B.new)
        class B; define_method(:hi) { "defined" }; end
        out << call(B.new)
        B.send(:remove_method, :hi)
        out << call(B.new) << B.new.hello
        p out
        # the same site sees receivers of different classes
        p [1, "s", :y, none, 2.0].map { |x| x.to_s }
    "#;
    assert_eq!(
        run(src),
        "[\"A\", \"A2\", \"A2\", \"P\", \"singleton\", \"P\", \"extended\", \"B\", \"none\", \"defined\", \"none\", \"B\"]\n\
         [\"1\", \"s\", \"y\", \"\", \"2.0\"]\n"
    );
}
