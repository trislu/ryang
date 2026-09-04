mod test_utils;

use test_utils::{has_code, library};
use yrepo::{DiagnosticCode, Repository, TypeCandidateKind};

const BASE: &str =
    "module ib {\n  namespace \"urn:ib\";\n  prefix ib;\n  typedef ip { type string; }\n}\n";
const CHILD: &str = "module child {\n  namespace \"urn:child\";\n  prefix c;\n  import ib { prefix b; }\n  typedef server { type b:ip; }\n  leaf srv { type server; }\n  leaf direct { type b:ip; }\n}\n";

/// A typedef chain bottoms out at a builtin — possibly through an imported
/// module's typedef.
#[test]
fn test_typedef_chain_resolution() {
    let mut repo = Repository::new();
    repo.upsert("/td.yang",
        "module td {\n  namespace \"urn:td\";\n  prefix td;\n  typedef port { type uint16; }\n  typedef svc-port { type port { range \"1..1024\"; } }\n  leaf p { type svc-port; }\n}\n",
    );
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    // direct builtin
    let r = lib.resolve_type("td", "uint16").unwrap();
    assert_eq!(r.builtin.as_deref(), Some("uint16"));
    assert!(r.typedefs.is_empty() && r.complete);

    // chain svc-port -> port -> uint16
    let r = lib.resolve_type("td", "svc-port").unwrap();
    assert_eq!(r.builtin.as_deref(), Some("uint16"));
    assert!(r.complete);
    let names: Vec<&str> = r.typedefs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["svc-port", "port"]);

    // unknown name -> None
    assert!(lib.resolve_type("td", "nope").is_none());
}

#[test]
fn test_cross_module_typedef_chain() {
    let mut repo = Repository::new();
    repo.upsert("/ib.yang", BASE);
    repo.upsert("/child.yang", CHILD);
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    // server -> b:ip (ib) -> string
    let r = lib.resolve_type("child", "server").unwrap();
    assert_eq!(r.builtin.as_deref(), Some("string"));
    assert!(r.complete);
    let steps: Vec<(&str, &str)> = r
        .typedefs
        .iter()
        .map(|s| (s.module.as_ref(), s.name.as_str()))
        .collect();
    assert_eq!(steps, [("child", "server"), ("ib", "ip")]);

    // completion candidates include builtins, local typedefs, and `b:ip`
    let cands = lib.type_candidates("child");
    assert!(
        cands
            .iter()
            .any(|c| c.name == "string" && c.kind == TypeCandidateKind::Builtin)
    );
    assert!(
        cands
            .iter()
            .any(|c| c.name == "server" && c.kind == TypeCandidateKind::Typedef)
    );
    assert!(
        cands
            .iter()
            .any(|c| c.name == "b:ip" && c.module.as_deref() == Some("ib"))
    );
}

#[test]
fn test_unresolved_typedef_is_a_diagnostic() {
    let mut repo = Repository::new();
    repo.upsert(
        "/bad.yang",
        "module bad {\n  namespace \"urn:bad\";\n  prefix bad;\n  typedef x { type gone; }\n  leaf q { type nope; }\n}\n",
    );
    let out = repo.compile();
    assert!(out.library.is_some());
    let count = out
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedTypedef)
        .count();
    assert_eq!(
        count, 2,
        "expected 2 UnresolvedTypedef diagnostics, got {count}"
    );
}

/// Referencing a typedef from an unloaded module is an import problem, not a
/// typedef problem — don't double-report.
#[test]
fn test_missing_module_is_not_an_unresolved_typedef() {
    let mut repo = Repository::new();
    repo.upsert("/child.yang", CHILD);
    let out = repo.compile();
    assert!(has_code(&out.diagnostics, DiagnosticCode::UnresolvedImport));
    assert!(!has_code(
        &out.diagnostics,
        DiagnosticCode::UnresolvedTypedef
    ));
}
