mod test_utils;

use test_utils::{find, library, load_file};
use yrepo::{NodeKind, Repository};

#[test]
fn test_compile_types() {
    let repo = load_file("types.yang");
    let (lib, diags) = library(&repo);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let m = lib.module("types").expect("module types");

    // builtin-typed leaves
    assert_eq!(find(m, &["str"]).unwrap().type_name(), Some("string"));
    assert_eq!(find(m, &["num32"]).unwrap().type_name(), Some("int32"));
    assert_eq!(find(m, &["unum64"]).unwrap().type_name(), Some("uint64"));
    assert_eq!(find(m, &["flag"]).unwrap().type_name(), Some("boolean"));

    // typedef + a leaf referencing it
    assert_eq!(find(m, &["custom"]).unwrap().type_name(), Some("my-int"));
    assert!(m.typedefs().iter().any(|t| t.name == "my-int"));
    assert!(lib.search_type("types", "my-int").is_some());
    assert!(lib.search_type("types", "nope").is_none());

    // leaf-list is a distinct node kind
    let tags = find(m, &["tags"]).unwrap();
    assert_eq!(tags.kind(), NodeKind::LeafList);
    assert_eq!(tags.type_name(), Some("string"));
}

/// A `decimal64` leaf's `default` may be a bare decimal (RFC 7950 §7.6.4,
/// §9.3.4) — including negative values like `-3.25` — and must parse cleanly
/// rather than collapsing the whole document.
#[test]
fn decimal64_negative_default_parses() {
    let mut repo = Repository::new();
    repo.upsert(
        "/m.yang",
        "module m {\n namespace \"urn:m\";\n prefix m;\n\
         leaf gain { type decimal64 { fraction-digits 2; } default -3.25; }\n\
         leaf ratio { type decimal64 { fraction-digits 4; } default 2.5; }\n\
         leaf int-default { type int8; default -10; }\n}\n",
    );
    let out = repo.compile();
    assert!(out.library.is_some(), "module failed to parse");
    assert!(
        out.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        out.diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
}
