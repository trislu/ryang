//! Closure-scope compile probe (memory goal): build a `CatalogIndex` of a
//! tree, pick root modules, then parse + compile ONLY the roots and their
//! reachable closure (imports + includes resolved by name through the
//! catalog) via `yrepo::build_closure_repository`. Logs the closure size and
//! RSS/VmHWM, contrasting with full-tree retention.
//!
//! Usage: closure <dir> <roots:N> [--text-light]

use std::path::Path;
use std::time::Instant;

use yrepo::{Catalog, CatalogIndex, build_closure_repository};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).expect("closure <dir> <roots:N> [--text-light]");
    let roots: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let light = args.iter().any(|a| a == "--text-light");

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

    // Catalog phase (cheap): one transient parse per file, header facts only.
    let mut index = CatalogIndex::default();
    let mut names: Vec<String> = Vec::new();
    let t0 = Instant::now();
    for path in &files {
        let url = path.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let c = Catalog::scan(url, text);
        if !c.name.is_empty() {
            names.push(c.name.clone());
            index.push(c);
        }
    }
    println!(
        "[closure] catalog files={} named={} elapsed_s={:.2} rss_kb={} peak_kb={}",
        files.len(),
        index.len(),
        t0.elapsed().as_secs_f64(),
        rss_kb(),
        peak_kb()
    );

    // Roots: first `roots` module names (deterministic).
    names.sort();
    names.dedup();
    names.truncate(roots);

    // Closure parse + compile: documents read from disk on demand by url.
    let t1 = Instant::now();
    let repo = build_closure_repository(&index, &names, light, &|url| {
        std::fs::read_to_string(url).ok()
    });
    let out = repo.compile();
    let lib = out.library.as_ref();
    println!(
        "[closure] roots={} modules={} submodules={} diags={} elapsed_s={:.2} rss_kb={} peak_kb={}",
        names.len(),
        lib.map(|l| l.modules().len()).unwrap_or(0),
        lib.map(|l| l.submodules().len()).unwrap_or(0),
        out.diagnostics.len(),
        t1.elapsed().as_secs_f64(),
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
