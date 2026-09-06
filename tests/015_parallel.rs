//! Parallel file ingest & compilation (`parallel` cargo feature, D20).
//!
//! `Repository::upsert_many_files` reads **and** parses a batch of documents
//! in parallel when the `parallel` feature is enabled and falls back to a
//! plain sequential loop otherwise. These tests assert the two *behaviors*
//! that must hold in both modes — they never depend on threads actually
//! running:
//!
//! 1. a batch ingest produces exactly the same `Library` and diagnostics as
//!    the equivalent sequence of `upsert` calls (ordering included);
//! 2. repeated compiles of the same repository are deterministic.
mod test_utils;

use std::fs;
use std::ops::Range;

use test_utils::SAMPLE_DIR;
use yrepo::{Diagnostic, DiagnosticCode, Library, Repository, Severity};

/// All `.yang` fixtures as sorted `(url, source)` pairs.
fn read_samples() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for entry in fs::read_dir(SAMPLE_DIR).unwrap().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("yang") {
            let content = fs::read_to_string(&p).unwrap();
            v.push((p.to_string_lossy().to_string(), content));
        }
    }
    v.sort();
    v
}

/// A diagnostics entry reduced to fully comparable parts.
type DiagKey = (
    Option<String>,
    Option<Range<usize>>,
    Severity,
    DiagnosticCode,
    String,
);

fn diag_key(d: &Diagnostic) -> DiagKey {
    (
        d.url.as_ref().map(|s| s.to_string()),
        d.range.clone(),
        d.severity,
        d.code,
        d.message.clone(),
    )
}

fn diag_keys(out: &yrepo::Outcome) -> Vec<DiagKey> {
    out.diagnostics.iter().map(diag_key).collect()
}

/// A structural fingerprint of a `Library`: module order plus each module's
/// full effective node arena (kind/name/parent/children per id) and symbol
/// lists — enough to catch any ordering or expansion difference.
fn fingerprint(lib: &Library) -> Vec<String> {
    let mut out = Vec::new();
    for m in lib.modules() {
        out.push(format!("module {} rev {:?}", m.name(), m.revision()));
        out.push(format!(
            "  includes {:?}",
            m.includes().iter().collect::<Vec<_>>()
        ));
        out.push(format!(
            "  top {:?}",
            m.top_nodes()
                .iter()
                .map(|&id| m.node(id).map(|n| n.name().to_string()))
                .collect::<Vec<_>>()
        ));
        for (id, n) in m.nodes().iter().enumerate() {
            out.push(format!(
                "  {id}|{:?}|{}|parent {:?}|children {:?}",
                n.kind(),
                n.name(),
                n.parent(),
                n.children()
            ));
        }
        out.push(format!(
            "  groupings {:?}",
            m.groupings().iter().map(|g| &g.name).collect::<Vec<_>>()
        ));
        out.push(format!(
            "  typedefs {:?}",
            m.typedefs().iter().map(|t| &t.name).collect::<Vec<_>>()
        ));
        out.push(format!(
            "  identities {:?}",
            m.identities().iter().map(|i| &i.name).collect::<Vec<_>>()
        ));
        out.push(format!(
            "  extensions {:?}",
            m.extensions().iter().map(|e| &e.name).collect::<Vec<_>>()
        ));
        out.push(format!(
            "  features {:?}",
            m.features().iter().map(|f| &f.name).collect::<Vec<_>>()
        ));
    }
    out
}

/// Compiling the whole sample workspace gives identical results whether the
/// documents are ingested one-by-one (`upsert`) or as one file batch
/// (`upsert_many_files`, which reads each file itself). In both feature modes
/// this is the same code path shape, but it pins the contract: a file batch
/// never reorders or drops anything, and commits exactly the readable files.
#[test]
fn files_batch_matches_sequential_semantics() {
    let samples = read_samples();
    assert!(!samples.is_empty(), "sample fixtures must exist");

    let mut seq = Repository::new();
    for (url, source) in &samples {
        seq.upsert(url.clone(), source.clone());
    }

    let mut file_batch = Repository::new();
    let files: Vec<(String, std::path::PathBuf)> = samples
        .iter()
        .map(|(url, _)| (url.clone(), std::path::PathBuf::from(url)))
        .collect();
    let committed = file_batch.upsert_many_files(files);
    assert_eq!(
        committed,
        samples.len(),
        "committed count must match readable files"
    );

    let o_seq = seq.compile();
    let o_batch = file_batch.compile();
    assert_eq!(diag_keys(&o_seq), diag_keys(&o_batch), "diagnostics differ");

    let (l_seq, l_batch) = (
        o_seq.library.as_ref().expect("sequential compiled library"),
        o_batch.library.as_ref().expect("batch compiled library"),
    );
    assert_eq!(fingerprint(l_seq), fingerprint(l_batch), "libraries differ");

    // Repeated compiles are deterministic.
    assert_eq!(diag_keys(&o_batch), diag_keys(&file_batch.compile()));

    // An empty batch is a no-op and commits nothing.
    let mut empty = Repository::new();
    assert_eq!(
        empty.upsert_many_files([] as [(String, std::path::PathBuf); 0]),
        0
    );
    assert!(empty.is_empty());
}

/// `upsert_many_files` reuses the same commit semantics as `upsert`: an
/// existing url is replaced in place (no duplicate document), new urls are
/// appended.
#[test]
fn files_batch_replaces_and_appends() {
    let dir = std::env::temp_dir().join(format!("yrepo-015-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let a_path = dir.join("a.yang");
    let b_path = dir.join("b.yang");
    let a_url = a_path.to_string_lossy().to_string();
    let b_url = b_path.to_string_lossy().to_string();

    let mut repo = Repository::new();
    repo.upsert(&a_url, "module a { namespace \"urn:a\"; prefix a; }");

    // Replace `a.yang` and add `b.yang` in one file batch.
    fs::write(
        &a_path,
        "module a { namespace \"urn:a\"; prefix a; container c {} }\n",
    )
    .unwrap();
    fs::write(&b_path, "module b { namespace \"urn:b\"; prefix b; }\n").unwrap();
    let committed = repo.upsert_many_files([(a_url.clone(), a_path), (b_url.clone(), b_path)]);
    assert_eq!(committed, 2);

    assert_eq!(repo.len(), 2, "replace must not duplicate a.yang");
    assert!(repo.contains(&a_url) && repo.contains(&b_url));

    let lib = repo.compile().library.expect("modules compiled");
    assert_eq!(lib.modules().len(), 2);
    let a = lib.module("a").expect("module a");
    assert_eq!(a.top_nodes().len(), 1, "a.yang was replaced, not kept");

    // An empty batch is a no-op.
    assert_eq!(
        repo.upsert_many_files([] as [(String, std::path::PathBuf); 0]),
        0
    );
    assert_eq!(repo.len(), 2);

    let _ = fs::remove_dir_all(&dir);
}
