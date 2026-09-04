mod test_utils;

use test_utils::{find, has_code, library, load_files};
use yrepo::{DiagnosticCode, NodeKind, Repository};

/// RFC 7950 §5.1 forbids circular chains of imports — they are reported as a
/// diagnostic, never silently accepted.
#[test]
fn test_import_cycle_reported() {
    let repo = load_files(&["cycle-a.yang", "cycle-b.yang"]);
    let out = repo.compile();
    assert!(
        has_code(&out.diagnostics, DiagnosticCode::ImportCycle),
        "expected ImportCycle, got {:?}",
        out.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
    // both modules are still parsed best-effort (diagnostics, not errors)
    let lib = out.library.expect("best-effort library");
    assert_eq!(lib.modules().len(), 2);
    assert!(lib.module("cycle-a").is_some());
    assert!(lib.module("cycle-b").is_some());
}

/// Submodules fold into their parent module; the effective tree carries the
/// defining `Location` of the physical (submodule) file ([D6]).
#[test]
fn test_submodule_folds_into_parent_module() {
    let repo = load_files(&["submod-parent.yang", "submod-child.yang"]);
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    assert_eq!(lib.modules().len(), 1, "only the parent is a module");
    let m = lib.module("submod-parent").expect("module submod-parent");

    // the submodule name is recorded as an include
    assert_eq!(m.includes(), ["submod-child"]);

    // `child-leaf` is folded into the parent's effective tree ...
    let leaf = find(m, &["child-leaf"]).expect("leaf from submodule");
    assert_eq!(leaf.kind(), NodeKind::Leaf);
    // ... but its defining Location points at the submodule file
    let url = leaf.defining().url.to_string();
    assert!(url.ends_with("submod-child.yang"), "defining url was {url}");

    // `search_submodule` resolves the submodule document for goto on `include`
    let sub = lib.submodule("submod-child").expect("submodule record");
    assert_eq!(sub.parent_module(), Some("submod-parent"));
}

/// Include cycles (submodule includes a sibling that includes it back) are
/// illegal and reported as a diagnostic.
#[test]
fn test_include_cycle_detected() {
    let mut repo = Repository::new();
    repo.upsert(
        "/pa.yang",
        "module pa { namespace \"urn:pa\"; prefix pa; include s1; }\n",
    );
    repo.upsert(
        "/s1.yang",
        "submodule s1 {\n belongs-to pa { prefix pa; }\n include s2;\n leaf a { type string; }\n}\n",
    );
    repo.upsert(
        "/s2.yang",
        "submodule s2 {\n belongs-to pa { prefix pa; }\n include s1;\n leaf b { type string; }\n}\n",
    );
    let out = repo.compile();
    assert!(has_code(&out.diagnostics, DiagnosticCode::IncludeCycle));
}

/// Diamond includes are NOT cycles (RFC 7950 §5.2): a module includes two
/// sibling submodules that both include a shared `*-base` submodule. The shared
/// submodule is folded once; no `IncludeCycle` is reported (regression: the old
/// global "seen" check reported every second visit as a cycle — the BBF corpus
/// case).
#[test]
fn test_diamond_include_is_not_a_cycle() {
    let mut repo = Repository::new();
    repo.upsert(
        "/pd.yang",
        "module pd { namespace \"urn:pd\"; prefix pd; include a; include b; }\n",
    );
    repo.upsert(
        "/a.yang",
        "submodule a {\n belongs-to pd { prefix pd; }\n include base;\n leaf la { type string; }\n}\n",
    );
    repo.upsert(
        "/b.yang",
        "submodule b {\n belongs-to pd { prefix pd; }\n include base;\n leaf lb { type string; }\n}\n",
    );
    repo.upsert(
        "/base.yang",
        "submodule base {\n belongs-to pd { prefix pd; }\n leaf shared { type string; }\n}\n",
    );
    let out = repo.compile();
    assert!(
        !has_code(&out.diagnostics, DiagnosticCode::IncludeCycle),
        "diamond include misreported as a cycle: {:?}",
        out.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    // the shared submodule is folded exactly once into the parent module
    let lib = out.library.expect("library");
    let m = lib.module("pd").expect("module pd");
    let mut includes: Vec<&str> = m.includes().iter().map(|s| s.as_ref()).collect();
    includes.sort_unstable();
    assert_eq!(includes, ["a", "b", "base"]);
    assert!(
        find(m, &["shared"]).is_some(),
        "shared leaf should be folded"
    );
}
