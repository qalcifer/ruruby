//! Command-line front end: the counterpart of mruby's `mruby` and `mirb` binaries.

use ruruby::{Error, Interpreter, value::Value};
use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;

const USAGE: &str = "\
Usage: ruruby [options] [programfile] [arguments]
  -e 'code'    run one line of script
  -c           check syntax only (compile without running)
  --dump       print the compiled bytecode before running (like `mruby -v`)
  --gc-stress  collect garbage at every safe point (slow; finds GC bugs)
  --version    print the version
  -h, --help   show this message
With no program, reads the script from stdin, or starts a REPL on a terminal.";

/// Native stack for the interpreter thread. Ruby→Ruby calls don't use it,
/// but natives calling back into Ruby (`each` with a block) do.
const STACK_SIZE: usize = 1 << 30;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || run(args))
        .expect("failed to spawn interpreter thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn run(args: Vec<String>) -> ExitCode {
    let mut dump = false;
    let mut check = false;
    let mut gc_stress = false;
    let mut code: Option<String> = None;
    let mut file: Option<String> = None;
    let mut it = args.into_iter();
    let mut script_args = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-e" => match it.next() {
                Some(c) => code = Some(c),
                None => {
                    eprintln!("ruruby: no code specified for -e");
                    return ExitCode::FAILURE;
                }
            },
            "--dump" | "-v" => dump = true,
            "-c" => check = true,
            "--gc-stress" => gc_stress = true,
            "--version" => {
                println!(
                    "ruruby {} (mruby-style Ruby interpreter in Rust)",
                    env!("CARGO_PKG_VERSION")
                );
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if code.is_none() && file.is_none() => file = Some(a),
            _ => {
                script_args.push(a);
                script_args.extend(it.by_ref());
            }
        }
    }

    let mut interp = Interpreter::new();
    interp.vm.stack_budget = STACK_SIZE - (64 << 20);
    interp.vm.heap.gc.set_stress(gc_stress);
    let args = script_args.into_iter().map(|a| interp.vm.str(a)).collect();
    let argv = interp.vm.array(args);
    interp.vm.set_const("ARGV", argv);

    let (name, src) = match (code, file) {
        (Some(c), _) => ("-e".to_string(), c),
        (None, Some(f)) => match std::fs::read_to_string(&f) {
            Ok(s) => (f, s),
            Err(e) => {
                eprintln!("ruruby: {f}: {e}");
                return ExitCode::FAILURE;
            }
        },
        (None, None) if std::io::stdin().is_terminal() => return repl(&mut interp),
        (None, None) => {
            let mut s = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut s) {
                eprintln!("ruruby: {e}");
                return ExitCode::FAILURE;
            }
            ("-".to_string(), s)
        }
    };

    let result = interp.compile_file(&src, &name, 1).and_then(|irep| {
        if dump {
            print!("{}", irep.dump(&interp.vm.syms));
        }
        if check {
            println!("Syntax OK");
            return Ok(Value::Nil);
        }
        let r = interp.run(irep);
        let at_exit = interp.run_at_exit();
        r.and_then(|v| at_exit.map(|_| v))
    });
    let _ = std::io::stdout().flush();
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A minimal `mirb`.
fn repl(interp: &mut Interpreter) -> ExitCode {
    let stdin = std::io::stdin();
    let mut buf = String::new();
    loop {
        print!(
            "{}",
            if buf.is_empty() {
                "ruruby> "
            } else {
                "ruruby* "
            }
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            println!();
            return ExitCode::SUCCESS;
        }
        buf.push_str(&line);
        match interp.eval(&buf) {
            Ok(v) => println!(" => {}", interp.inspect(&v)),
            // unfinished input (e.g. an open `def`): keep reading
            Err(Error::Syntax(m)) if m.contains("found Eof") => continue,
            Err(e) => println!("{e}"),
        }
        buf.clear();
    }
}
