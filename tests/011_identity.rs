mod test_utils;

use test_utils::{has_code, library};
use yrepo::{DiagnosticCode, Repository};

const BASE: &str = "module ib {\n  namespace \"urn:ib\";\n  prefix ib;\n  identity iface;\n}\n";
const CHILD: &str = "module child {\n  namespace \"urn:child\";\n  prefix c;\n  import ib { prefix b; }\n  identity eth { base b:iface; }\n  identity eth100 { base eth; }\n}\n";

/// Identity derivation is a graph: identities reference a `base` in the same
/// module or (prefix-qualified) in an imported one.
#[test]
fn test_local_identity_derivation() {
    let mut repo = Repository::new();
    repo.upsert(
        "/idt.yang",
        "module idt {\n  namespace \"urn:idt\";\n  prefix idt;\n  identity base-if;\n  identity ethernet { base base-if; }\n  identity fast { base ethernet; }\n}\n",
    );
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    // ancestry of `fast` is `fast -> ethernet -> base-if`
    let res = lib.resolve_identity("idt", "fast").expect("resolve fast");
    assert_eq!(res.root.name, "fast");
    let chain: Vec<(&str, &str)> = res
        .bases
        .iter()
        .map(|b| (b.module.as_ref(), b.name.as_str()))
        .collect();
    assert_eq!(chain, [("idt", "ethernet"), ("idt", "base-if")]);

    // derived set includes the base and everything under it
    let derived: Vec<String> = lib
        .derived_identities("idt", "base-if")
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert_eq!(derived, ["base-if", "ethernet", "fast"]);
}

#[test]
fn test_cross_module_identity_derivation() {
    let mut repo = Repository::new();
    repo.upsert("/ib.yang", BASE);
    repo.upsert("/child.yang", CHILD);
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    // `eth` is based on `ib:iface`
    let res = lib.resolve_identity("child", "eth").expect("resolve eth");
    assert_eq!(res.root.name, "eth");
    assert_eq!(res.bases.len(), 1);
    assert_eq!(res.bases[0].module.as_ref(), "ib");
    assert_eq!(res.bases[0].name, "iface");

    // cross-module derived set
    let derived: Vec<String> = lib
        .derived_identities("ib", "iface")
        .iter()
        .map(|d| d.qualified())
        .collect();
    assert!(derived.iter().any(|d| d == "child:eth"));
    assert!(derived.iter().any(|d| d == "child:eth100"));
    assert!(derived.iter().any(|d| d == "ib:iface"));

    // completion: own identities + imported ones as `prefix:name`
    let cands = lib.identity_candidates("child");
    assert!(cands.iter().any(|c| c == "eth"));
    assert!(cands.iter().any(|c| c == "b:iface"));
}

#[test]
fn test_unresolved_identity_base_is_a_diagnostic() {
    let mut repo = Repository::new();
    repo.upsert(
        "/bad.yang",
        "module bad {\n  namespace \"urn:bad\";\n  prefix bad;\n  identity broken { base missing-ident; }\n}\n",
    );
    let out = repo.compile();
    assert!(out.library.is_some());
    assert!(
        has_code(&out.diagnostics, DiagnosticCode::UnresolvedIdentity),
        "expected UnresolvedIdentity, got {:?}",
        out.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}
