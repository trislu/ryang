mod test_utils;

use test_utils::{child_names, find, library, load_file};
use yrepo::{NodeKind, Repository, StatementKind, TokenSpot};

#[test]
fn test_compile_simple() {
    let repo = load_file("simple.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    assert_eq!(lib.modules().len(), 1);

    let m = lib.module("simple").expect("module simple");
    assert_eq!(m.namespace(), Some("urn:simple"));
    assert_eq!(m.prefix(), Some("s"));

    let config = find(m, &["config"]).expect("container config");
    assert_eq!(config.kind(), NodeKind::Container);
    assert_eq!(child_names(m, config), ["hostname", "port", "enabled"]);

    let port = find(m, &["config", "port"]).expect("leaf port");
    assert_eq!(port.kind(), NodeKind::Leaf);
    assert_eq!(port.type_name(), Some("uint16"));
    assert_eq!(port.default(), Some("8080"));

    let description = find(m, &["description"]).expect("top-level leaf description");
    assert_eq!(description.kind(), NodeKind::Leaf);
}

#[test]
fn test_token_at_distinguishes_keyword_and_argument() {
    let mut repo = Repository::new();
    let src = "module m {\n  namespace \"urn:m\";\n  prefix m;\n  container c {\n    leaf x { type string; }\n  }\n}\n";
    repo.upsert("/m.yang", src);

    // caret on the `container` keyword (row 3, col 2 = 'c')
    let hit = repo.token_at("/m.yang", 3, 2).unwrap();
    assert_eq!(hit.spot, TokenSpot::Keyword);
    assert_eq!(hit.statement, StatementKind::Container);

    // caret on the argument `c` (row 3, col 12)
    let hit = repo.token_at("/m.yang", 3, 12).unwrap();
    assert_eq!(hit.spot, TokenSpot::Argument);
}
