//! Opt-in text-light parsing: description/reference/organization/contact
//! statements are dropped from the Statement tree and their quoted runs from
//! the token stream; schema resolution is unaffected. Default (no flag) keeps
//! everything, so existing behavior/tests are untouched.

use std::sync::Arc;

use yrepo::{Library, Repository, StatementKind};

const SRC: &str = "module tl { namespace \"urn:tl\"; prefix tl;\n\
  description \"very long prose that no LSP feature reads\";\n\
  reference \"RFC XXXX\";\n\
  container c { leaf x { type string; } }\n\
  leaf y { type string; }\n\
}\n";

fn compile_with(repo: &mut Repository, url: &str) -> Arc<Library> {
    repo.upsert(url, SRC);
    let out = repo.compile();
    let diags = out.diagnostics.clone();
    assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
    out.library.expect("lib")
}

fn desc_count(root: &yrepo::Statement) -> usize {
    root.preorder()
        .filter(|s| s.kind == StatementKind::Description)
        .count()
}

fn has_phrase(repo: &Repository, url: &str, phrase: &str) -> bool {
    repo.tokens(url)
        .map(|ts| ts.iter().any(|t| t.text.contains(phrase)))
        .unwrap_or(false)
}

#[test]
fn default_keeps_text_statements_and_tokens() {
    let mut repo = Repository::new();
    let lib = compile_with(&mut repo, "/tl.yang");
    let root = repo.statement("/tl.yang").expect("root");
    assert_eq!(desc_count(root), 1);
    assert!(has_phrase(&repo, "/tl.yang", "very long prose"));
    let m = lib.module("tl").expect("module");
    assert_eq!(m.top_nodes().len(), 2); // container c + leaf y
}

#[test]
fn text_light_drops_text_statements_and_their_tokens() {
    let mut repo = Repository::new();
    repo.set_text_light(true);
    let lib = compile_with(&mut repo, "/tl2.yang");
    let root = repo.statement("/tl2.yang").expect("root");
    assert_eq!(desc_count(root), 0);
    assert!(!has_phrase(&repo, "/tl2.yang", "very long prose"));
    let m = lib.module("tl").expect("module");
    assert_eq!(m.top_nodes().len(), 2); // schema unchanged
}
