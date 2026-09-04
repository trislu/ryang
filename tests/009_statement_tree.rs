//! Statement-tree exposure — the syntax view that backs fold / format /
//! highlight / precise goto-hover.
//!
//! Covers: `Repository::statement` (whole document tree), `preorder`
//! enumeration, argument-string reading (logical text + raw spans), keyword /
//! statement / terminator spans (`StatementEnd`: `;` leaf vs `{ … }` block),
//! and `Repository::statement_at` caret lookup into a real statement.

use yrepo::{Repository, Statement, StatementEnd, StatementKind};

/// (0-based row, col) of the first occurrence of `needle` in `src`, so tests
/// can place the caret at a known token. Fixtures are ASCII, so byte and char
/// columns coincide.
fn pos(src: &str, needle: &str) -> (usize, usize) {
    for (row, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return (row, col);
        }
    }
    panic!("needle {needle:?} not found in source");
}

fn arg_of(stmt: &Statement) -> &str {
    stmt.arg.as_ref().expect("statement has an argument").name()
}

#[test]
fn statement_returns_the_document_tree() {
    let src = "module m {\n  namespace \"urn:m\";\n  prefix m;\n  container c {\n    leaf x { type string; }\n  }\n  leaf description {\n    type string;\n  }\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/m.yang", src);

    let root = repo.statement("/m.yang").expect("root statement");
    assert_eq!(root.kind, StatementKind::Module);
    assert_eq!(arg_of(root), "m");
    assert!(root.is_block());

    // Top-level statements in source order.
    let tops: Vec<StatementKind> = root.children.iter().map(|c| c.kind.clone()).collect();
    assert_eq!(
        tops,
        vec![
            StatementKind::Namespace,
            StatementKind::Prefix,
            StatementKind::Container,
            StatementKind::Leaf,
        ]
    );

    // Enumerate the whole tree, pre-order: parent before children, source order.
    let kinds: Vec<StatementKind> = root.preorder().map(|s| s.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            StatementKind::Module,
            StatementKind::Namespace,
            StatementKind::Prefix,
            StatementKind::Container,
            StatementKind::Leaf, // leaf x
            StatementKind::Type,
            StatementKind::Leaf, // leaf description
            StatementKind::Type,
        ]
    );

    // Unknown url -> None.
    assert!(repo.statement("/nope.yang").is_none());
}

#[test]
fn statement_exposes_argument_strings_and_spans() {
    let src = "module m {\n  namespace \"urn:m\";\n  prefix m;\n  container c {\n    leaf x { type string; }\n  }\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/m.yang", src);
    let root = repo.statement("/m.yang").unwrap();

    // Namespace: quoted argument -> logical text dequoted, range keeps quotes.
    let ns = root.find_one(StatementKind::Namespace).unwrap();
    let ns_arg = ns.arg.as_ref().unwrap();
    assert_eq!(ns_arg.logical, "urn:m");
    assert_eq!(&src[ns_arg.range.clone()], "\"urn:m\"");

    // Container name.
    let container = root.find_one(StatementKind::Container).unwrap();
    assert_eq!(arg_of(container), "c");

    // Exact keyword and whole-statement text via spans.
    let kw = container.keyword.as_ref().unwrap();
    assert_eq!(&src[kw.clone()], "container");
    assert_eq!(
        &src[container.span()],
        "container c {\n    leaf x { type string; }\n  }"
    );

    // The leaf `x` and its `type string;` child.
    let x = container.children.first().unwrap();
    assert_eq!(x.kind, StatementKind::Leaf);
    assert_eq!(arg_of(x), "x");
    let ty = x.children.first().unwrap();
    assert_eq!(ty.kind, StatementKind::Type);
    assert_eq!(arg_of(ty), "string");
}

