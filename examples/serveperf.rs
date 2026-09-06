//! Serving-pipeline performance probe (memory goal): the catalog+closure
//! model as a standalone, language-server-free measurement. Phase 1 indexes a
//! `*.yang` tree with the parallel batch catalog scan
//! (`CatalogIndex::scan_many_files`, feature `parallel`; sequential
//! otherwise), Phase 2 materializes + compiles the open closure of the first
//! `--roots` module names over that catalog (`build_closure_repository`,
//! text-light ON unless `--no-text-light`). Reports wall time and RSS/VmHWM
//! per phase so serving memory can be checked against tree size without LSP
//! transport overhead.
//!
//! Usage:
//!   serveperf <dir> [--roots N] [--limit K] [--no-text-light]
//!
//! `--limit K` scans only the first K walked files (sorted), for stepwise
//! scale curves (each run is a fresh process, so RSS at a limit is that
//! tree's own footprint, like `catmem --stop-at`).
//!
//! RSS caveat: with the `parallel` feature the phase-1 scan runs 16+ parsers
//! at once, so the reported RSS/VmHWM is dominated by the TRANSIENT parse
//! high-water (allocator does not return freed CST arenas) and varies run to
//! run — it is NOT the retained catalog footprint. Retained footprint is best
//! measured sequentially (`catmem`; the resident language server scans
//! sequentially for this reason). Use serveperf for wall-time and closure
//! numbers; read retention from sequential runs.

use std::path::Path;
use std::time::Instant;

fn proc_field(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            let v: String = rest
                .trim()
                .trim_end_matches("kB")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return v.parse().ok();
        }
    }
    None
}
fn rss_kb() -> u64 {
    proc_field("VmRSS:").unwrap_or(0)
}
fn peak_kb() -> u64 {
    proc_field("VmHWM:").unwrap_or(0)
}

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .expect("serveperf <dir> [--roots N] [--limit K]");
    let mut roots_n: usize = 20;
    let mut limit: Option<usize> = None;
    let mut light = true;
    let mut root_name: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--roots" => {
                roots_n = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(roots_n);
                i += 2;
            }
            "--root-name" => {
                root_name = args.get(i + 1).cloned();
                i += 2;
            }
            "--limit" => {
                limit = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--no-text-light" => {
                light = false;
                i += 1;
            }
            _ => i += 1,
        }
    }

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    walk(Path::new(dir), &mut files);
    files.sort();
    if let Some(k) = limit {
        files.truncate(k);
    }
    println!(
        "[serveperf] files={} roots={roots_n} text_light={light} rss0_kb={} peak0_kb={}",
        files.len(),
        rss_kb(),
        peak_kb()
    );

    // Phase 1: parallel catalog of the whole tree (header facts only).
    let mut index = yrepo::CatalogIndex::default();
    let t1 = Instant::now();
    let scanned = index.scan_many_files(&files);
    let dt1 = t1.elapsed();
    let names: Vec<String> = index_names(&index);
    println!(
        "[serveperf] catalog scanned={scanned} named={} wall_s={:.2} rss_kb={} peak_kb={}",
        names.len(),
        dt1.as_secs_f64(),
        rss_kb(),
        peak_kb()
    );

    // Phase 2: materialize + compile the open closure (first N names, or one
    // named root with --root-name).
    let roots: Vec<String> = match root_name {
        Some(name) => vec![name],
        None => names.into_iter().take(roots_n).collect(),
    };
    let t2 = Instant::now();
    let repo = yrepo::build_closure_repository(&index, &roots, light, &|url| {
        std::fs::read_to_string(url).ok()
    });
    let out = repo.compile();
    let dt2 = t2.elapsed();
    let lib = out.library.as_ref();
    println!(
        "[serveperf] closure roots={} modules={} submodules={} diags={} wall_s={:.2} rss_kb={} peak_kb={}",
        roots.len(),
        lib.map(|l| l.modules().len()).unwrap_or(0),
        lib.map(|l| l.submodules().len()).unwrap_or(0),
        out.diagnostics.len(),
        dt2.as_secs_f64(),
        rss_kb(),
        peak_kb()
    );
}

/// All module names in the index, sorted (the deterministic root picker).
/// All distinct module names in the index, sorted (deterministic root picker).
fn index_names(index: &yrepo::CatalogIndex) -> Vec<String> {
    index.names()
}
