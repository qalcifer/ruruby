# A small mspec-compatible runner for ruby/spec files, written for ruruby.
#
#   ruruby -r tools/mspec/shim.rb rubyspec/core/array/push_spec.rb
#
# The real mspec (github.com/ruby/mspec) needs File, Thread, ENV, IO and
# more; this shim implements the parts the core and language specs use:
# describe/context/it, before/after, shared examples, `should`/`should_not`
# (operator proxies and matchers), guards, mocks, ScratchPad and helpers.
#
# Like mspec, every block runs with instance_exec on one environment object
# (so `@method` set by `it_behaves_like` reaches the shared examples).
#
# Results go to stdout, one line per example, for src/bin/rubyspec.rs:
#   \x01P <description>
#   \x01F <description>\t<message>      expectation not met
#   \x01E <description>\t<Class: msg>   an exception in the example
#   \x01S <description>\t<why>          skipped (needs something not modelled)
#   \x01X <where>\t<Class: msg>         an error outside examples

TOLERANCE = 0.00003
TIME_TOLERANCE = 20.0

class SpecExpectationNotMetError < StandardError; end
class SpecSkip < StandardError; end

module MSpecShim
  MARK = "\x01"
  RUBY_VERSION_PARTS = RUBY_VERSION.split(".").map(&:to_i)

  class Context
    attr_reader :desc, :parent, :befores, :afters, :examples, :children, :shared

    def initialize(desc, parent, shared)
      @desc = desc
      @parent = parent
      @shared = shared
      @befores = { all: [], each: [] }
      @afters = { all: [], each: [] }
      @examples = []
      @children = []
    end

    def full
      @parent && !@parent.desc.empty? ? "#{@parent.full} #{@desc}" : @desc
    end

    def chain
      @parent ? @parent.chain + [self] : [self]
    end
  end

  @root = Context.new("", nil, false)
  @current = @root
  @shared = {}
  @env = nil
  @expected = false

  class << self
    attr_accessor :current, :env, :expected

    def shared
      @shared
    end

    def root
      @root
    end

    def report(kind, desc, msg = nil)
      line = "#{MARK}#{kind} #{desc}"
      line += "\t#{msg.to_s.gsub("\n", ' ')[0, 300]}" if msg
      puts line
    end

    def describe(desc, shared)
      ctx = Context.new(desc, shared ? nil : @current, shared)
      if shared
        @shared[desc.to_s] = ctx
      else
        @current.children << ctx
      end
      outer = @current
      @current = ctx
      begin
        yield if block_given?
      rescue => e
        report("X", ctx.full, "#{e.class}: #{e.message}")
      ensure
        @current = outer
      end
    end

    def protect(where)
      yield
      true
    rescue SpecSkip => e
      report("S", where, e.message)
      false
    rescue SpecExpectationNotMetError => e
      report("F", where, e.message)
      false
    rescue Exception => e
      report("E", where, "#{e.class}: #{e.message}")
      false
    end

    def run_blocks(blocks)
      blocks.each { |b| @env.instance_exec(&b) }
    end

    def run(ctx = @root)
      chain = ctx.chain
      unless ctx.examples.empty?
        ok = protect("#{ctx.full} (before :all)") { run_blocks(ctx.befores[:all]) }
        if ok
          ctx.examples.each do |name, blk|
            desc = "#{ctx.full} #{name}"
            Mock.reset
            @expected = false
            passed = protect(desc) do
              chain.each { |c| run_blocks(c.befores[:each]) }
              begin
                @env.instance_exec(&blk)
                Mock.verify
              ensure
                chain.reverse.each { |c| run_blocks(c.afters[:each]) }
              end
            end
            Mock.cleanup
            report("P", desc) if passed
          end
        end
        protect("#{ctx.full} (after :all)") { run_blocks(ctx.afters[:all]) }
      end
      ctx.children.each { |c| run(c) }
    end

    def version_parts(v)
      v.to_s.split(".").map(&:to_i)
    end

    # mspec's version guard: "3.1" (from 3.1 on), "3.1"..."3.3" (before 3.3),
    # "3.1".."3.3" (to 3.3.x inclusive), ""..."3.0".
    def version_matches?(spec)
      v = RUBY_VERSION_PARTS
      case spec
      when Range
        lo = spec.begin.to_s.empty? ? [] : version_parts(spec.begin)
        hi = spec.end.to_s.empty? ? nil : version_parts(spec.end)
        return false if (v <=> lo) < 0
        return true unless hi
        if spec.exclude_end?
          (v <=> hi) < 0
        else
          (v[0, hi.size] <=> hi) <= 0
        end
      else
        (v <=> version_parts(spec)) >= 0
      end
    end
  end

  # `obj.should` without a matcher.
  class Positive < BasicObject
    def initialize(actual, negate)
      @actual = actual
      @negate = negate
    end

    def __check(ok, what)
      ::MSpecShim.expected = true
      ok = !ok if @negate
      return true if ok
      neg = @negate ? "not " : ""
      ::Kernel.raise ::SpecExpectationNotMetError, "Expected #{::MSpecShim.show(@actual)} #{neg}#{what}"
    end

    def ==(o) = __check(@actual == o, "== #{::MSpecShim.show(o)}")
    def !=(o) = __check(@actual != o, "!= #{::MSpecShim.show(o)}")
    def =~(o) = __check(@actual =~ o, "=~ #{::MSpecShim.show(o)}")
    def <(o) = __check(@actual < o, "< #{::MSpecShim.show(o)}")
    def <=(o) = __check(@actual <= o, "<= #{::MSpecShim.show(o)}")
    def >(o) = __check(@actual > o, "> #{::MSpecShim.show(o)}")
    def >=(o) = __check(@actual >= o, ">= #{::MSpecShim.show(o)}")
    def equal?(o) = __check(@actual.equal?(o), "to be identical to #{::MSpecShim.show(o)}")
    def eql?(o) = __check(@actual.eql?(o), "to be eql? to #{::MSpecShim.show(o)}")

    def raise(cls = ::Exception, msg = nil, opts = nil, &blk)
      m = ::MSpecShim::RaiseError.new(cls, msg, &blk)
      if @negate
        ::MSpecShim.expected = true
        begin
          @actual.call
        rescue ::Exception => e
          ::Kernel.raise ::SpecExpectationNotMetError, "Expected no exception, got #{e.class} (#{e.message})"
        end
        true
      else
        m.check(@actual)
      end
    end

    def method_missing(name, *args, &blk)
      __check(@actual.__send__(name, *args, &blk), "to #{name} #{args.map { |a| ::MSpecShim.show(a) }.join(', ')}")
    end
  end

  def self.show(v)
    s = v.inspect
    s.size > 120 ? s[0, 117] + "..." : s
  rescue ::Exception
    "#<#{v.class}>"
  end

  class RaiseError
    def initialize(cls, msg, &blk)
      @cls = cls
      @msg = msg
      @blk = blk
    end

    def check(callable)
      MSpecShim.expected = true
      begin
        callable.call
      rescue Exception => e
        unless e.is_a?(@cls)
          raise SpecExpectationNotMetError, "Expected #{@cls}#{@msg ? " (#{@msg})" : ''} but got #{e.class} (#{e.message})"
        end
        if @msg && !(defined?(Regexp) && @msg.is_a?(Regexp) ? @msg =~ e.message : @msg == e.message)
          raise SpecExpectationNotMetError, "Expected #{@cls} (#{@msg.inspect}) but got #{e.class} (#{e.message.inspect})"
        end
        @blk.call(e) if @blk
        return true
      end
      raise SpecExpectationNotMetError, "Expected #{@cls}#{@msg ? " (#{@msg.inspect})" : ''} but no exception was raised"
    end

    def matches?(callable)
      check(callable)
    end
  end

  # Matcher objects (`should be_close(...)` and the older `should be_nil` style).
  class Matcher
    def initialize(desc, &test)
      @desc = desc
      @test = test
    end

    def check(actual, negate)
      MSpecShim.expected = true
      ok = @test.call(actual)
      ok = !ok if negate
      return true if ok
      raise SpecExpectationNotMetError, "Expected #{MSpecShim.show(actual)} #{negate ? 'not ' : ''}#{@desc}"
    end
  end

  # should_receive & co.
  class Expectation
    attr_reader :obj, :name, :calls

    def initialize(obj, name, kind)
      @obj = obj
      @name = name
      @kind = kind # :expect, :never, :stub
      @args = nil
      @count = kind == :never ? (0..0) : (kind == :stub ? (0..) : (1..1))
      @calls = 0
      @results = nil
      @raise = nil
      @yields = nil
      @block = nil
    end

    def with(*args) = (@args = args; self)
    def and_return(*vals) = (@results = vals; self)
    def and_raise(e) = (@raise = e; self)
    def and_yield(*vals) = (@yields = vals; self)
    def and_call_original = (@block = :original; self)
    def once = (@count = 1..1; self)
    def twice = (@count = 2..2; self)
    def exactly(n) = (@count = n..n; self)
    def at_least(n) = (@count = ((n == :once ? 1 : n == :twice ? 2 : n)..); self)
    def at_most(n) = (@count = 0..(n == :once ? 1 : n == :twice ? 2 : n); self)
    def any_number_of_times = (@count = (0..); self)
    def times = self
    def time = self

    def matches_args?(args)
      @args.nil? || @args == [:any_args] || @args == args || (@args == [:no_args] && args.empty?)
    end

    def call(args, blk)
      @calls += 1
      if @yields && blk
        blk.call(*@yields)
      end
      raise @raise if @raise
      return nil unless @results
      i = [@calls - 1, @results.size - 1].min
      @results[i]
    end

    def verify
      return if @count.include?(@calls)
      raise SpecExpectationNotMetError,
        "Mock '#{MSpecShim.show(@obj)}' expected to receive '#{@name}' #{@count.inspect} times but received it #{@calls} times"
    end
  end

  module Mock
    @expectations = []
    @proxied = []

    def self.reset
      @expectations = []
      @proxied = []
    end

    def self.add(obj, name, kind)
      e = Expectation.new(obj, name, kind)
      @expectations << e
      unless @proxied.any? { |o, n| o.equal?(obj) && n == name }
        @proxied << [obj, name]
        exps = @expectations
        obj.define_singleton_method(name) do |*args, &blk|
          found = exps.reverse.find { |x| x.obj.equal?(self) && x.name == name && x.matches_args?(args) }
          unless found
            raise SpecExpectationNotMetError, "Mock '#{MSpecShim.show(self)}' received unexpected '#{name}' with #{MSpecShim.show(args)}"
          end
          found.call(args, blk)
        end
      end
      e
    end

    def self.verify
      @expectations.each(&:verify)
    end

    def self.cleanup
      @proxied.each do |obj, name|
        obj.singleton_class.send(:remove_method, name) rescue nil
      end
      @proxied = []
    end
  end

  class MockObject
    def initialize(name) = @name = name
    def inspect = "#<MockObject #{@name}>"
    def to_s = inspect
  end

  # One per run: every block runs on it (mspec: MSpecEnv).
  class Env
  end
end

module ScratchPad
  def self.clear = @record = nil
  def self.record(v) = @record = v
  def self.<<(v) = (@record << v; self)
  def self.recorded = @record
end

class Object
  def should(matcher = nil, &blk)
    MSpecShim.expected = true
    return MSpecShim::Positive.new(self, false) if matcher.nil?
    matcher.is_a?(MSpecShim::RaiseError) ? matcher.check(self) : matcher.check(self, false)
  end

  def should_not(matcher = nil, &blk)
    MSpecShim.expected = true
    return MSpecShim::Positive.new(self, true) if matcher.nil?
    if matcher.is_a?(MSpecShim::RaiseError)
      begin
        call
      rescue Exception => e
        raise SpecExpectationNotMetError, "Expected no exception, got #{e.class} (#{e.message})"
      end
      return true
    end
    matcher.check(self, true)
  end

  def should_receive(name) = MSpecShim::Mock.add(self, name.to_sym, :expect)
  def should_not_receive(name) = MSpecShim::Mock.add(self, name.to_sym, :never)
  def stub!(name) = MSpecShim::Mock.add(self, name.to_sym, :stub)

  private

  def describe(desc, extra = nil, opts = nil, &blk)
    shared = false
    if extra.is_a?(Hash)
      shared = extra[:shared]
      extra = nil
    end
    shared ||= opts.is_a?(Hash) && opts[:shared]
    name = desc.is_a?(Module) ? desc.name.to_s : desc.to_s
    name = "#{name}#{extra.to_s.start_with?('#', '.', '::') ? '' : ' '}#{extra}" if extra
    MSpecShim.describe(name, shared, &blk)
  end
  alias context describe

  def it(desc, &blk)
    MSpecShim.current.examples << [desc.to_s, blk || -> { raise SpecSkip, "no body" }]
  end
  alias specify it

  def before(kind = :each, &blk)
    MSpecShim.current.befores[kind == :all ? :all : :each] << blk
  end

  def after(kind = :each, &blk)
    MSpecShim.current.afters[kind == :all ? :all : :each].unshift(blk)
  end

  def it_behaves_like(desc, meth = nil, obj = nil)
    shared = MSpecShim.shared[desc.to_s]
    raise "Unable to find shared 'describe' for #{desc}" unless shared
    cur = MSpecShim.current
    cur.befores[:all] << -> { @method = meth; @object = obj }
    shared.befores[:all].each { |b| cur.befores[:all] << b }
    shared.befores[:each].each { |b| cur.befores[:each] << b }
    shared.afters[:each].each { |b| cur.afters[:each] << b }
    shared.afters[:all].each { |b| cur.afters[:all] << b }
    shared.examples.each { |e| cur.examples << e }
    shared.children.each { |c| cur.children << c }
  end
  alias it_should_behave_like it_behaves_like

  # ---- guards ------------------------------------------------------------

  def ruby_version_is(*specs)
    ok = specs.all? { |s| MSpecShim.version_matches?(s) }
    yield if ok && block_given?
    ok
  end

  def platform_is(*names, **_)
    ok = names.any? { |n| [:darwin, :unix, :posix, :linux_like].include?(n) } && !names.include?(:windows)
    yield if ok && block_given?
    ok
  end

  def platform_is_not(*names, **_)
    ok = !names.any? { |n| [:darwin, :unix, :posix].include?(n) }
    yield if ok && block_given?
    ok
  end

  def guard(cond = nil)
    ok = cond.respond_to?(:call) ? cond.call : cond
    yield if ok && block_given?
    ok
  end

  def guard_not(cond = nil)
    ok = !(cond.respond_to?(:call) ? cond.call : cond)
    yield if ok && block_given?
    ok
  end

  def ruby_bug(*) = (yield if block_given?)
  def conflicts_with(*) = nil
  def not_supported_on(*names) = (yield if block_given? && !names.include?(:ruruby))
  def deviates_on(*) = nil
  def extended_on(*) = nil
  def kernel_is(*) = nil
  def with_feature(*) = nil
  def without_feature(*) = (yield if block_given?)
  def as_user = (yield if block_given?)
  def as_superuser = nil
  def as_real_superuser = nil
  def little_endian = (yield if block_given?)
  def big_endian = nil
  def quarantine! = nil
  def slow = (yield if block_given?)
  def wordsize = 64
  def platform_is_windows? = false
  def process_is_foreground = (yield if block_given?)

  # ---- matchers (older style and the ones `should` takes) ------------------

  def be_close(v, tol) = MSpecShim::Matcher.new("to be within #{tol} of #{v.inspect}") { |a| (a - v).abs <= tol }
  def be_nil = MSpecShim::Matcher.new("to be nil") { |a| a.nil? }
  def be_true = MSpecShim::Matcher.new("to be true") { |a| a == true }
  def be_false = MSpecShim::Matcher.new("to be false") { |a| a == false }
  def be_true_or_false = MSpecShim::Matcher.new("to be true or false") { |a| a == true || a == false }
  def be_empty = MSpecShim::Matcher.new("to be empty") { |a| a.empty? }
  def be_nan = MSpecShim::Matcher.new("to be NaN") { |a| a.is_a?(Float) && a.nan? }
  def be_positive_infinity = MSpecShim::Matcher.new("to be Infinity") { |a| a.is_a?(Float) && a.infinite? == 1 }
  def be_negative_infinity = MSpecShim::Matcher.new("to be -Infinity") { |a| a.is_a?(Float) && a.infinite? == -1 }
  def be_positive_zero = MSpecShim::Matcher.new("to be +0.0") { |a| a.is_a?(Float) && a == 0 && 1 / a > 0 }
  def be_negative_zero = MSpecShim::Matcher.new("to be -0.0") { |a| a.is_a?(Float) && a == 0 && 1 / a < 0 }
  def be_kind_of(c) = MSpecShim::Matcher.new("to be a kind of #{c}") { |a| a.is_a?(c) }
  def be_an_instance_of(c) = MSpecShim::Matcher.new("to be an instance of #{c}") { |a| a.instance_of?(c) }
  def be_ancestor_of(c) = MSpecShim::Matcher.new("to be an ancestor of #{c}") { |a| c.ancestors.include?(a) }
  def eql(v) = MSpecShim::Matcher.new("to be eql? to #{v.inspect}") { |a| a.eql?(v) }
  def equal(v) = MSpecShim::Matcher.new("to be identical to #{v.inspect}") { |a| a.equal?(v) }
  def include(*vs) = MSpecShim::Matcher.new("to include #{vs.inspect}") { |a| vs.all? { |v| a.include?(v) } }
  def include_any_of(*vs) = MSpecShim::Matcher.new("to include any of #{vs.inspect}") { |a| vs.any? { |v| a.include?(v) } }
  def respond_to(m) = MSpecShim::Matcher.new("to respond to #{m}") { |a| a.respond_to?(m) }
  def have_method(m, *) = MSpecShim::Matcher.new("to have method #{m}") { |a| a.methods.include?(m.to_sym) }
  def have_instance_method(m, *) = MSpecShim::Matcher.new("to have instance method #{m}") { |a| a.instance_methods.include?(m.to_sym) }
  def have_public_instance_method(m, *) = MSpecShim::Matcher.new("to have public instance method #{m}") { |a| a.public_instance_methods.include?(m.to_sym) }
  def have_private_instance_method(m, *) = MSpecShim::Matcher.new("to have private instance method #{m}") { |a| a.private_instance_methods.include?(m.to_sym) }
  def have_protected_instance_method(m, *) = MSpecShim::Matcher.new("to have protected instance method #{m}") { |a| a.protected_instance_methods.include?(m.to_sym) }
  def have_private_method(m, *) = MSpecShim::Matcher.new("to have private method #{m}") { |a| a.private_methods.include?(m.to_sym) }
  def have_singleton_method(m, *) = MSpecShim::Matcher.new("to have singleton method #{m}") { |a| a.singleton_methods.include?(m.to_sym) }
  def have_constant(c) = MSpecShim::Matcher.new("to have constant #{c}") { |a| a.constants.include?(c.to_sym) }
  def have_instance_variable(v) = MSpecShim::Matcher.new("to have ivar #{v}") { |a| a.instance_variables.include?(v.to_sym) }
  def have_class_variable(v) = MSpecShim::Matcher.new("to have cvar #{v}") { |a| a.class_variables.include?(v.to_sym) }
  def raise_error(cls = Exception, msg = nil, &blk) = MSpecShim::RaiseError.new(cls, msg, &blk)

  def be_computed_by(meth, *args)
    MSpecShim::Matcher.new("to be computed by #{meth}") do |table|
      table.all? do |row|
        recv, *rest = row
        expected = rest.pop
        recv.__send__(meth, *rest, *args) == expected
      end
    end
  end

  # warnings and output aren't captured: these expectations are skipped
  def complain(*) = raise(SpecSkip, "complain (warnings not captured)")
  def output(*) = raise(SpecSkip, "output (stdout not captured)")
  def output_to_fd(*) = raise(SpecSkip, "output_to_fd")
  def raise_consistent_error(*) = raise(SpecSkip, "raise_consistent_error")

  # ---- helpers -------------------------------------------------------------

  def mock(name = "mock") = MSpecShim::MockObject.new(name)
  alias mock_numeric mock

  def mock_int(n)
    m = mock("int")
    m.should_receive(:to_int).any_number_of_times.and_return(n)
    m
  end

  def nan_value = 0.0 / 0.0
  def infinity_value = 1.0 / 0.0
  def bignum_value(plus = 0) = 2**64 + plus
  def fixnum_max = 2**62 - 1
  def fixnum_min = -(2**62)
  def max_long = 2**63 - 1
  def min_long = -(2**63)
  def suppress_warning = yield
  def suppress_keyword_warning = yield
  def flunk(msg = "flunked") = raise(SpecExpectationNotMetError, msg)
  def skip(msg = "skip") = raise(SpecSkip, msg)
  def fixture(file, *names) = file[0, file.rindex("/") || 0] + "/fixtures/" + names.join("/")
  def ruby_exe(*) = raise(SpecSkip, "ruby_exe")
  def tmp(name, uniquify = true) = "/tmp/ruruby_spec_#{name}"
  def NATFIXME(*) = (yield if block_given?)
end

# require_relative of the real spec_helper is the shim itself
class Object
  alias_method :__shim_require_relative, :require_relative

  def require_relative(path, from = nil)
    return true if path.end_with?("spec_helper")
    __shim_require_relative(path, from)
  end
end

MSpecShim.env = MSpecShim::Env.new
at_exit do
  MSpecShim.run
  puts "#{MSpecShim::MARK}DONE"
end
