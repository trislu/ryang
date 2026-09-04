mod test_utils;

use test_utils::{child_names, find, has_code, library, load_file, load_files};
use yrepo::DiagnosticCode;

/// Cross-module `uses prefix:grouping` must resolve through imports and
/// instantiate the grouping's nodes in the importing module's tree.
#[test]
fn test_imported_grouping_expands() {
    let repo = load_files(&["import-base.yang", "import-ext.yang"]);
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    assert_eq!(lib.modules().len(), 2);
    let ext = lib.module("import-ext").expect("module import-ext");

    // header import recorded
    assert!(
        ext.imports()
            .iter()
            .any(|i| i.module == "import-base" && i.prefix == "ib")
    );
    assert_eq!(
        lib.prefix_to_module("import-ext", "ib"),
        Some("import-base")
    );

    // grouping `ib:definitions` is instantiated under `extended`
    let extended = find(ext, &["extended"]).expect("container extended");
    assert_eq!(child_names(ext, extended), ["extra", "item"]);

    let item = find(ext, &["extended", "item"]).expect("leaf item from grouping");
    // nodes defined in an imported grouping keep that module's origin ([D9])
    assert_eq!(item.origin_module(), "import-base");
    // ... and the `uses` site is recorded
    assert!(item.used_from().is_some());
}

/// A missing import is a *diagnostic*, never an error ([D3]) — the module
/// still compiles best-effort.
#[test]
fn test_missing_import_is_a_diagnostic() {
    let repo = load_file("import-ext.yang");
    let out = repo.compile();
    let lib = out.library.expect("library still produced");
    assert!(has_code(&out.diagnostics, DiagnosticCode::UnresolvedImport));
    assert!(has_code(
        &out.diagnostics,
        DiagnosticCode::UnresolvedGrouping
    ));

    let ext = lib.module("import-ext").unwrap();
    let extended = find(ext, &["extended"]).expect("container extended still present");
    // the module's own leaf is there; the grouping node is not instantiated
    let names = child_names(ext, extended);
    assert!(names.contains(&"extra"));
    assert!(!names.contains(&"item"));
}
