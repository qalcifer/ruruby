//! Runs ruby/spec (github.com/ruby/spec, cloned into `rubyspec/`) with
//! ruruby and reports how much of it passes, per core class and per
//! language feature.
//!
//! The real mspec runner needs much of Ruby's standard library; the specs
//! run under a small compatible shim instead (`tools/mspec/shim.rb`,
//! embedded here). Each spec file runs in a child process with a timeout;
//! the shim prints one line per example (`\x01P`, `F`, `E`, `S`).
//!
//! ```text
//! cargo run --release --bin rubyspec                        # language + core
//! cargo run --release --bin rubyspec -- core/array          # one directory
//! cargo run --release --bin rubyspec -- -v core/array/push_spec.rb
//! ```

use ruruby::Interpreter;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SHIM: &str = include_str!("../../tools/mspec/shim.rb");
const ROOT: &str = "rubyspec";
const DEFAULT_DIRS: &[&str] = &["language", "core"];
const TIMEOUT: Duration = Duration::from_secs(10);
const STACK_SIZE: usize = 1 << 30;

const USAGE: &str = "\
Usage: rubyspec [-v] [-j N] [--top N] [--groups] [PATH ...]
  Runs ruby/spec files (default: rubyspec/language and rubyspec/core) and
  reports passing examples per group (core class or language file).
  PATH is a spec file or directory, relative to rubyspec/ or not.
  -v        print every example that didn't pass
  -j N      parallel jobs (default: number of CPUs)
  --top N   how many of the most common errors to list (default 40)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        let file = PathBuf::from(&args[1]);
        return std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn(move || child(&file))
            .unwrap()
            .join()
            .unwrap_or(ExitCode::FAILURE);
    }
    parent(args)
}

// ---------------------------------------------------------------------------
// child: one spec file under the shim
// ---------------------------------------------------------------------------

fn child(file: &Path) -> ExitCode {
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string().replace('\n', " ");
        println!("\x01X panic\t{msg}");
    }));
    let mut interp = Interpreter::new();
    interp.vm.stack_budget = STACK_SIZE - (64 << 20);
    let file_name = std::fs::canonicalize(file).unwrap_or(file.to_path_buf()).to_string_lossy().into_owned();
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let shim = interp.compile_file(SHIM, "tools/mspec/shim.rb", 1).and_then(|i| interp.run(i));
        if let Err(e) = shim {
            println!("\x01X shim\t{e}");
            return;
        }
        let src = match std::fs::read_to_string(&file_name) {
            Ok(s) => s,
            Err(e) => {
                println!("\x01X read\t{e}");
                return;
            }
        };
        let r = interp.compile_file(&src, &file_name, 1).and_then(|i| interp.run(i));
        if let Err(e) = r {
            println!("\x01X load\t{e}");
        }
        // the examples run in the shim's at_exit block
        if let Err(e) = interp.run_at_exit() {
            println!("\x01X run\t{e}");
        }
    }));
    let _ = std::io::stdout().flush();
    if run.is_err() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

// ---------------------------------------------------------------------------
// parent: schedule, aggregate, report
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Counts {
    pass: usize,
    fail: usize,
    error: usize,
    skip: usize,
    /// examples of files that didn't finish (estimated from `it` lines)
    lost: usize,
    files: usize,
    broken_files: usize,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.pass += o.pass;
        self.fail += o.fail;
        self.error += o.error;
        self.skip += o.skip;
        self.lost += o.lost;
        self.files += o.files;
        self.broken_files += o.broken_files;
    }

    fn total(&self) -> usize {
        self.pass + self.fail + self.error + self.lost
    }
}

struct FileResult {
    file: PathBuf,
    counts: Counts,
    /// (kind, example, message) of everything that didn't pass
    problems: Vec<(char, String, String)>,
}

/// Examples a file declares, for files that die before reporting them.
fn count_its(src: &str) -> usize {
    src.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("it \"") || t.starts_with("it '") || t.starts_with("it(")
        })
        .count()
}

fn run_child(exe: &Path, file: &Path) -> FileResult {
    let declared = std::fs::read_to_string(file).map(|s| count_its(&s)).unwrap_or(0);
    let start = Instant::now();
    let mut child = Command::new(exe)
        .arg("--child")
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn rubyspec child");
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(rest) = line.strip_prefix('\x01') {
                lines.push(rest.to_string());
            }
        }
        lines
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(_)) => break "exited",
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break "timeout";
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break "died",
        }
    };
    let lines = reader.join().unwrap_or_default();
    let mut counts = Counts { files: 1, ..Counts::default() };
    let mut problems = Vec::new();
    let mut done = false;
    for l in &lines {
        let Some(kind) = l.chars().next() else { continue };
        let body = l.get(2..).unwrap_or("");
        let (what, msg) = body.split_once('\t').unwrap_or((body, ""));
        match kind {
            'P' => counts.pass += 1,
            'F' => counts.fail += 1,
            'E' => counts.error += 1,
            'S' => counts.skip += 1,
            'D' => done = true,
            _ => {}
        }
        if matches!(kind, 'F' | 'E' | 'S' | 'X') {
            problems.push((kind, what.to_string(), msg.to_string()));
        }
    }
    if !done {
        counts.broken_files = 1;
        let seen = counts.pass + counts.fail + counts.error + counts.skip;
        counts.lost = declared.saturating_sub(seen);
        problems.push(('X', status.to_string(), format!("file didn't finish ({status})")));
    }
    FileResult { file: file.to_path_buf(), counts, problems }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            let name = e.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if e.is_dir() && name != "fixtures" && name != "shared" {
                collect(&e, out);
            } else if name.ends_with("_spec.rb") {
                out.push(e);
            }
        }
    } else if path.is_file() {
        out.push(path.to_path_buf());
    }
}

