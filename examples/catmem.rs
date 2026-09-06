//! Catalog-only retention probe: scan files with `Catalog::scan` (transient
//! parse, header fields retained only) and log RSS/VmHWM every N files.
//! Usage: catmem <dir> [--log-every N] [--sleep-ms M] [--stop-at N]

use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("catmem <dir> [--log-every N] [--stop-at N]");
        std::process::exit(2);
    };
    let mut log_every: usize = 100;
    let mut sleep_ms: u64 = 0;
    let mut stop_at: Option<usize> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--log-every" => log_every = args[i + 1].parse().unwrap_or(log_every),
            "--sleep-ms" => sleep_ms = args[i + 1].parse().unwrap_or(0),
            "--stop-at" => stop_at = Some(args[i + 1].parse().unwrap()),
            _ => {}
        }
        i += 2;
    }
    fn walk(root: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(e) = std::fs::read_dir(root) {
            for e in e.flatten() {
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
    println!("[catmem] start files={total} log_every={log_every} stop_at={stop_at:?}");
    println!("[catmem] rss0_kb={} peak_kb={}", rss_kb(), peak_kb());
    let mut cats: Vec<yrepo::Catalog> = Vec::with_capacity(total);
    let mut bad = 0usize;
    let started = Instant::now();
    for (idx, path) in files.iter().enumerate() {
        if let Some(stop) = stop_at
            && idx > stop
        {
            break;
        }
        let url = path.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let c = yrepo::Catalog::scan(url, text);
        if !c.parse_ok {
            bad += 1;
        }
        cats.push(c);
        let n = idx + 1;
        if n % log_every == 0 {
            let dt = started.elapsed().as_secs_f64();
            println!(
                "[catmem] files={n}/{total} elapsed_s={dt:.2} rss_kb={} peak_kb={} bad={bad}",
                rss_kb(),
                peak_kb()
            );
            if sleep_ms > 0 {
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
        }
    }
    let dt = started.elapsed().as_secs_f64();
    println!(
        "[catmem] done retained={} rss_kb={} peak_kb={} elapsed_s={dt:.2}",
        cats.len(),
        rss_kb(),
        peak_kb()
    );
}

fn status_kb(tag: &str) -> u64 {
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
    status_kb("VmRSS:")
}
fn peak_kb() -> u64 {
    status_kb("VmHWM:")
}
