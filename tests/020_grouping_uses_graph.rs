//! Behavior pins for grouping `uses` expansion (PHASE ② pre-swap guard).
//!
//! The planned refactor (`YREPO_PHASES.md` ②/step 3) drives grouping expansion
//! from a dependency DAG instead of recursion-only inline expansion. These
//! tests pin the current end-to-end semantics so that swap must keep them
//! green: cross-module recursive `uses` chains expand to the effective tree,
//! each `uses` site instantiates its own copy (diamond), expansion is
//! independent of definition order, and recursive groupings yield exactly one
//! `UnresolvedGrouping` diagnostic at the re-entry site.
//!
//! Modules are inline strings (no fixtures) so the graph shapes are readable
//! next to their assertions.

use std::sync::Arc;

use yrepo::{Diagnostic, DiagnosticCode, Library, ModuleRecord, Repository, SchemaNode};

fn compile(pairs: &[(&str, &str)]) -> (Arc<Library>, Vec<Diagnostic>) {
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

/// A grouping in module `base` used by a grouping in module `mid`, used by a
/// container in module `top`: expansion must recurse across module boundaries
/// through canonical resolution, producing the leaf of `base` inside `top`'s
/// container regardless of definition order.
#[test]
fn chain_across_modules_expands_recursively() {
    let (lib, diags) = compile(&[
        (
            "/mods/base.yang",
            "module base { namespace urn:base; prefix b;\n\
             container later { uses bg; }\n\
             grouping bg { leaf bx { type string; } }\n\
             }",
        ),
        (
            "/mods/mid.yang",
            "module mid { namespace urn:mid; prefix m;\n\
             import base { prefix b; }\n\
             grouping mg { leaf mx { type string; } uses b:bg; }\n\
             }",
        ),
        (
            "/mods/top.yang",
            "module top { namespace urn:top; prefix t;\n\
             import mid { prefix m; }\n\
             container c { uses m:mg; }\n\
             }",
        ),
    ]);
    assert!(diags.is_empty(), "expected zero diagnostics, got {diags:?}");

    // base itself expands its own grouping (used by `later`).
    let base = lib.module("base").expect("base");
    assert_eq!(names(base, top(base, "later")), vec!["bx"]);

    let top_mod = lib.module("top").expect("top");
    let c = top(top_mod, "c");
    // grouping mg body order: leaf mx, then uses base:bg -> leaf bx
    assert_eq!(names(top_mod, c), vec!["mx", "bx"]);
}

/// Using the same grouping from two sibling groupings must instantiate two
/// independent copies (no cross-talk, no memoized sharing of nodes).
#[test]
fn diamond_uses_instantiate_per_site() {
    let (lib, diags) = compile(&[(
        "/mods/d.yang",
        "module d { namespace urn:d; prefix d;\n\
         container first { uses a; }\n\
         grouping addr { leaf street { type string; } }\n\
         grouping a { uses addr; leaf a1 { type string; } }\n\
         grouping b { uses addr; leaf b1 { type string; } }\n\
         container second { uses b; }\n\
         }",
    )]);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let m = lib.module("d").expect("d");
    // containers defined before the groupings they use: order-independent.
    let first = top(m, "first");
    let second = top(m, "second");
    assert_eq!(names(m, first), vec!["street", "a1"]);
    assert_eq!(names(m, second), vec!["street", "b1"]);
    // two independent street instances (distinct node ids), not shared: the
    // first child of each container is that container's own street copy.
    assert_ne!(first.children()[0], second.children()[0]);
}

fn recursive_errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedGrouping && d.message.contains("recursive"))
        .collect()
}

/// A grouping that uses itself yields exactly one recursive-grouping error at
/// the re-entry `uses` site, and expansion still yields the acyclic prefix.
#[test]
fn self_recursive_grouping_single_error() {
    let (lib, diags) = compile(&[(
        "/mods/sr.yang",
        "module sr { namespace urn:sr; prefix sr;\n\
         grouping g { leaf a { type string; } uses g; }\n\
         container c { uses g; }\n\
         }",
    )]);
    let found = recursive_errors(&diags);
    assert_eq!(found.len(), 1, "expected one recursive error: {diags:?}");
    let m = lib.module("sr").expect("sr");
    assert_eq!(names(m, top(m, "c")), vec!["a"]);
}

/// Mutual recursion between two groupings in one module is also one error.
#[test]
fn mutual_recursive_groupings_single_error() {
    let (lib, diags) = compile(&[(
        "/mods/mr.yang",
        "module mr { namespace urn:mr; prefix mr;\n\
         grouping ga { uses gb; }\n\
         grouping gb { uses ga; }\n\
         container c { uses ga; }\n\
         }",
    )]);
    let found = recursive_errors(&diags);
    assert_eq!(found.len(), 1, "expected one recursive error: {diags:?}");
    let m = lib.module("mr").expect("mr");
    assert!(names(m, top(m, "c")).is_empty(), "unexpected children");
}

/// A five-deep uses chain must expand fully (recursion-depth + ordering pin:
/// a future template/memoized expansion must still materialize every level and
/// keep dependency-first child order l1..l5).
#[test]
fn deep_uses_chain_expands_in_order() {
    let (lib, diags) = compile(&[(
        "/mods/dc.yang",
        "module dc { namespace urn:dc; prefix dc;\n\
         grouping g1 { leaf l1 { type string; } }\n\
         grouping g2 { uses g1; leaf l2 { type string; } }\n\
         grouping g3 { uses g2; leaf l3 { type string; } }\n\
         grouping g4 { uses g3; leaf l4 { type string; } }\n\
         grouping g5 { uses g4; leaf l5 { type string; } }\n\
         container c { uses g5; }\n\
         }",
    )]);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let m = lib.module("dc").expect("dc");
    assert_eq!(names(m, top(m, "c")), vec!["l1", "l2", "l3", "l4", "l5"]);
}
