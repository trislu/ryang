//! Regression: a `*.yang` file whose content is actually HTML/XML (first
//! non-whitespace char `<`) is not YANG at all. It should be reported once as
//! `not-a-yang-document` (warning); the whole-file `parse-error` (error)
//! previously reported alongside it was noise.

mod test_utils;

use yrepo::{DiagnosticCode, Repository, Severity};

#[test]
fn test_html_document_is_not_a_parse_error() {
    let mut repo = Repository::new();
    repo.upsert(
        "/not-yang.html.yang",
        r#"<link rel="stylesheet" href="/styles.css"><!DOCTYPE html>
<html><head><title>Not YANG</title></head><body>hi</body></html>
"#,
    );
    let outcome = repo.compile();
    let codes: Vec<(Severity, DiagnosticCode)> = outcome
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.code))
        .collect();
    assert_eq!(
        codes,
        vec![(Severity::Warning, DiagnosticCode::NotYangDocument)],
        "expected a single not-a-yang warning, got {codes:?}"
    );
    assert!(outcome.library.is_none());
}

#[test]
fn test_bom_leading_html_still_detected() {
    let mut repo = Repository::new();
    repo.upsert("/bom.html.yang", "\u{feff}<!DOCTYPE html>\n<html></html>\n");
    let outcome = repo.compile();
    assert!(
        outcome
            .diagnostics
            .iter()
            .all(|d| d.code == DiagnosticCode::NotYangDocument)
    );
}

#[test]
fn test_real_yang_still_reports_parse_errors() {
    // A genuine YANG document with a syntax problem must keep its parse-error
    // (`type string }` is missing the terminating `;`).
    let mut repo = Repository::new();
    repo.upsert(
        "/broken.yang",
        "module m { namespace \"urn:m\"; prefix m; leaf a { type string } }",
    );
    let outcome = repo.compile();
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::ParseError)
    );
}
