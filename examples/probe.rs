//! Temporary probe: print yrepo diagnostics for one or more `.yang` files,
//! with context snippets around each error range.
//!
//! Ingest is parallel: `Repository::upsert_many_files` reads *and* parses the
//! whole batch off-thread (`yrepo` `parallel` feature); unreadable files are
//! skipped and reported via the loaded count / `contains` below.

use std::{path::PathBuf, time::Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: probe <file.yang> [file2.yang ...]");
        std::process::exit(2);
    }

    let start = Instant::now();
    let mut repo = yrepo::Repository::new();
    let loaded = repo.upsert_many_files(args.iter().map(|f| (f.clone(), PathBuf::from(f))));
    let out = repo.compile();
    let duration = start.elapsed();
    println!(
        "loaded {loaded}/{} files; {} total diagnostics",
        args.len(),
        out.diagnostics.len()
    );
    println!(
        "compiled {} files in {:.3}s",
        args.len(),
        duration.as_secs_f64()
    );

    for f in &args {
        if !repo.contains(f) {
            println!("\n== {f}: NOT LOADED (missing / unreadable / not UTF-8) ==");
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let mut diags: Vec<_> = out
            .diagnostics
            .iter()
            .filter(|d| d.url.as_deref() == Some(f.as_str()))
            .collect();
        diags.sort_by_key(|d| d.range.as_ref().map(|r| r.start).unwrap_or(0));
        println!(
            "\n== {f}  len={} : {} diagnostics ==",
            text.len(),
            diags.len()
        );
        for d in &diags {
            let Some(r) = &d.range else { continue };
            let s = text.get(r.start.min(text.len())..(r.start + 160).min(text.len()));
            let ctx = s.unwrap_or_default().replace('\n', "\\n");
            println!(
                "\n[{}] {:?} range {}..{} :: {}\n  …{}…",
                d.code.as_str(),
                d.severity,
                r.start,
                r.end,
                d.message,
                ctx
            );
        }
    }
}
