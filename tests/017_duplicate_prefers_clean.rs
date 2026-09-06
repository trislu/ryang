//! Regression: among duplicate copies of the same (name, revision) module, a
//! copy that parsed cleanly must win over a broken one — otherwise the broken
//! copy shadows the healthy one and every importer reports unresolved symbols.

use yrepo::{DiagnosticCode, Repository, Severity};

const BROKEN: &str = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2024-01-01;
  leaf x { type string }
}"#;

const GOOD: &str = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2024-01-01;
  typedef t { type string; }
}"#;

const IMPORTER: &str = r#"module imp {
  namespace "urn:imp";
  prefix imp;
  import m { prefix m; }
  revision 2024-02-01;
  leaf l { type m:t; }
}"#;

#[test]
fn test_duplicate_prefers_parse_clean_copy() {
    let mut repo = Repository::new();
    // "a/broken" sorts before "c/good" so the broken copy is ingested first.
    repo.upsert("/a/broken.yang", BROKEN);
    repo.upsert("/c/good.yang", GOOD);
    repo.upsert("/z/importer.yang", IMPORTER);

    let out = repo.compile();

    // Exactly one duplicate warning, pointing at the *dropped* broken copy.
    let dups: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::DuplicateModule)
        .collect();
    assert_eq!(dups.len(), 1, "duplicates: {dups:?}");
    assert_eq!(dups[0].severity, Severity::Warning);
    assert_eq!(dups[0].url.as_deref(), Some("/a/broken.yang"));

    // The importer resolves `m:t` against the clean copy: no unresolved
    // typedef errors.
    let unresolved = out
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedTypedef)
        .count();
    assert_eq!(
        unresolved, 0,
        "importer should resolve against the clean copy"
    );

    // The library exposes module m (the clean copy's content).
    let lib = out.library.expect("library");
    assert!(lib.module("m").is_some(), "module m present");
}
