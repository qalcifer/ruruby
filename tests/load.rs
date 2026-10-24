//! `require`, `require_relative`, `load` and `__dir__` with files on disk.

use ruruby::Interpreter;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ruruby-load-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    dir
}

#[test]
fn require_family() {
    let dir = scratch("family");
    std::fs::write(dir.join("lib/helper.rb"), "$count = ($count || 0) + 1\ndef helper = :helped\n").unwrap();
    std::fs::write(dir.join("sub/rel.rb"), "REL_DIR = __dir__\nrequire_relative '../lib/helper'\n").unwrap();
    std::fs::write(dir.join("bad.rb"), "def (\n").unwrap();
    let main = dir.join("main.rb");
    std::fs::write(
        &main,
        r#"
p require_relative('sub/rel'), require_relative('lib/helper'), helper, $count
$LOAD_PATH << __dir__ + '/lib'
p require('helper'), $count
p load(__dir__ + '/lib/helper.rb'), $count
p (begin; require 'nope'; rescue LoadError => e; e.message; end)
p (begin; require_relative 'bad'; rescue SyntaxError => e; e.class; end)
p REL_DIR.end_with?('sub'), $LOADED_FEATURES.size, $".equal?($LOADED_FEATURES)
"#,
    )
    .unwrap();
    let mut interp = Interpreter::new();
    interp.capture_output();
    let src = std::fs::read_to_string(&main).unwrap();
    let irep = interp.compile_file(&src, &main.to_string_lossy(), 1).unwrap();
    interp.run(irep).unwrap();
    assert_eq!(
        interp.take_output(),
        "true\nfalse\n:helped\n1\nfalse\n1\ntrue\n2\n\"cannot load such file -- nope\"\nSyntaxError\ntrue\n3\ntrue\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn require_relative_needs_a_file() {
    let mut interp = Interpreter::new();
    let err = interp.eval("require_relative 'x'").unwrap_err().to_string();
    assert!(err.contains("cannot infer basepath"), "{err}");
}
