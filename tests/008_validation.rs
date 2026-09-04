mod test_utils;

use test_utils::{has_code, library, load_files};
use yrepo::{DiagnosticCode, Repository, Severity};

#[test]
fn test_list_without_key_warning() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n list no-key {\n   leaf x { type string; }\n }\n}\n",
    );
    let out = repo.compile();
    assert!(out.library.is_some());
    assert!(
        has_code(&out.diagnostics, DiagnosticCode::ListWithoutKey),
        "expected ListWithoutKey, got {:?}",
        out.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::ListWithoutKey)
        .unwrap();
    assert_eq!(d.severity, Severity::Warning);
}

/// A key-less list under a `config false` ancestor is legal (A5): `config`
/// is inherited down the subtree.
#[test]
fn test_keyless_list_under_config_false_ok() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n\
         container state { config false;\n\
           list item { leaf x { type string; } }\n\
         }\n}\n",
    );
    let out = repo.compile();
    assert!(!has_code(&out.diagnostics, DiagnosticCode::ListWithoutKey));
}

/// Key-less lists inside an `rpc`/`action` `input`/`output` or a `notification`
/// are not configuration and need no `key`.
#[test]
fn test_keyless_list_in_rpc_input_ok() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n\
         rpc run { input { list arg { leaf v { type string; } } } }\n\
         notification changed { list edit { leaf p { type string; } } }\n}\n",
    );
    let out = repo.compile();
    assert!(!has_code(&out.diagnostics, DiagnosticCode::ListWithoutKey));
}

/// A key-less list defined inside a grouping with no explicit `config` is not
/// flagged — the grouping's config cannot be judged at the grouping level (A6);
/// the grouping author must only `uses` it into a `config false` tree.
#[test]
fn test_keyless_list_in_grouping_without_config_not_flagged() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n\
         grouping g { list item { leaf x { type string; } } }\n\
         container c { uses g; }\n}\n",
    );
    let out = repo.compile();
    assert!(!has_code(&out.diagnostics, DiagnosticCode::ListWithoutKey));
}

/// ... but if the grouping explicitly sets `config true` on the key-less list,
/// it is a real error and IS flagged.
#[test]
fn test_keyless_list_in_grouping_explicit_config_true_flagged() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n\
         grouping g { list bad { config true; leaf x { type string; } } }\n\
         container c { uses g; }\n}\n",
    );
    let out = repo.compile();
    assert!(has_code(&out.diagnostics, DiagnosticCode::ListWithoutKey));
}

#[test]
fn test_multi_module_compile() {
    let repo = load_files(&["simple.yang", "types.yang", "list-choice.yang"]);
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    assert_eq!(lib.modules().len(), 3);
    for name in ["simple", "types", "list-choice"] {
        assert!(lib.module(name).is_some(), "missing module {name}");
    }
}

#[test]
fn test_empty_repository_has_no_library() {
    let repo = Repository::new();
    let out = repo.compile();
    assert!(out.library.is_none());
    assert!(out.diagnostics.is_empty());
}
