# ruruby

A language with Ruby-based syntax, implemented in pure Rust (it builds for
wasm). It is not a Ruby port: Ruby compatibility is not a goal and the syntax
will diverge. The implementation is structured after
[mruby](https://github.com/mruby/mruby): the same pipeline (parser → AST →
register bytecode → VM), the same opcode vocabulary, and modules that map onto
mruby's source files.

On the `lang-fork` branch the language has its own syntax (`fn`, braces,
closures `|x| ...`, files as modules with `use`, `Ok`/`Err` and `?` instead of
exceptions, `none` instead of nil, `interface`/`impl` instead of mixins, Rust-like `enum`s); **[examples/](examples/README.md)** is a
guided tour of it.

```sh
cargo run --release -- examples/01_basics.rb
cargo run -- -e 'puts (1..10).select(&:even?).map(|x| x * x).sum'
cargo run -- --dump -e 'p [1, 2].map(|x| x + 1)'     # print bytecode, like `mruby -v`
cargo run -- -c file.rb                            # check syntax only
cargo run                                          # REPL
cargo test --workspace
cargo run --release -p ruruby-egui -- examples/ui/todo.rb   # lang-fork-ui: a UI in a window (egui)
cargo run --release --bin mrbtest   # run mruby's own test suite (needs ./mruby)
```

## Layout

| File | Role | mruby counterpart |
|---|---|---|
| `src/lexer.rs`, `src/parser/`, `src/ast.rs` | Source → AST, with locals resolved per scope; pattern matching is desugared in `parser/pattern.rs` | Prism (`mrbgems/mruby-compiler/lib/prism`) |
| `src/codegen.rs` | AST → register bytecode | `mruby-compiler/src/codegen.c` |
| `src/opcode.rs` | `Op`, `Irep`, disassembler | `include/mruby/ops.h`, `irep.h`, `src/codedump.c` |
| `src/vm.rs` | Dispatch loop, method lookup, calls, exceptions | `src/vm.c`, `state.c`, `class.c`, `error.c` |
| `src/value.rs` | `Value`, classes, objects, procs, hashes | `include/mruby/value.h`, `object.h`, `class.h`, `proc.h` |
| `src/gc.rs` | Heap arenas, `Gc` handles, mark-sweep collector | `src/gc.c` |
| `src/symbol.rs` | Symbol interning | `src/symbol.c` |
| `src/core/` | Built-in classes and methods (`reflect.rs`: singleton classes, `define_method`, `class_eval`, …; `prelude.rb`: iterators written in Ruby) | `src/{kernel,numeric,string,array,hash,range}.c`, `mrblib/`, default gems |
| `src/main.rs` | CLI + REPL | `mruby`, `mirb` |
| `reactive/` | `ruruby-reactive`: signals, memos, effects, push-pull updates (lang-fork-ui) | — |
| `src/ui.rs`, `src/core/ui.rb` | the declarative UI's elements and the changes a renderer applies (lang-fork-ui) | — |
| `backends/egui/` | `ruruby-ui`: a first renderer, on egui (lang-fork-ui) | — |

See **[docs/MRUBY_TO_RUST.md](docs/MRUBY_TO_RUST.md)** for the mruby analysis, the
C-to-Rust mapping, what's implemented, and the roadmap.

`./mruby` holds a reference checkout of mruby (git-ignored).
