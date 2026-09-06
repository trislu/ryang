//! Component retention decomposition (memory-goal analysis tool).
//!
//! Non-parallel, read-only: upserts one file at a time (same loop shape as
//! `memstep`) and, per document, walks the public statement/token/comment
//! views to total: statement count, owned argument-string bytes
//! (`Argument.logical`), token-string bytes, comment count, and the retained
//! source bytes. Prints an aggregate summary so the cost of owned-string
//! duplication (the candidate rope-range redesign) can be estimated against
//! keeping whole sources.
//!
//! Usage: memcomp <dir>

use std::io::Write;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("memcomp <dir>");
        std::process::exit(2);
    };

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

    let mut repo = yrepo::Repository::new();
    let mut files_done = 0usize;
    let mut source_bytes: u64 = 0;
    let mut stmts_total: u64 = 0;
    let mut arg_bytes: u64 = 0;
    let mut tok_count: u64 = 0;
    let mut tok_bytes: u64 = 0;
    let mut comment_count: u64 = 0;
    for path in &files {
        let url = path.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        source_bytes += text.len() as u64;
        repo.upsert(url.clone(), text);
        if let Some(root) = repo.statement(&url) {
            for s in root.preorder() {
                stmts_total += 1;
                if let Some(a) = &s.arg {
                    arg_bytes += a.logical.len() as u64;
                }
            }
        }
        if let Some(ts) = repo.tokens(&url) {
            tok_count += ts.len() as u64;
            for t in ts {
                tok_bytes += t.text.len() as u64;
            }
        }
        if let Some(cs) = repo.comments(&url) {
            comment_count += cs.len() as u64;
        }
        files_done += 1;
    }
    let _ = std::io::stdout().flush();

    println!("files={files_done}");
    println!("source_bytes={source_bytes}");
    println!(
        "statements={stmts_total}  avg_per_file={:.1}",
        stmts_total as f64 / files_done as f64
    );
    println!(
        "arg_logical_bytes={arg_bytes}  ({:.1}% of source)",
        100.0 * arg_bytes as f64 / source_bytes as f64
    );
    println!(
        "token_text_bytes={tok_bytes}  tokens={tok_count}  ({:.1}% of source)",
        100.0 * tok_bytes as f64 / source_bytes as f64
    );
    println!(
        "owned_dup_bytes(arg+token)={}  ({:.1}% of source)",
        arg_bytes + tok_bytes,
        100.0 * (arg_bytes + tok_bytes) as f64 / source_bytes as f64
    );
    println!("comments={comment_count}");
    let per_file = source_bytes as f64 / files_done as f64;
    println!("avg_source_per_file={per_file:.0}B");
}
