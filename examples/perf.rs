//! Performance probe: ingest + compile a `*.yang` tree with yrepo and record
//! timing **and** memory footprint so regressions stay visible.
//!
//! A background thread samples the process resident set size (RSS) from
//! `/proc/self/status` every few ms for the whole run, so we can report the
//! observed peak *and* write the full footprint curve to a CSV. Peak RSS
//! (VmHWM) and CPU time are read from the kernel afterwards. On non-Linux the
//! memory numbers are simply unavailable (the timing still works).
//!
//! Usage:
//!   perf [--repeat N] [--csv out.csv] <dir-or-files...>
//!
//! Like `inspect`, ingest goes through `Repository::upsert_many_files` in one
//! batch (yrepo `parallel`): yrepo reads+parses off-thread and never buffers
//! the whole tree as text.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Resident set size in kB from /proc/self/status (Linux).
fn proc_field(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            let v: String = rest
                .trim()
                .trim_end_matches(" kB")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return v.parse().ok();
        }
    }
    None
}

fn rss_kb() -> Option<u64> {
    proc_field("VmRSS:")
}

fn hwm_kb() -> Option<u64> {
    proc_field("VmHWM:")
}

/// User+system CPU seconds from /proc/self/stat (fields 14/15, clock ticks).
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // `comm` (field 2) may contain spaces/parens; fields restart after the last
    // ')'. Tokens after it: index 0 == field 3 (state), so field 14 (utime) is
    // index 11 and field 15 (stime) is index 12.
    let after = stat.split(')').next_back()?;
    let ticks: Vec<&str> = after.split_whitespace().collect();
    if ticks.len() < 15 {
        return None;
    }
    let utime: u64 = ticks.get(11)?.parse().ok()?;
    let stime: u64 = ticks.get(12)?.parse().ok()?;
    Some((utime + stime) as f64 / 100.0)
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
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

fn main() {
    let mut repeat = 1usize;
    let mut csv: Option<PathBuf> = None;
    let mut dirs: Vec<String> = Vec::new();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--repeat" => {
                i += 1;
                repeat = raw.get(i).and_then(|s| s.parse().ok()).unwrap_or(1);
            }
            "--csv" => {
                i += 1;
                csv = raw.get(i).map(PathBuf::from);
            }
            a if a.starts_with('-') => {
                eprintln!("perf: unknown flag {a}");
                std::process::exit(2);
            }
            d => dirs.push(d.to_string()),
        }
        i += 1;
    }
    if dirs.is_empty() {
        eprintln!("usage: perf [--repeat N] [--csv out.csv] <dir-or-files...>");
        std::process::exit(2);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for d in &dirs {
        let p = Path::new(d);
        if p.is_dir() {
            walk(p, &mut files);
        } else if p.extension().is_some_and(|x| x == "yang") {
            files.push(p.to_path_buf());
        }
    }
    files.sort();
    files.dedup();
    let total_bytes: u64 = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    let files_n = files.len();
    let urls: Vec<(String, PathBuf)> = files
        .iter()
        .map(|p| (p.to_string_lossy().to_string(), p.clone()))
        .collect();

    // ---- baseline ----
    let base_rss = rss_kb();
    let cpu0 = cpu_seconds();

    // ---- RSS sampler (2 ms period) ----
    let stop = Arc::new(AtomicBool::new(false));
    let samples: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let s_stop = Arc::clone(&stop);
    let s_samples = Arc::clone(&samples);
    let sampler = std::thread::spawn(move || {
        let t0 = Instant::now();
        while !s_stop.load(Ordering::Relaxed) {
            if let Some(rss) = rss_kb() {
                s_samples
                    .lock()
                    .unwrap()
                    .push((t0.elapsed().as_millis() as u64, rss));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    // ---- timed ingest + compile ----
    let wall0 = Instant::now();
    let mut last_diags = 0usize;
    let mut loaded = files_n;
    for _ in 0..repeat {
        let mut repo = yrepo::Repository::new();
        loaded = repo.upsert_many_files(urls.clone());
        if loaded != files_n {
            eprintln!("perf: warning: loaded {loaded}/{files_n} files (some unreadable)");
        }
        let out = repo.compile();
        last_diags = out.diagnostics.len();
    }
    let wall = wall0.elapsed();

    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();

    let cpu1 = cpu_seconds();
    let hwm = hwm_kb();
    let peak = samples.lock().unwrap().iter().map(|&(_, r)| r).max();

    // ---- report ----
    let mb = |kb: Option<u64>| kb.map(|k| k as f64 / 1024.0);
    println!(
        "files: {files_n} loaded {loaded} ({} bytes, {:.1} MB source)",
        total_bytes,
        total_bytes as f64 / 1_048_576.0
    );
    println!(
        "ingest+compile x{repeat}: {:.3}s wall  ({:.0} files/s)",
        wall.as_secs_f64(),
        (files_n * repeat) as f64 / wall.as_secs_f64()
    );
    if let (Some(c0), Some(c1)) = (cpu0, cpu1) {
        println!("cpu: {:.3}s (user+sys)", (c1 - c0).max(0.0));
    }
    println!(
        "rss: base {:?}  peak-sampled {:?}  high-water(VmHWM) {:?}",
        mb(base_rss),
        mb(peak),
        mb(hwm)
    );
    println!(
        "samples: {} ({} diagnostics)",
        samples.lock().unwrap().len(),
        last_diags
    );

    if let Some(path) = csv {
        let mut s = String::from("t_ms,rss_kb\n");
        for (t, r) in samples.lock().unwrap().iter() {
            s.push_str(&format!("{t},{r}\n"));
        }
        if std::fs::write(&path, s).is_ok() {
            println!(
                "csv: wrote {} rows to {}",
                samples.lock().unwrap().len(),
                path.display()
            );
        } else {
            eprintln!("csv: failed to write {}", path.display());
        }
    }
}
