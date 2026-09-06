//! A module instance's internal `type` references must resolve against that
//! instance's OWN typedef table — not the canonical (highest-revision)
//! instance of the same module name. Revisions coexist in a repository; each
//! is validated on its own terms (RFC 7950: an unprefixed type names a
//! typedef in the same module).
//!
//! Regression: a non-canonical revision that defines and uses its own
//! `acl-type`-style typedef was flagged as "no such typedef is in scope"
//! because the check consulted the canonical instance's typedefs.

use std::sync::Arc;

use yrepo::{DiagnosticCode, Library, Repository};

fn compile(pairs: &[(&str, &str)]) -> (Arc<Library>, Vec<yrepo::Diagnostic>) {
    let mut repo = Repository::new();
    for (url, src) in pairs {
        repo.upsert(*url, *src);
    }
    let outcome = repo.compile();
    (
        outcome.library.expect("modules compiled"),
        outcome.diagnostics,
    )
}

fn typedef_errors(diags: &[yrepo::Diagnostic]) -> Vec<&yrepo::Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedTypedef)
        .collect()
}

/// Two revisions of `acl`: the OLDER one defines and uses its own typedef
/// `acl-type`; the NEWER (canonical) one does not define it. The older
/// instance's internal reference must resolve locally.
#[test]
fn non_canonical_revision_resolves_own_typedef() {
    let (lib, diags) = compile(&[
        (
            "/acls/acl@2014-10-10.yang",
            "module acl { namespace urn:acl; prefix acl;\n\
             revision 2014-10-10;\n\
             typedef acl-type { type string; }\n\
             container rules { leaf t { type acl-type; } }\n\
             }",
        ),
        (
            "/acls/acl@2015-03-04.yang",
            "module acl { namespace urn:acl; prefix acl;\n\
             revision 2015-03-04;\n\
             container rules { leaf t { type string; } }\n\
             }",
        ),
    ]);
    assert!(
        typedef_errors(&diags).is_empty(),
        "older revision's own typedef must resolve: {diags:?}"
    );
    // both instances are open in the library.
    assert!(lib.module("acl").is_some());
    assert_eq!(diags.len(), 0, "no other diagnostics expected: {diags:?}");
}

/// Control: a typedef that only the CANONICAL revision defines is NOT visible
/// to the older instance — the older instance's reference stays an error
/// (no over-silencing).
#[test]
fn older_revision_cannot_see_canonical_only_typedef() {
    let (lib, diags) = compile(&[
        (
            "/acls/acl@2014-10-10.yang",
            "module acl { namespace urn:acl; prefix acl;\n\
             revision 2014-10-10;\n\
             container rules { leaf t { type acl-type; } }\n\
             }",
        ),
        (
            "/acls/acl@2015-03-04.yang",
            "module acl { namespace urn:acl; prefix acl;\n\
             revision 2015-03-04;\n\
             typedef acl-type { type string; }\n\
             }",
        ),
    ]);
    assert_eq!(
        typedef_errors(&diags).len(),
        1,
        "expected one error: {diags:?}"
    );
    let m = lib.module("acl").expect("acl");
    let _ = m;
}
