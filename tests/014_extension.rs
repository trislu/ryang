mod test_utils;

use test_utils::library;
use yrepo::Repository;

const EXT_BASE: &str = "module ext-base {\n  namespace \"urn:ext\";\n  prefix ext;\n\
  extension info { argument name; }\n\
  extension validate;\n\
}\n";
const USAGE: &str = "module usage {\n  namespace \"urn:usage\";\n  prefix u;\n\
  import ext-base { prefix ext; }\n\
  leaf x { type string; ext:info \"x\"; }\n\
}\n";

/// `extension` definitions are symbols: a module exposes them and an
/// extension *usage* (`prefix:name`) resolves through the import to them —
/// the data goto/hover relies on.
#[test]
fn test_extension_symbol_resolution() {
    let mut repo = Repository::new();
    repo.upsert("/t.yang", EXT_BASE.to_string());
    repo.upsert("/u.yang", USAGE.to_string());
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    // The defining module indexes its extensions (+ the declared argument).
    let m = lib.module("ext-base").expect("module ext-base");
    let info = m
        .extensions()
        .iter()
        .find(|e| e.name == "info")
        .expect("extension info indexed");
    assert_eq!(info.argument.as_deref(), Some("name"));
    assert!(m.extensions().iter().any(|e| e.name == "validate"));
    assert_eq!(m.extensions().iter().find(|e| e.name == "validate").unwrap().argument, None);

    // Resolve from a using module: prefix -> module -> extension definition.
    assert_eq!(lib.prefix_to_module("usage", "ext"), Some("ext-base"));
    let found = lib
        .search_extension("ext-base", "info")
        .expect("resolve ext:info");
    assert_eq!(found.name, "info");
    assert_eq!(found.defining.url.as_ref(), "/t.yang");

    // Unresolved extension name yields nothing.
    assert!(lib.search_extension("ext-base", "no-such-ext").is_none());
}
