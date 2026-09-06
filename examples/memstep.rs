//! Stepwise memory-boundary probe (goal: giant-corpus memory work).
//!
//! Non-concurrent ingest: upserts ONE file at a time into a Repository, logs
//! progress + RSS/peak every `--log-every` files and (optionally) sleeps
//! `--sleep-ms` per interval to observe the memory waterline. Periodically
//! compiles the whole repository (every `--compile-every`) so semantic-phase
//! memory is included. Logs go to stderr and, when `MEMSTEP_LOG` is set, to
//! that file (append) — leave durable "last words" at the exhaustion edge.
//!
//! Usage:
//!   memstep <dir> [--log-every N] [--sleep-ms M] [--compile-every K]
//!             [--start-at N] [--stop-at N]
//!
//! RSS / VmHWM are read from /proc/self/status (Linux). No `parallel`
//! feature is required or used here.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const USAGE: &str =
    "memstep <dir> [--log-every N] [--sleep-ms M] [--compile-every K] [--start-at N] [--stop-at N]";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let dir = &args[1];
    let mut log_every: usize = 200;
    let mut sleep_ms: u64 = 0;
    let mut compile_every: usize = 0; // 0 = never
    let mut start_at: usize = 0;
    let mut stop_at: Option<usize> = None;
    let mut light = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--text-light" => {
                light = true;
                i += 1;
            }
            "--log-every" => {
                log_every = args[i + 1].parse().unwrap_or(log_every);
                i += 2;
            }
            "--sleep-ms" => {
                sleep_ms = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--compile-every" => {
                compile_every = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--start-at" => {
                start_at = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--stop-at" => {
                stop_at = Some(args[i + 1].parse().unwrap());
                i += 2;
            }
            _ => i += 1,
        }
    }

    let log_target = std::env::var("MEMSTEP_LOG").ok();
    let mut file = log_target.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut log = |msg: &str| {
        eprintln!("{msg}");
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{msg}");
        }
    };

    // Collect candidate files first (paths only, no content).
    fn walk(root: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "yang") {
                    out.push(p);
                }
            }
        }
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    walk(Path::new(dir), &mut files);
    files.sort();
    let total = files.len();
    log(&format!(
        "[memstep] start dir={dir} files={total} log_every={log_every} sleep_ms={sleep_ms} compile_every={compile_every} start_at={start_at} stop_at={stop_at:?}"
    ));
    log(&format!(
        "[memstep] rss0_kb={} peak_kb={}",
        rss_kb(),
        peak_kb()
    ));

    let mut repo = yrepo::Repository::new();
    if light {
        repo.set_text_light(true);
        log("[memstep] text_light=on");
    }
    let mut source_bytes: u64 = 0;
    let started = Instant::now();
    for (idx, path) in files.iter().enumerate() {
        if idx < start_at {
            continue;
        }
        if let Some(stop) = stop_at
            && idx > stop
        {
            break;
        }
        let url = path.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        source_bytes += text.len() as u64;
        repo.upsert(url, text);
        let n = idx + 1;
        if n % log_every == 0 {
            let dt = started.elapsed().as_secs_f64();
            log(&format!(
                "[memstep] files={n}/{total} src_bytes={source_bytes} elapsed_s={dt:.1} rss_kb={} peak_kb={}",
                rss_kb(),
                peak_kb()
            ));
            if compile_every > 0 && n % compile_every == 0 {
                let out = repo.compile();
                log(&format!(
                    "[memstep] compiled files={n} diags={} rss_kb={} peak_kb={}",
                    out.diagnostics.len(),
                    rss_kb(),
                    peak_kb()
                ));
            }
            if sleep_ms > 0 {
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
            let _ = std::io::stdout().flush();
        }
    }
    let dt = started.elapsed().as_secs_f64();
    log(&format!(
        "[memstep] done files={total} src_bytes={source_bytes} elapsed_s={dt:.1} rss_kb={} peak_kb={}",
        rss_kb(),
        peak_kb()
    ));
}

fn proc_self_status_kb(tag: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix(tag)
            && let Some(num) = rest.trim().strip_suffix("kB")
            && let Ok(v) = num.trim().parse::<u64>()
        {
            return v;
        }
    }
    0
}

fn rss_kb() -> u64 {
    proc_self_status_kb("VmRSS:")
}

fn peak_kb() -> u64 {
    proc_self_status_kb("VmHWM:")
}
