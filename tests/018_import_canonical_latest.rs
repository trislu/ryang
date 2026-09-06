//! Regression: name-only references (unpinned imports, typedef/identity base
//! lookups) must resolve to the HIGHEST revision of a module, and each
//! revision must keep its own symbols — otherwise an old dated copy whose
//! filename sorts after an undated/newer copy shadows the newest module
//! (real-world: `ietf-yang-types.yang` (rev 2025-12-22) vs
//! `ietf-yang-types@2010-09-24.yang`, which made name-only importers lose
//! `hex-string`/`dotted-quad`).

use yrepo::{DiagnosticCode, Repository};

#[test]
fn test_name_only_resolution_uses_latest_revision() {
    let old = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2010-09-24;
  typedef legacy { type string; }
}"#;
    let new = r#"module m {
  namespace "urn:m";
  prefix m;
  revision 2025-12-22;
  typedef legacy { type string; }
  typedef hex-string { type string; }
}"#;
    let imp = r#"module imp {
  namespace "urn:imp";
  prefix imp;
  import m { prefix m; }
  revision 2026-01-01;
  leaf l { type m:hex-string; }
}"#;

    let mut repo = Repository::new();
    // The OLD revision's url sorts LAST, which used to make it win.
    repo.upsert("/a/m-new.yang", new);
    repo.upsert("/b/m-old@2010-09-24.yang", old);
    repo.upsert("/z/imp.yang", imp);

    let out = repo.compile();
    let unresolved = out
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedTypedef)
        .count();
    assert_eq!(unresolved, 0, "importer should see the newest revision");

    // The old revision keeps its own (smaller) symbol set.
    let lib = out.library.expect("library");
    let new_rec = lib.module_rev("m", "2025-12-22").expect("new rev");
    assert!(new_rec.typedefs().iter().any(|t| t.name == "hex-string"));
    let old_rec = lib.module_rev("m", "2010-09-24").expect("old rev");
    assert!(!old_rec.typedefs().iter().any(|t| t.name == "hex-string"));
}
