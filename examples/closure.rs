//! Closure-scope compile probe (memory goal): build a catalog of a tree, pick
//! root modules, then parse + compile ONLY the roots and their import closure
//! (module-level imports resolved by name through the catalog). Logs the
//! closure size and RSS/VmHWM, contrasting with full-tree retention.
//!
//! Usage: closure <dir> <roots:N>

use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).expect("closure <dir> <roots:N>");
    let roots: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);

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

    // Catalog phase (cheap): name -> entries (url,path,revision) + imports.
    let mut by_name: std::collections::HashMap<String, Vec<(String, String, Option<String>)>> =
        std::collections::HashMap::new();
    let mut imports_of: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for path in &files {
        let url = path.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let c = yrepo::Catalog::scan(url.clone(), text);
        if c.name.is_empty() {
            continue;
        }
        by_name.entry(c.name.clone()).or_default().push((
            url.clone(),
            path.to_string_lossy().to_string(),
            c.revision.clone(),
        ));
        imports_of
            .entry(c.name.clone())
            .or_insert_with(|| c.imports.iter().map(|(m, _)| m.clone()).collect());
        order.push(c.name.clone());
    }
    println!(
        "[closure] catalog names={} files={}",
        by_name.len(),
        files.len()
    );

    // Roots: first `roots` module names (deterministic).
    let mut names: Vec<String> = order
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();
    names.truncate(roots);

    // BFS closure over module names.
    let mut repo = yrepo::Repository::new();
    let mut in_repo: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = names;
    let mut ensured = 0usize;
    let started = Instant::now();
    let mut q = queue;
    while let Some(name) = q.pop() {
        if !in_repo.insert(name.clone()) {
            continue;
        }
        let Some(cands) = by_name.get(&name) else {
            continue; // unresolved import (module not in this tree)
        };
        // canonical: highest revision, else first.
        let chosen = cands
            .iter()
            .max_by_key(|(_, _, r)| r.clone().unwrap_or_default())
            .unwrap();
        let Ok(text) = std::fs::read_to_string(&chosen.1) else {
            continue;
        };
        repo.upsert(chosen.0.clone(), text);
        ensured += 1;
        if let Some(imps) = imports_of.get(&name) {
            for m in imps {
                if !in_repo.contains(m) {
                    q.push(m.clone());
                }
            }
        }
    }
    let out = repo.compile();
    let dt = started.elapsed().as_secs_f64();
    println!(
        "[closure] ensured={ensured} diags={} elapsed_s={dt:.2} rss_kb={} peak_kb={}",
        out.diagnostics.len(),
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
