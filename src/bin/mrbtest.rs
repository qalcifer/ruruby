//! Runs mruby's own test suite (`mruby/test/t/*.rb`) with ruruby — the
//! counterpart of mruby's `mrbtest`.
//!
//! Each file runs in a child process with a timeout, so a crash or an endless
//! loop only affects that file. Files that don't parse as a whole are split
//! into top-level chunks and the chunks that do parse still run, so one
//! unsupported construct doesn't hide every test in the file.
//!
//! ```text
//! cargo run --release --bin mrbtest                 # all of mruby/test/t
//! cargo run --release --bin mrbtest -- -v array.rb  # one file, with failure details
//! ```

use ruruby::Interpreter;
use ruruby::value::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HARNESS: &str = include_str!("../../tests/mruby/assert.rb");
const DEFAULT_DIR: &str = "mruby/test/t";
const TIMEOUT: Duration = Duration::from_secs(20);

const USAGE: &str = "\
Usage: mrbtest [-v] [-j N] [--top N] [--gc-stress] [FILE|DIR ...]
  Runs mruby test files (default: mruby/test/t/*.rb) and reports results.
  -v           print every failure/crash/syntax error message
  -j N         parallel jobs (default: number of CPUs)
  --top N      how many of the most common errors to list (default 15)
  --gc-stress  collect garbage at every safe point (slow; finds GC bugs)";

const STACK_SIZE: usize = 1 << 30;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        let file = PathBuf::from(&args[1]);
        let gc_stress = args.get(2).map(String::as_str) == Some("--gc-stress");
        // natives calling back into Ruby recurse on the native stack
        return std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn(move || child(&file, gc_stress))
            .unwrap()
            .join()
            .unwrap_or(ExitCode::FAILURE);
    }
    parent(args)
}

// ---------------------------------------------------------------------------
// child: run one file, report progress on stdout
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Stats {
    ok: i64,
    ko: i64,
    crash: i64,
    skip: i64,
    warn: i64,
    /// assertions inside chunks that failed to parse
    syntax: i64,
    /// chunks that raised outside of any `assert`
    toplevel: i64,
}

impl Stats {
    fn add(&mut self, o: &Stats) {
        self.ok += o.ok;
        self.ko += o.ko;
        self.crash += o.crash;
        self.skip += o.skip;
        self.warn += o.warn;
        self.syntax += o.syntax;
        self.toplevel += o.toplevel;
    }
}

fn emit(kind: &str, msg: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "MSG\t{kind}\t{}",
        msg.replace('\\', "\\\\").replace('\n', "\\n")
    );
}

fn global(interp: &mut Interpreter, name: &str) -> Value {
    let s = interp.vm.intern(name);
    interp.vm.globals.get(&s).cloned().unwrap_or(Value::Nil)
}

fn global_int(interp: &mut Interpreter, name: &str) -> i64 {
    match global(interp, name) {
        Value::Int(i) => i,
        _ => 0,
    }
}

/// Counts `assert(...)` / `assert '...'` calls — the number of tests a chunk declares.
fn count_asserts(src: &str) -> i64 {
    src.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("assert(") || t.starts_with("assert ") || t == "assert do"
        })
        .count() as i64
}

/// Splits a file into top-level chunks: a new chunk starts at every line in
/// column 0 that isn't a continuation (`end`, `}`, `else`, ...).
fn split_chunks(src: &str) -> Vec<(usize, String)> {
    const CONT: &[&str] = &[
        "end", "}", ")", "]", "else", "elsif", "when", "in ", "rescue", "ensure", ".",
    ];
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut start = 1;
    let mut in_doc = false;
    for (i, line) in src.lines().enumerate() {
        if line.starts_with("=begin") {
            in_doc = true;
        }
        let new_stmt = !in_doc
            && line.starts_with(|c: char| !c.is_whitespace() && c != '#')
            && !CONT.iter().any(|k| {
                line.starts_with(k)
                    && !line[k.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
            });
        if line.starts_with("=end") {
            in_doc = false;
        }
        if new_stmt && !cur.trim().is_empty() {
            chunks.push((start, std::mem::take(&mut cur)));
            start = i + 1;
        }
        if cur.trim().is_empty() && new_stmt {
            start = i + 1;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        chunks.push((start, cur));
    }
    chunks
}

fn child(file: &Path, gc_stress: bool) -> ExitCode {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            emit("toplevel", &format!("cannot read {}: {e}", file.display()));
            return ExitCode::FAILURE;
        }
    };
    let last_panic: Arc<Mutex<String>> = Arc::default();
    {
        let last_panic = last_panic.clone();
        std::panic::set_hook(Box::new(move |info| {
            *last_panic.lock().unwrap() = info.to_string();
        }));
    }

    let mut interp = Interpreter::new();
    interp.vm.stack_budget = STACK_SIZE - (64 << 20);
    interp.vm.heap.gc.set_stress(gc_stress);
    interp.capture_output();
    if let Err(e) = interp.eval(HARNESS) {
        emit("toplevel", &format!("harness failed to load: {e}"));
        return ExitCode::FAILURE;
    }

    let mut extra = Stats::default();
    let mut reported = 0;
    // Run top-level chunks one by one: a top-level error or a syntax error
    // then only costs its own chunk instead of the rest of the file.
    let chunks = split_chunks(&src);

    let mut chunks = chunks.into_iter().peekable();
    while let Some((line, mut chunk)) = chunks.next() {
        // a chunk that ends too early (heredoc bodies and `=begin` blocks sit
        // in column 0) is retried together with the next ones
        let mut compiled = interp.compile_file(&chunk, &file.to_string_lossy(), line as u32);
        while let Err(e) = &compiled {
            let m = e.to_string();
            let premature =
                m.contains("found Eof") || m.contains("before EOF") || m.contains("unterminated");
            if !premature || chunks.peek().is_none() {
                break;
            }
            let (_, next) = chunks.next().unwrap();
            chunk.push_str(&next);
            compiled = interp.compile_file(&chunk, &file.to_string_lossy(), line as u32);
        }
        {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "RUN\t{line}");
            let _ = out.flush();
        }
        match compiled {
            Err(e) => {
                let n = count_asserts(&chunk);
                extra.syntax += n;
                let msg = e.to_string();
                // report the line within the file, not within the chunk
                emit("syntax", &format!("{msg} [{n} asserts]"));
            }
            Ok(irep) => {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| interp.run(irep)));
                match r {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        extra.toplevel += 1;
                        emit("toplevel", &format!("line {line}: {e}"));
                    }
                    Err(_) => {
                        interp.vm.recover();
                        extra.crash += 1;
                        let p = last_panic.lock().unwrap().clone();
                        emit("panic", &format!("line {line}: {p}"));
                    }
                }
            }
        }
        // stream new failure messages and the running totals
        if let Value::Array(a) = global(&mut interp, "$asserts") {
            let heap = &interp.vm.heap;
            let msgs: Vec<String> = heap[a].borrow()[reported..]
                .iter()
                .map(|m| match m {
                    Value::Str(s) => heap[s].borrow().clone(),
                    _ => String::new(),
                })
                .collect();
            reported += msgs.len();
            for m in msgs {
                let kind = if m.starts_with("Fail: ") {
                    "fail"
                } else {
                    "crash"
                };
                emit(kind, &m);
            }
        }
        let mut st = Stats {
            ok: global_int(&mut interp, "$ok_test"),
            ko: global_int(&mut interp, "$ko_test"),
            crash: global_int(&mut interp, "$kill_test"),
            skip: global_int(&mut interp, "$skip_test"),
            warn: global_int(&mut interp, "$warning_test"),
            ..Stats::default()
        };
        st.add(&extra);
        let mut out = std::io::stdout().lock();
        let _ = writeln!(
            out,
            "STAT\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            st.ok, st.ko, st.crash, st.skip, st.warn, st.syntax, st.toplevel
        );
        let _ = out.flush();
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// parent: schedule children, aggregate, report
// ---------------------------------------------------------------------------

struct FileResult {
    name: String,
    declared: i64,
    stats: Stats,
    msgs: Vec<(String, String)>,
    status: &'static str,
    secs: f64,
}

fn run_child(exe: &Path, file: &Path, gc_stress: bool) -> FileResult {
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let declared = std::fs::read_to_string(file)
        .map(|s| count_asserts(&s))
        .unwrap_or(0);
    let start = Instant::now();
    let mut child = Command::new(exe)
        .arg("--child")
        .arg(file)
        .args(gc_stress.then_some("--gc-stress"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn mrbtest child");
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut stats = Stats::default();
        let mut msgs = Vec::new();
        let mut last_run = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            match parts.as_slice() {
                ["STAT", rest @ ..] => {
                    let n: Vec<i64> = rest
                        .join("\t")
                        .split('\t')
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    if n.len() == 7 {
                        stats = Stats {
                            ok: n[0],
                            ko: n[1],
                            crash: n[2],
                            skip: n[3],
                            warn: n[4],
                            syntax: n[5],
                            toplevel: n[6],
                        };
                    }
                }
                ["RUN", line] => last_run = line.to_string(),
                ["MSG", kind, msg] => {
                    msgs.push((
                        kind.to_string(),
                        msg.replace("\\n", "\n").replace("\\\\", "\\"),
                    ));
                }
                _ => {}
            }
        }
        (stats, msgs, last_run)
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => break "ok",
            Ok(Some(_)) => break "died",
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break "timeout";
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break "died",
        }
    };
    let (stats, mut msgs, last_run) = reader.join().unwrap_or_default();
    if status != "ok" {
        msgs.push((
            "toplevel".into(),
            format!("{status} while running the chunk at line {last_run}"),
        ));
    }
    FileResult {
        name,
        declared,
        stats,
        msgs,
        status,
        secs: start.elapsed().as_secs_f64(),
    }
}

fn collect_files(paths: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let paths = if paths.is_empty() {
        vec![DEFAULT_DIR.to_string()]
    } else {
        paths.to_vec()
    };
    for p in paths {
        let mut path = PathBuf::from(&p);
        if !path.exists() {
            path = Path::new(DEFAULT_DIR).join(&p);
        }
        if path.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "rb"))
                .collect();
            v.sort();
            files.extend(v);
        } else if path.exists() {
            files.push(path);
        } else {
            eprintln!("mrbtest: {p}: not found (clone mruby into ./mruby, or pass a path)");
        }
    }
    files
}

/// Groups similar messages: drops line numbers, assert names and inspected values.
fn normalize(kind: &str, msg: &str) -> String {
    let first = msg.lines().next().unwrap_or("");
    match kind {
        "syntax" => {
            let s = first.split_once(": ").map_or(first, |(_, t)| t);
            s.split(" [")
                .next()
                .unwrap_or(s)
                .trim_end_matches(" (SyntaxError)")
                .to_string()
        }
        "crash" => match first.split_once(" => ") {
            Some((head, m)) => format!("{}: {m}", head.split(':').next().unwrap_or(head)),
            None => first.to_string(),
        },
        "toplevel" | "panic" => first.split_once(": ").map_or(first, |(_, t)| t).to_string(),
        _ => first.to_string(),
    }
}

fn parent(args: Vec<String>) -> ExitCode {
    let mut verbose = false;
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut top = 15;
    let mut gc_stress = false;
    let mut paths = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" => verbose = true,
            "-j" => jobs = it.next().and_then(|n| n.parse().ok()).unwrap_or(jobs),
            "--top" => top = it.next().and_then(|n| n.parse().ok()).unwrap_or(top),
            "--gc-stress" => gc_stress = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(a),
        }
    }
    let files = collect_files(&paths);
    if files.is_empty() {
        return ExitCode::FAILURE;
    }
    let exe = std::env::current_exe().expect("current exe");

    let next = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<FileResult>>> = Arc::default();
    let files = Arc::new(files);
    let workers: Vec<_> = (0..jobs.max(1))
        .map(|_| {
            let (next, results, files, exe) =
                (next.clone(), results.clone(), files.clone(), exe.clone());
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(f) = files.get(i) else { break };
                    let r = run_child(&exe, f, gc_stress);
                    results.lock().unwrap().push(r);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let mut results = std::mem::take(&mut *results.lock().unwrap());
    results.sort_by(|a, b| a.name.cmp(&b.name));

    if verbose {
        for r in &results {
            if r.msgs.is_empty() {
                continue;
            }
            println!("==== {} ====", r.name);
            for (kind, m) in &r.msgs {
                println!("[{kind}] {m}");
            }
            println!();
        }
    }

    println!(
        "{:<22} {:>7} {:>6} {:>5} {:>6} {:>7} {:>5} {:>5} {:>6}  {:>6}  status",
        "file", "asserts", "OK", "KO", "Crash", "Syntax", "Skip", "Warn", "TopErr", "pass%"
    );
    let mut total = Stats::default();
    let mut declared = 0;
    for r in &results {
        let s = &r.stats;
        total.add(s);
        // asserts inside loops run more often than they're written
        let counted = s.ok + s.ko + s.crash + s.skip + s.warn + s.syntax;
        let declared_here = r.declared.max(counted);
        declared += declared_here;
        let pct = if declared_here > 0 {
            100.0 * s.ok as f64 / declared_here as f64
        } else {
            0.0
        };
        let status = if r.status == "ok" {
            String::new()
        } else {
            format!("{} ({:.1}s)", r.status, r.secs)
        };
        println!(
            "{:<22} {:>7} {:>6} {:>5} {:>6} {:>7} {:>5} {:>5} {:>6}  {:>5.1}%  {}",
            r.name,
            declared_here,
            s.ok,
            s.ko,
            s.crash,
            s.syntax,
            s.skip,
            s.warn,
            s.toplevel,
            pct,
            status
        );
    }
    let pct = if declared > 0 {
        100.0 * total.ok as f64 / declared as f64
    } else {
        0.0
    };
    println!("{}", "-".repeat(100));
    println!(
        "{:<22} {:>7} {:>6} {:>5} {:>6} {:>7} {:>5} {:>5} {:>6}  {:>5.1}%",
        "TOTAL",
        declared,
        total.ok,
        total.ko,
        total.crash,
        total.syntax,
        total.skip,
        total.warn,
        total.toplevel,
        pct
    );

    // the most common causes, to decide what to implement next
    for (kind, title) in [
        ("syntax", "Most common syntax errors (asserts affected)"),
        ("crash", "Most common exceptions inside asserts"),
        ("toplevel", "Most common top-level errors"),
        ("panic", "Interpreter panics (bugs)"),
    ] {
        let mut counts: HashMap<String, i64> = HashMap::new();
        for r in &results {
            for (k, m) in &r.msgs {
                if k == kind {
                    let weight = if kind == "syntax" {
                        m.rsplit_once(" [")
                            .and_then(|(_, n)| n.split(' ').next()?.parse().ok())
                            .unwrap_or(1)
                    } else {
                        1
                    };
                    *counts.entry(normalize(kind, m)).or_default() += weight;
                }
            }
        }
        if counts.is_empty() {
            continue;
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        println!("\n{title}:");
        for (m, n) in v.into_iter().take(top) {
            let m: String = m.chars().take(110).collect();
            println!("  {n:>5}  {m}");
        }
    }

    if total.ko == 0 && total.crash == 0 && total.syntax == 0 && total.toplevel == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
