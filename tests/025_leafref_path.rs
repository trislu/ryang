//! A `leafref` `path` must name an existing schema node (RFC 7950 §9.9).
//! Absolute paths (predicates stripped segment-wise) are validated here;
//! relative paths are deferred to the leafref-path engine.

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

fn leafref_errors(diags: &[yrepo::Diagnostic]) -> Vec<&yrepo::Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedLeafref)
        .collect()
}

#[test]
fn valid_absolute_paths_resolve() {
    let (_, diags) = compile(&[(
        "/ok.yang",
        "module okm { namespace urn:ok; prefix o;\n\
         container top { list item { key name; leaf name { type string; }\n\
         leaf port { type uint16; } } }\n\
         leaf r { type leafref { path \"/o:top/o:item/o:port\"; } }\n\
         leaf rp { type leafref { path \"/o:top/o:item[o:name='x']/o:port\"; } }\n\
         }",
    )]);
    assert!(leafref_errors(&diags).is_empty(), "{diags:?}");
}

#[test]
fn dangling_absolute_path_is_flagged() {
    let (_, diags) = compile(&[(
        "/bad.yang",
        "module badm { namespace urn:bad; prefix b;\n\
         container top { leaf a { type string; } }\n\
         leaf r { type leafref { path \"/b:top/b:missing\"; } }\n\
         }",
    )]);
    assert_eq!(leafref_errors(&diags).len(), 1, "{diags:?}");
}

#[test]
fn unknown_prefix_in_path_is_flagged() {
    let (_, diags) = compile(&[(
        "/pfx.yang",
        "module pfxm { namespace urn:pfx; prefix p;\n\
         leaf r { type leafref { path \"/q:top\"; } }\n\
         }",
    )]);
    assert_eq!(leafref_errors(&diags).len(), 1, "{diags:?}");
}

#[test]
fn relative_paths_are_not_reported_here() {
    let (_, diags) = compile(&[(
        "/rel.yang",
        "module relm { namespace urn:rel; prefix r;\n\
         container top { leaf a { type string; }\n\
         leaf r { type leafref { path \"../a\"; } } }\n\
         }",
    )]);
    assert!(leafref_errors(&diags).is_empty(), "{diags:?}");
}

/// A leafref written inside a grouping of module A (prefixes bound in A) must
/// resolve even when the grouping is instantiated in module B (whose prefix
/// map may not know A's prefixes) — the path is resolved against the ORIGIN
/// module's symbols.
#[test]
fn grouping_born_leafref_uses_origin_prefixes() {
    let (_, diags) = compile(&[
        (
            "/ga.yang",
            "module ga { namespace urn:ga; prefix ga;\n\
             container top { list item { key name; leaf name { type string; }\n\
             leaf port { type uint16; } } }\n\
             grouping ref { leaf r { type leafref { path \"/ga:top/ga:item/ga:port\"; } } }\n\
             }",
        ),
        (
            "/gb.yang",
            "module gb { namespace urn:gb; prefix gb;\n\
             import ga { prefix ga; }\n\
             container c { uses ga:ref; }\n\
             }",
        ),
    ]);
    assert!(
        leafref_errors(&diags).is_empty(),
        "origin-module resolution must not misreport: {diags:?}"
    );
}