#[test]
fn statement_end_distinguishes_semicolon_leaf_from_block() {
    // `feature fx;` is a leaf even though `feature` may carry a block;
    // `container empty {}` is an (empty) block even though it has no children.
    let src = "module t {\n  namespace \"urn:t\";\n  prefix t;\n  feature fx;\n  feature fg {\n    status current;\n  }\n  container empty {\n  }\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/t.yang", src);
    let root = repo.statement("/t.yang").unwrap();

    // Leaf statement: `;` terminator, `body()` is None, span ends at the `;`.
    let fx = root.find_one(StatementKind::Feature).unwrap();
    assert!(!fx.is_block());
    assert!(fx.body().is_none());
    match &fx.end {
        Some(StatementEnd::Semicolon { semi }) => assert_eq!(&src[semi.clone()], ";"),
        other => panic!("expected a `;`-terminated leaf, got {other:?}"),
    }
    assert_eq!(&src[fx.span()], "feature fx;");

    // Block statement with children.
    let fg = root
        .children
        .iter()
        .find(|c| c.kind == StatementKind::Feature && arg_of(c) == "fg")
        .unwrap();
    assert!(fg.is_block());
    let body = fg.body().unwrap();
    assert_eq!(&src[body], "\n    status current;\n  ");

    // Empty block `{ … }` with no children is still a block (fold needs this).
    let empty = root
        .children
        .iter()
        .find(|c| c.kind == StatementKind::Container)
        .unwrap();
    assert!(empty.children.is_empty());
    assert!(empty.is_block());
    assert_eq!(&src[empty.body().unwrap()], "\n  ");
    assert_eq!(&src[empty.span()], "container empty {\n  }");
}

#[test]
fn statement_at_returns_the_narrowest_statement() {
    let src = "module m {\n  namespace \"urn:m\";\n  prefix m;\n  container c {\n    leaf x { type string; }\n  }\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/m.yang", src);

    // Caret on the `container` keyword -> the container statement.
    let (r, c) = pos(src, "container");
    let hit = repo.statement_at("/m.yang", r, c).unwrap();
    assert_eq!(hit.kind, StatementKind::Container);
    assert_eq!(arg_of(hit), "c");

    // Caret inside the quoted namespace argument.
    let (r, c) = pos(src, "urn:m");
    let hit = repo.statement_at("/m.yang", r, c).unwrap();
    assert_eq!(hit.kind, StatementKind::Namespace);
    assert_eq!(hit.arg.as_ref().unwrap().logical, "urn:m");

    // Caret inside a nested `type string;` -> the type statement (not `leaf x`).
    let (r, c) = pos(src, "type string");
    let hit = repo.statement_at("/m.yang", r, c).unwrap();
    assert_eq!(hit.kind, StatementKind::Type);
    assert_eq!(arg_of(hit), "string");

    // Caret at a position after the last token -> no statement.
    let last = src.lines().count();
    assert!(repo.statement_at("/m.yang", last, 0).is_none());

    // Unknown url.
    assert!(repo.statement_at("/nope.yang", 0, 0).is_none());
}

#[test]
fn goto_and_hover_read_argument_strings_of_interesting_statements() {
    let b = "module b {\n  namespace \"urn:b\";\n  prefix b;\n  typedef t { type string; }\n  grouping g { leaf gl { type string; } }\n}\n";
    let a = "module a {\n  namespace \"urn:a\";\n  prefix a;\n  import b {\n    prefix bb;\n  }\n  container c {\n    leaf x { type bb:t; }\n    uses bb:g;\n  }\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/b.yang", b);
    repo.upsert("/a.yang", a);

    // import: caret on the module-name argument -> the import statement.
    let (r, c) = pos(a, " b {");
    let hit = repo.statement_at("/a.yang", r, c + 1).unwrap();
    assert_eq!(hit.kind, StatementKind::Import);
    assert_eq!(arg_of(hit), "b"); // module to jump to

    // ... and the `prefix bb` inside it.
    let imp = repo
        .statement("/a.yang")
        .unwrap()
        .find_one(StatementKind::Import)
        .unwrap();
    let pfx = imp
        .children
        .iter()
        .find(|c| c.kind == StatementKind::Prefix)
        .unwrap();
    assert_eq!(arg_of(pfx), "bb");

    // type: prefixed arg `bb:t`.
    let (r, c) = pos(a, "bb:t");
    let hit = repo.statement_at("/a.yang", r, c).unwrap();
    assert_eq!(hit.kind, StatementKind::Type);
    assert_eq!(hit.arg.as_ref().unwrap().logical, "bb:t");

    // uses: prefixed arg `bb:g`.
    let (r, c) = pos(a, "bb:g");
    let hit = repo.statement_at("/a.yang", r, c).unwrap();
    assert_eq!(hit.kind, StatementKind::Uses);
    assert_eq!(hit.arg.as_ref().unwrap().logical, "bb:g");
}
