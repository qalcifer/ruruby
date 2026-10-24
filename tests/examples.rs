//! The tour in `examples/` must keep running and printing what it shows.
//!
//! Each example's output is compared with `examples/expected/<name>.txt`.
//! After changing an example on purpose, regenerate them with
//! `UPDATE_EXPECTED=1 cargo test --test examples` and review the diff.

use ruruby::Interpreter;
use std::path::{Path, PathBuf};

fn examples() -> Vec<(String, PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            if p.extension().is_some_and(|x| x == "rb") {
                Some((p.file_stem()?.to_string_lossy().into_owned(), p))
            } else if p.join("main.rb").is_file() {
                // a multi-file example: its main.rb
                Some((p.file_name()?.to_string_lossy().into_owned(), p.join("main.rb")))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

fn run(path: &Path) -> String {
    let src = std::fs::read_to_string(path).unwrap();
    let mut interp = Interpreter::new();
    interp.capture_output();
    let result = interp
        .compile_file(&src, &path.to_string_lossy(), 1)
        .and_then(|irep| interp.run(irep));
    let mut out = interp.take_output();
    if let Err(e) = result {
        out.push_str(&format!("ERROR: {e}\n"));
    }
    out
}

#[test]
fn examples_print_what_they_show() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/expected");
    let update = std::env::var_os("UPDATE_EXPECTED").is_some();
    let mut problems = Vec::new();
    let list = examples();
    assert!(list.len() >= 12, "examples missing: {list:?}");
    for (name, path) in list {
        let out = run(&path);
        let expected_path = dir.join(format!("{name}.txt"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&expected_path, &out).unwrap();
            continue;
        }
        if out.contains("ERROR: ") {
            problems.push(format!("{name}: {}", out.lines().last().unwrap_or("")));
            continue;
        }
        match std::fs::read_to_string(&expected_path) {
            Ok(expected) if expected == out => {}
            Ok(_) => problems.push(format!("{name}: output differs from {}", expected_path.display())),
            Err(_) => problems.push(format!("{name}: no {}", expected_path.display())),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
