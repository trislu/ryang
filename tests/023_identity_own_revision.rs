//! Identity derivation bases resolve against the DEFINING instance's own
//! identities — an unprefixed `base` names an identity of the same module
//! (RFC 7950 §7.18), so a non-canonical revision that defines and derives its
//! own identities must not be judged against the canonical revision's table.
//! Regression: a DRAFT routing-types copy deriving e164/ipv4/… from its own
//! `address-family` identity was reported "based on unknown identity" because
//! the check fell back to the canonical revision, which had restructured it.

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

fn identity_errors(diags: &[yrepo::Diagnostic]) -> Vec<&yrepo::Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedIdentity)
        .collect()
}

/// The OLDER (non-canonical) revision defines `address-family` and derives
/// identities from it; the NEWER (canonical) one does not. The older
/// instance's internal derivations must resolve.
#[test]
fn non_canonical_revision_resolves_own_identity_bases() {
    let (_, diags) = compile(&[
        (
            "/r/ietf-routing-types@2017-02-27.yang",
            "module rt { namespace urn:rt; prefix rt;\n\
             revision 2017-02-27;\n\
             identity address-family;\n\
             identity ipv4 { base address-family; }\n\
             identity ipv6 { base address-family; }\n\
             }",
        ),
        (
            "/r/ietf-routing-types@2017-12-04.yang",
            "module rt { namespace urn:rt; prefix rt;\n\
             revision 2017-12-04;\n\
             identity af-other;\n\
             }",
        ),
    ]);
    assert!(
        identity_errors(&diags).is_empty(),
        "own-revision identity bases must resolve: {diags:?}"
    );
}

/// Control: an identity base that only the CANONICAL revision defines is not
/// visible to the older instance (no over-silencing).
#[test]
fn older_revision_cannot_see_canonical_only_identity() {
    let (_, diags) = compile(&[
        (
            "/r/ietf-routing-types@2017-02-27.yang",
            "module rt { namespace urn:rt; prefix rt;\n\
             revision 2017-02-27;\n\
             identity ipv4 { base af-new; }\n\
             }",
        ),
        (
            "/r/ietf-routing-types@2017-12-04.yang",
            "module rt { namespace urn:rt; prefix rt;\n\
             revision 2017-12-04;\n\
             identity af-new;\n\
             }",
        ),
    ]);
    assert_eq!(
        identity_errors(&diags).len(),
        1,
        "expected one error: {diags:?}"
    );
}
