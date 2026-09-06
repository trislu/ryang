//! Temporary diagnostic inspector: compile a directory of *.yang files with
//! yrepo and summarize problems (parse errors, full-document errors, codes).
//!
//! Ingest is parallel: `Repository::upsert_many_files` reads *and* parses
//! every file off-thread (`yrepo` `parallel` feature) without ever buffering
//! the whole workspace as text.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

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
    let start = Instant::now();
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let root = Path::new(&dir);
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();

    let total = files.len();
    let mut repo = yrepo::Repository::new();
    // Parallel ingest: yrepo reads *and* parses every file off-thread
    // (`parallel` feature); only the in-flight file is in memory.
    let loaded = repo.upsert_many_files(files.into_iter().map(|p| {
        let url = p.to_string_lossy().to_string();
        (url, p)
    }));
    let out = repo.compile();
    let duration = start.elapsed();
    println!("compiled {} files in {:.3}s", total, duration.as_secs_f64());
    println!("files: {total} (loaded {loaded})");
    println!("total diagnostics: {}", out.diagnostics.len());
    let errors = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == yrepo::Severity::Error)
        .count();
    println!(
        "errors: {errors}   warnings: {}",
        out.diagnostics.len() - errors
    );

    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    for d in &out.diagnostics {
        *by_code.entry(d.code.as_str().to_owned()).or_default() += 1;
    }
    println!("by code:");
    for (k, v) in by_code {
        println!("  {v:>5}  {k}");
    }

    // Files with a full-document parse error (range covers most of the file).
    let mut full_doc = 0;
    println!("\n-- parse diagnostics with long ranges --");
    for d in &out.diagnostics {
        if !matches!(
            d.code,
            yrepo::DiagnosticCode::ParseError | yrepo::DiagnosticCode::NotYangDocument
        ) {
            continue;
        }
        let Some(r) = &d.range else { continue };
        if r.end - r.start > 200 {
            full_doc += 1;
            let url = d.url.as_deref().unwrap_or("?");
            let text = std::fs::read_to_string(url).unwrap_or_default();
            let frac = if text.is_empty() {
                0.0
            } else {
                (r.end - r.start) as f64 / text.len() as f64
            };
            let ctx = text
                .get(r.start.min(text.len())..(r.start + 120).min(text.len()))
                .map(|s| s.replace('\n', "\\n"))
                .unwrap_or_default();
            println!(
                "len>200  frac={frac:.2}  {}:{}..{} :: {} :: …{}…",
                d.code.as_str(),
                r.start,
                r.end,
                url,
                ctx
            );
        }
    }
    println!("\nfull-doc-ish parse diagnostics (range>200B): {full_doc}");

    // PHASE 0 metric: whole-file parse *collapses* — a parse-error whose range
    // covers essentially the whole document (the "single top-level ERROR"
    // failure mode that error-localization work targets).
    let mut collapses = 0;
    for d in &out.diagnostics {
        if d.code != yrepo::DiagnosticCode::ParseError {
            continue;
        }
        let Some(r) = &d.range else { continue };
        let Some(url) = d.url.as_deref() else {
            continue;
        };
        let len = std::fs::read_to_string(url).map(|s| s.len()).unwrap_or(0);
        if len == 0 {
            continue;
        }
        let frac = (r.end - r.start) as f64 / len as f64;
        if r.start == 0 && frac >= 0.95 {
            collapses += 1;
        }
    }
    println!("whole-file parse collapses: {collapses}");
}
