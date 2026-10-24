//! Compiles (without running) every `.rb` file in the mruby checkout, the
//! same check as `ruruby -c`. It catches parser regressions on a large body
//! of real code. Skipped when `./mruby` isn't cloned.

use ruruby::Interpreter;
use std::path::{Path, PathBuf};

/// Files expected not to compile, with the reason. The test fails when one of
/// them starts compiling, so the list stays current.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("test/t/literals.rb", "heredoc started inside #{}"),
    ("test/t/codegen.rb", "heredoc started inside #{}"),
    ("mrbgems/mruby-bigint/test/bigint.rb", "bignum literal (step 7)"),
    ("mrbgems/mruby-cmath/test/cmath.rb", "imaginary literal (step 7)"),
    ("mrbgems/mruby-complex/test/complex.rb", "imaginary literal (step 7)"),
    ("mrbgems/mruby-numeric-ext/test/numeric.rb", "rational literal (step 7)"),
    ("mrbgems/mruby-rational/test/rational.rb", "rational literal (step 7)"),
    // lang-fork: intended differences from Ruby
    ("mrbgems/mruby-hash-ext/mrblib/hash.rb", "`none` is a keyword (a parameter named none)"),
];

fn ruby_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "prism" && n != ".git") {
                ruby_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rb") {
            out.push(p);
        }
    }
}

#[test]
fn mruby_sources_compile() {
    // input nested up to the parser's limit needs ~4 MiB in a debug build,
    // more than a test thread gets by default
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(compile_all)
        .unwrap()
        .join()
        .unwrap();
}

fn compile_all() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("mruby");
    if !root.is_dir() {
        eprintln!("skipped: {} not found", root.display());
        return;
    }
    let mut files = Vec::new();
    ruby_files(&root, &mut files);
    files.sort();
    let mut problems = Vec::new();
    let mut removed = 0;
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let Ok(src) = std::fs::read_to_string(f) else {
            continue; // not UTF-8
        };
        let result = Interpreter::new().compile_file(&src, &rel, 1);
        let known = KNOWN_FAILURES.iter().any(|(k, _)| *k == rel);
        // lang-fork: Ruby code using removed syntax (exceptions, nil) is meant to fail
        let result = match result {
            Err(e) if e.to_string().contains("` was removed:") => {
                removed += 1;
                continue;
            }
            r => r,
        };
        match (result, known) {
            (Err(e), false) => problems.push(format!("{rel}: {e}")),
            (Ok(_), true) => problems.push(format!("{rel}: compiles now, remove it from KNOWN_FAILURES")),
            _ => {}
        }
    }
    eprintln!("{removed} files use syntax lang-fork removed (exceptions, nil)");
    assert!(
        problems.is_empty(),
        "{} of {} files:\n{}",
        problems.len(),
        files.len(),
        problems.join("\n")
    );
}
