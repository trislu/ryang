mod test_utils;

use std::collections::HashSet;

use test_utils::load_file;
use yrepo::{Repository, TokenKind};

const DOC: &str = r#"module m {
  // hi
  namespace "urn:m";
  prefix m;
  leaf d {
    type uint16 { range "1..10"; }
    default "8080";
  }
  leaf f { type decimal64 { fraction-digits 2; } }
  leaf c { type boolean; config true; }
  leaf s { type string; description "foo" + "bar"; }
}
"#;

/// `Repository::tokens` exposes the grammar's raw lexical stream: keywords,
/// identifiers, quoted strings, numbers, booleans and `+` — disjoint, sorted,
/// source-order spans the Statement tree drops.
#[test]
fn test_token_stream_kinds() {
    let mut repo = Repository::new();
    repo.upsert("/m.yang", DOC);
    let tokens = repo.tokens("/m.yang").expect("tokens");

    let has = |kind: TokenKind, text: &str| tokens.iter().any(|t| t.kind == kind && t.text == text);

    // keywords vs identifiers
    assert!(has(TokenKind::Keyword, "module"));
    assert!(has(TokenKind::Keyword, "range"));
    assert!(has(TokenKind::Keyword, "config"));
    assert!(has(TokenKind::Identifier, "m"));
    assert!(has(TokenKind::Identifier, "d"));

    // quoted strings keep their quotes and are not split
    assert!(has(TokenKind::String, "\"urn:m\""));
    assert!(has(TokenKind::String, "\"8080\""));
    assert!(has(TokenKind::String, "\"foo\""));
    assert!(has(TokenKind::String, "\"bar\""));
    // "8080" lives *inside* a string — it must not also be a number
    assert!(!has(TokenKind::Number, "8080"));
    // a quoted `range` argument is one opaque string (RFC 7950 quoted-string)
    assert!(has(TokenKind::String, "\"1..10\""));

    // unquoted numbers (fraction-digits) still tokenize as numbers
    assert!(has(TokenKind::Number, "2"));

    // booleans and the `+` concat operator
    assert!(has(TokenKind::Boolean, "true"));
    assert!(has(TokenKind::Operator, "+"));

    // comments are part of the stream too
    let comment_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .count();
    assert_eq!(
        comment_count,
        repo.comments("/m.yang").map(|c| c.len()).unwrap_or(0)
    );
    assert!(comment_count >= 1);
}

#[test]
fn test_token_spans_are_disjoint_and_ordered() {
    let mut repo = Repository::new();
    repo.upsert("/m.yang", DOC);
    let tokens = repo.tokens("/m.yang").expect("tokens");

    let mut prev_end = 0usize;
    for t in tokens {
        assert!(
            t.range.start >= prev_end,
            "overlapping or out-of-order token {t:?}"
        );
        assert!(t.range.start < t.range.end, "empty token {t:?}");
        prev_end = t.range.end;
    }
}

#[test]
fn test_tokens_on_sample_and_unknown_url() {
    // sample files tokenize without panicking
    let repo = load_file("types.yang");
    let _ = repo.tokens("tests/sample_yang/types.yang");

    // unknown url -> None
    let repo = Repository::new();
    assert!(repo.tokens("/nope.yang").is_none());
}

#[test]
fn test_token_text_set_is_stable() {
    // sanity: the set of distinct token texts matches expectations for DOC
    let mut repo = Repository::new();
    repo.upsert("/m.yang", DOC);
    let texts: HashSet<&str> = repo
        .tokens("/m.yang")
        .unwrap()
        .iter()
        .map(|t| t.text.as_str())
        .collect();
    for expected in [
        "module",
        "namespace",
        "range",
        "true",
        "+",
        "\"foo\"",
        "\"1..10\"",
        "\"8080\"",
    ] {
        assert!(texts.contains(expected), "missing token text {expected:?}");
    }
    let _ = texts;
}
