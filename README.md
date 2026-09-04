# yrepo — yet another YANG repository

[![Rust CI](https://github.com/trislu/yrepo/actions/workflows/rust.yml/badge.svg)](https://github.com/trislu/yrepo/actions/workflows/rust.yml)
[![Cargo Publish](https://github.com/trislu/yrepo/actions/workflows/cargo-publish.yml/badge.svg)](https://github.com/trislu/yrepo/actions/workflows/cargo-publish.yml)
[![GitHub Release](https://github.com/trislu/yrepo/actions/workflows/github-release.yml/badge.svg)](https://github.com/trislu/yrepo/actions/workflows/github-release.yml)

An [LSP](https://microsoft.github.io/language-server-protocol/)-friendly YANG
schema toolkit (parse + resolved semantic model), written in Rust. Currently
supports `*.yang` only.

## Two entry points

- **`Repository`** — manages the open documents of a workspace by `url`
  (`upsert` / `remove`) and compiles them.
- **`Library`** — the resolved snapshot returned by `Repository::compile()`:
  modules (with their folded submodules), the *effective/expanded* schema tree,
  symbol tables, and cross-module queries.

```rust
use yrepo::Repository;

let mut repo = Repository::new();
repo.upsert("/mods/m.yang", "module m { namespace \"urn:m\"; prefix m; leaf x { type string; } }");
let outcome = repo.compile();
for d in &outcome.diagnostics {
    println!("{}: {} {}", d.severity, d.code, d.message);
}
let lib = outcome.library.expect("library");
let m = lib.module("m").expect("module m");
println!("{} top-level nodes", m.top_nodes().len());
```

## Syntax view (statement tree)

For fold / format / highlight / goto-hover, the parsed document also exposes its
tree-sitter-free **statement tree** (no second parse needed):

```rust
use yrepo::{Repository, StatementKind};

let mut repo = Repository::new();
repo.upsert("/m.yang", "module m { namespace \"urn:m\"; prefix m; container c { leaf x { type string; } } }");

// Whole document tree, rooted at the `module`/`submodule` statement.
let root = repo.statement("/m.yang").expect("module m");
for stmt in root.preorder() {
    // kind, keyword span, argument text, `;` vs `{…}` terminator, children…
    let arg = stmt.arg.as_ref().map(|a| a.logical.as_str()).unwrap_or("");
    println!("{:?} {:?}", stmt.kind, arg);
}

// The narrowest statement under the caret — read its argument for goto/hover.
// Col 40 is the `container` keyword in the line above.
if let Some(stmt) = repo.statement_at("/m.yang", 0, 40) {
    assert_eq!(stmt.kind, StatementKind::Container);
}

// Comments are not part of the statement tree; they are exposed separately
// (in source order) so a formatter never deletes them.
let comments = repo.comments("/m.yang").unwrap();
assert!(comments.is_empty());
```

## Notes

- User-content problems (parse errors, unresolved imports, bad keys, …) are
  reported as **diagnostics** — never as `Err`.
- Circular chains of imports and include cycles are reported as diagnostics
  (RFC 7950 §5.1 forbids circular chains of imports).
- Groupings are instantiated into the effective tree at each `uses` site, and
  cross-module `augment`/`deviation` targets are resolved onto it.

## Layout

```bash
src/
  lib.rs        # facade: Repository + re-exports
  text.rs       # raw source + line/offset access
  syntax.rs     # CST layer
  yang.rs       # syntactic per-doc model + header extraction
  compile.rs    # symbol scan, effective-tree expansion, augment/deviation
  schema.rs     # semantic model types (effective nodes, module records)
  library.rs    # Library + queries
  diag.rs       # Diagnostic / Severity / DiagnosticCode
tests/
  NNN_*.rs      # numbered integration tests
  sample_yang/  # fixture files
```

## License

This project is licensed under the [MIT License](LICENSE)