/// `core/array`, `language/if`: the group a spec file belongs to.
fn group_of(file: &Path) -> String {
    let parts: Vec<String> = file.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let i = parts.iter().position(|p| p == ROOT).map_or(0, |i| i + 1);
    let rel = &parts[i.min(parts.len())..];
    match rel {
        [top, sub, _, ..] if top == "core" || top == "library" => format!("{top}/{sub}"),
        [top, file] => format!("{top}/{}", file.trim_end_matches("_spec.rb")),
        _ => rel.join("/"),
    }
}

/// Groups similar messages: inspected values and numbers vary per example.
fn normalize(kind: char, msg: &str) -> String {
    let msg = msg.trim();
    let msg = if kind == 'F' {
        // the values differ per example: group by the kind of expectation
        if let Some((want, got)) = msg.split_once(" but got ") {
            let want = want.trim_start_matches("Expected ").split(' ').next().unwrap_or("");
            let got = got.split(' ').next().unwrap_or("");
            format!("(expectation) raised {got} instead of {want}")
        } else if msg.ends_with("but no exception was raised") {
            let want = msg.trim_start_matches("Expected ").split([' ', '(']).next().unwrap_or("");
            format!("(expectation) {want} not raised")
        } else if msg.starts_with("Mock ") {
            "(expectation) mock call count".to_string()
        } else {
            "(expectation) wrong value".to_string()
        }
    } else {
        msg.to_string()
    };
    let mut s: String = msg.chars().take(110).collect();
    if s.len() < msg.len() {
        s.push('…');
    }
    s
}

fn parent(args: Vec<String>) -> ExitCode {
    let mut verbose = false;
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut top = 40;
    let mut paths = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" => verbose = true,
            "-j" => jobs = it.next().and_then(|n| n.parse().ok()).unwrap_or(jobs).max(1),
            "--top" => top = it.next().and_then(|n| n.parse().ok()).unwrap_or(top),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(a),
        }
    }
    if !Path::new(ROOT).is_dir() {
        eprintln!("rubyspec: no {ROOT}/ here (git clone --depth 1 https://github.com/ruby/spec {ROOT})");
        return ExitCode::FAILURE;
    }
    if paths.is_empty() {
        paths = DEFAULT_DIRS.iter().map(|s| s.to_string()).collect();
    }
    let mut files = Vec::new();
    for p in &paths {
        let direct = PathBuf::from(p);
        let path = if direct.exists() { direct } else { Path::new(ROOT).join(p) };
        if !path.exists() {
            eprintln!("rubyspec: {p}: not found");
            continue;
        }
        collect(&path, &mut files);
    }
    let exe = std::env::current_exe().expect("current exe");
    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::new()));
    let start = Instant::now();
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (files, next, results, exe) = (files.clone(), next.clone(), results.clone(), exe.clone());
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(f) = files.get(i) else { break };
                    let r = run_child(&exe, f);
                    results.lock().unwrap().push(r);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let mut results = Arc::try_unwrap(results).ok().unwrap().into_inner().unwrap();
    results.sort_by(|a, b| a.file.cmp(&b.file));

    let mut groups: BTreeMap<String, Counts> = BTreeMap::new();
    let mut total = Counts::default();
    let mut errors: HashMap<String, usize> = HashMap::new();
    let mut skips: HashMap<String, usize> = HashMap::new();
    for r in &results {
        groups.entry(group_of(&r.file)).or_default().add(&r.counts);
        total.add(&r.counts);
        for (kind, what, msg) in &r.problems {
            match kind {
                'S' => *skips.entry(normalize(*kind, msg)).or_default() += 1,
                _ => *errors.entry(normalize(*kind, msg)).or_default() += 1,
            }
            if verbose && *kind != 'S' {
                println!("[{kind}] {}: {what}\n      {msg}", r.file.display());
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<34} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7}\n",
        "group", "total", "pass", "fail", "error", "lost", "skip", "pass%"
    ));
    out.push_str(&"-".repeat(84));
    out.push('\n');
    let row = |name: &str, c: &Counts| {
        let pct = if c.total() == 0 { 0.0 } else { 100.0 * c.pass as f64 / c.total() as f64 };
        let broken = if c.broken_files > 0 { format!("  ({}/{} files broken)", c.broken_files, c.files) } else { String::new() };
        format!(
            "{name:<34} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {pct:>6.1}%{broken}\n",
            c.total(),
            c.pass,
            c.fail,
            c.error,
            c.lost,
            c.skip
        )
    };
    for (g, c) in &groups {
        out.push_str(&row(g, c));
    }
    out.push_str(&"-".repeat(84));
    out.push('\n');
    let mut by_top: BTreeMap<String, Counts> = BTreeMap::new();
    for (g, c) in &groups {
        by_top.entry(g.split('/').next().unwrap_or(g).to_string()).or_default().add(c);
    }
    for (t, c) in &by_top {
        out.push_str(&row(&format!("TOTAL {t}"), c));
    }
    out.push_str(&row("TOTAL", &total));
    out.push_str(&format!("\n{} files in {:.1}s\n", total.files, start.elapsed().as_secs_f64()));

    let mut list: Vec<_> = errors.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.push_str("\nMost common failures and errors (examples affected):\n");
    for (m, n) in list.iter().take(top) {
        out.push_str(&format!("  {n:>5}  {m}\n"));
    }
    let mut list: Vec<_> = skips.into_iter().collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.1));
    out.push_str("\nSkipped (not modelled by the shim):\n");
    for (m, n) in list.iter().take(10) {
        out.push_str(&format!("  {n:>5}  {m}\n"));
    }
    print!("{out}");
    ExitCode::SUCCESS
}
