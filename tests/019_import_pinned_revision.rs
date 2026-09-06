//! Regression: an import that pins `revision-date` must resolve against
//! EXACTLY that revision, even when a newer revision of the module exists
//! (canonical-latest) — otherwise symbols present only in the pinned revision
//! become unresolved.

use yrepo::{DiagnosticCode, Repository};

#[test]
fn test_pinned_import_resolves_exact_revision() {
    let old = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2010-09-24;
  typedef legacy-only { type string; }
}"#;
    let new = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2025-12-22;
  typedef modern-only { type string; }
}"#;
    let imp = r#"module imp {
  namespace "urn:imp";
  prefix imp;
  import m {
    prefix m;
    revision-date 2010-09-24;
  }
  revision 2011-01-01;
  leaf l { type m:legacy-only; }
}"#;

    let mut repo = Repository::new();
    repo.upsert("/a/m-new.yang", new);
    repo.upsert("/b/m-old@2010-09-24.yang", old);
    repo.upsert("/z/imp.yang", imp);

    let out = repo.compile();
    let unresolved = out
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedTypedef)
        .count();
    assert_eq!(
        unresolved, 0,
        "pinned import should see the 2010 revision with legacy-only"
    );
}
