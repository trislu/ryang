//! Duplicate sibling data-node names are an error (RFC 7950 §7): two nodes
//! with the same name under one parent make hover/goto/completion ambiguous.
//! Names under different `case` branches of a `choice` live under different
//! parents and are allowed.

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

fn dup_errors(diags: &[yrepo::Diagnostic]) -> Vec<&yrepo::Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::DuplicateNode)
        .collect()
}

#[test]
fn duplicate_leaves_under_same_container_error() {
    let (_, diags) = compile(&[(
        "/dup.yang",
        "module dupm { namespace urn:d; prefix d;\n\
         container c { leaf x { type string; } leaf x { type uint8; } }\n\
         }",
    )]);
    assert_eq!(dup_errors(&diags).len(), 1, "one duplicate: {diags:?}");
}

#[test]
fn same_name_under_distinct_parents_is_fine() {
    let (_, diags) = compile(&[(
        "/ok.yang",
        "module okm { namespace urn:o; prefix o;\n\
         container a { leaf x { type string; } }\n\
         container b { leaf x { type string; } }\n\
         }",
    )]);
    assert!(dup_errors(&diags).is_empty(), "{diags:?}");
    assert!(diags.is_empty(), "{diags:?}");
}

/// Two `case`s of one choice may each define a leaf named `x` (only one case
/// is active in any instance): different parents, allowed.
#[test]
fn duplicate_name_across_choice_cases_is_fine() {
    let (_, diags) = compile(&[(
        "/case.yang",
        "module casem { namespace urn:cm; prefix cm;\n\
         container c { choice pick { case one { leaf x { type string; } }\n\
         case two { leaf x { type string; } } } }\n\
         }",
    )]);
    assert!(dup_errors(&diags).is_empty(), "{diags:?}");
}

/// The same grouping used twice under one container duplicates its nodes:
/// flagged (RFC-invalid and ambiguous for navigation).
#[test]
fn repeated_uses_in_one_parent_flagged() {
    let (_, diags) = compile(&[(
        "/ru.yang",
        "module rum { namespace urn:ru; prefix ru;\n\
         grouping g { leaf k { type string; } }\n\
         container c { uses g; uses g; }\n\
         }",
    )]);
    assert_eq!(dup_errors(&diags).len(), 1, "one duplicate: {diags:?}");
}
