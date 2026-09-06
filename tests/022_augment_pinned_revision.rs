//! Augment/deviation target resolution honors the DECLARING instance's import
//! binding: a pinned import (revision-date) makes the augment target that
//! exact revision of the base module, not the canonical (highest) revision.
//!
//! Regression: an augment whose path exists only in the pinned older revision
//! (the canonical one restructured it away) was reported
//! `augment-target-not-found` because resolution used canonical-latest only.

use std::sync::Arc;

use yrepo::{DiagnosticCode, Library, ModuleRecord, Repository, SchemaNode};

const OLD: &str = "2020-01-01";
const NEW: &str = "2022-01-01";

fn base(rev: &str) -> &'static str {
    match rev {
        OLD => {
            "module base { namespace urn:base; prefix b;\n\
             revision 2020-01-01;\n\
             container legacy { leaf a { type string; } }\n\
             }"
        }
        _ => {
            "module base { namespace urn:base; prefix b;\n\
             revision 2022-01-01;\n\
             container modern { leaf m { type string; } }\n\
             }"
        }
    }
}

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

fn names<'m>(m: &'m ModuleRecord, node: &SchemaNode) -> Vec<&'m str> {
    node.children()
        .iter()
        .filter_map(|&id| m.node(id))
        .map(|n| n.name())
        .collect()
}

fn top<'m>(m: &'m ModuleRecord, name: &str) -> &'m SchemaNode {
    m.top_nodes()
        .iter()
        .find_map(|&id| m.node(id).filter(|n| n.name() == name))
        .expect("top-level node")
}

/// A pinned import must route the augment to the pinned (older) revision whose
/// tree the path exists in — not the canonical revision that lacks it.
#[test]
fn pinned_import_augments_target_pinned_revision() {
    let (lib, diags) = compile(&[
        ("/b/base@2020-01-01.yang", base(OLD)),
        ("/b/base@2022-01-01.yang", base(NEW)),
        (
            "/c/consumer.yang",
            "module consumer { namespace urn:c; prefix c;\n\
             import base { prefix b; revision-date 2020-01-01; }\n\
             augment \"/b:legacy\" { leaf added { type string; } }\n\
             }",
        ),
    ]);
    assert!(
        diags
            .iter()
            .all(|d| d.code != DiagnosticCode::AugmentTargetNotFound),
        "pinned augment must resolve: {diags:?}"
    );
    let old = lib.module_rev("base", OLD).expect("old revision compiled");
    assert_eq!(names(old, top(old, "legacy")), ["a", "added"]);
    // the canonical revision is untouched.
    let new = lib.module_rev("base", NEW).expect("new revision compiled");
    assert_eq!(names(new, top(new, "modern")), ["m"]);
}

/// Control: an UNPINNED augment resolves against the canonical revision (its
/// tree) and still fails for trees only an older revision has.
#[test]
fn unpinned_augment_stays_canonical() {
    let (lib, diags) = compile(&[
        ("/b/base@2020-01-01.yang", base(OLD)),
        ("/b/base@2022-01-01.yang", base(NEW)),
        (
            "/c/ok.yang",
            "module okc { namespace urn:okc; prefix okc;\n\
             import base { prefix b; }\n\
             augment \"/b:modern\" { leaf m2 { type string; } }\n\
             }",
        ),
        (
            "/c/bad.yang",
            "module badc { namespace urn:badc; prefix badc;\n\
             import base { prefix b; }\n\
             augment \"/b:legacy\" { leaf z { type string; } }\n\
             }",
        ),
    ]);
    let missing: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::AugmentTargetNotFound)
        .collect();
    assert_eq!(
        missing.len(),
        1,
        "only the canonical-missing path errors: {diags:?}"
    );
    let new = lib.module_rev("base", NEW).expect("new revision compiled");
    assert_eq!(names(new, top(new, "modern")), ["m", "m2"]);
}
