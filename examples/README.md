# A tour of the language

ruruby (on the `lang-fork` branch) is a language with Ruby's feel and Rust's
shapes: `fn` and braces, closures `|x| ...`, files as modules with `use`,
`Ok`/`Err` values and `?` instead of exceptions, and `none` instead of nil.
Each file below is one topic, runnable on its own; read them in order.

```sh
cargo run --release -- examples/01_basics.rb
cargo run --release -- examples/07_use/main.rb     # a program in several files
```

| File | Shows |
|---|---|
| [01_basics.rb](01_basics.rb) | `puts`/`p`, variables, interpolation, numbers, strings, symbols, `true`/`false`/`none`, arrays, hashes, ranges |
| [02_functions.rb](02_functions.rb) | `fn name(args) { }` and `fn name(args) = expr`, return values, defaults, keyword and rest arguments, spreading, recursion |
| [03_control_flow.rb](03_control_flow.rb) | `if` / `else if` / `else`, `unless`, `while`, `until`, `for`, `loop`, `break`/`next`, `case`/`when`, modifiers, `if` as a value |
| [04_closures.rb](04_closures.rb) | closures `\|x\| ...`, capturing, closures as a call's last argument, blocks, `yield`, `&f`, `&:sym` |
| [05_classes.rb](05_classes.rb) | classes, `initialize`, attributes, inheritance and `super`, class methods, operators, `impl Comparable`, `private fn`, reopening classes |
| [06_interfaces.rb](06_interfaces.rb) | `interface` with required and default methods, `impl I for C` and `impl I` in a class, built-in classes, super-interfaces, contract errors, `impl I by @field`, `Comparable`/`Enumerable`, modules as namespaces with their own functions |
| [07_use/](07_use/main.rb) | files as modules: `use file::Name`, `use file::Module::{A, B}`, `use file::*`, nested directories |
| [08_errors.rb](08_errors.rb) | `Ok`/`Err`, `?`, matching on results, `fetch` and `Integer()` returning results, `none` with `?`, panics and `catch_panic` |
| [09_pattern_matching.rb](09_pattern_matching.rb) | `case`/`in`: types, alternatives, arrays with splats, hashes, guards, binding with `=>`, `in` as a test |
| [10_collections.rb](10_collections.rb) | Array, Hash and Range methods |
| [11_strings.rb](11_strings.rb) | String methods, formatting, mutation and freezing, `to_s` in interpolation, heredocs, symbols |
| [12_program.rb](12_program.rb) | a small complete program: an inventory driven by text commands |

## The rules in brief

- **Functions** are `fn name(params) { body }`; the last expression is the
  value. One expression: `fn square(x) = x * x`.
- **`if` is an expression** (as in Rust; there is no `c ? a : b`):
  `x = if c { a } else { b }`; without `else` its value is `none`.
- **Every construct takes braces**: `if c { } else if c { } else { }`,
  `while c { }`, `case x { when 1 { } in [a, b] { } else { } }`,
  `class Name < Base { }`, `module Name { }`.
- **Closures** are `|a, b| expr`, `|x| { ... }`, `|| expr`. As a call's last
  argument a closure is the call's block: `list.map(|x| x * 2)`. A block in a
  condition goes in parentheses or uses that form:
  `if list.any?(|x| x > 1) { ... }`.
- **Files are modules**: `use path::to::file::Name`. Paths start at the main
  script's directory; every `use` is checked before the program runs.
- **Errors**: a failure the caller should handle is `Err(...)` (and success
  `Ok(...)`); `value?` returns an `Err` (or `none`) from the function.
  A bug is a `panic`, which ends the program.
- **`none`** is the absent value (falsy, like `false`).
- **Interfaces** name a contract: `interface Shape { fn area(); fn name() = "s" }`
  (no body: required; a body: a default). `impl Shape for Circle { fn area() = ... }`
  or `impl Shape { ... }` inside the class; checked when the block ends.
  Interfaces hold no state; `impl I by @field` reuses a field's methods.
- **Modules** are namespaces, like Rust's `mod`: constants, classes and functions;
  `fn f` in `module M { }` is called as `M.f` (`private fn` keeps it internal).

Removed from Ruby: `def`/`end`, `do ... end`, `->`, `elsif`, `then`, `begin`, `? :`,
`nil`, `raise`/`rescue`/`ensure`/`retry`, `include`/`extend`/`prepend`/`module_function` (modules are
namespaces; shared behaviour is an `interface`). Each is a compile error that says
what to write instead.

## Keeping the examples right

`tests/examples.rs` runs every example and compares its output with
`expected/<name>.txt`. After changing an example on purpose:

```sh
UPDATE_EXPECTED=1 cargo test --test examples   # then review the diff
```
