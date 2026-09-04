//! Comment exposure — `Repository::comments`.
//!
//! The statement tree does not model comments (they live between statements and
//! inside block bodies), so they are exposed as a document-ordered list for
//! format / highlight / comment-out quick-fixes. Covers: order + kind + raw
//! text/range, string-safety (comment markers inside quoted args are *not*
//! comments), trailing/leading comments outside the module, and empty docs.

use yrepo::{CommentKind, Repository};

#[test]
fn comments_are_collected_in_source_order() {
    let src = "//! header\nmodule m {\n  // before namespace\n  namespace \"urn:m\"; /* after namespace */\n  prefix m;\n  container c {\n    // inner comment\n    leaf x { type string; }\n  }\n  description \"not // a comment, nor /* one */\";\n}\n// trailing after module\n";
    let mut repo = Repository::new();
    repo.upsert("/m.yang", src);

    let comments = repo.comments("/m.yang").expect("doc present");
    let texts: Vec<&str> = comments.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "//! header",
            "// before namespace",
            "/* after namespace */",
            "// inner comment",
            "// trailing after module",
        ]
    );

    let kinds: Vec<CommentKind> = comments.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            CommentKind::Line,
            CommentKind::Line,
            CommentKind::Block,
            CommentKind::Line,
            CommentKind::Line,
        ]
    );

    // Ranges are in document order and slice back to the raw text.
    let mut prev_end = 0usize;
    for c in comments {
        assert!(c.range.start >= prev_end, "comments must be ordered");
        prev_end = c.range.end;
        assert_eq!(&src[c.range.clone()], c.text, "range must cover the text");
        assert!(c.range.end <= src.len());
    }
}

#[test]
fn comment_markers_inside_quoted_arguments_are_not_comments() {
    // `//`, `/* */` inside the quoted description must stay part of the string
    // and never appear as comments. `// url` on a bare line afterwards *is*.
    let src = "module m {\n  namespace \"urn:m\";\n  prefix m;\n  description \"see http://x and /* note */\";\n  // a real comment\n}\n";
    let mut repo = Repository::new();
    repo.upsert("/m.yang", src);

    let comments = repo.comments("/m.yang").unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].text, "// a real comment");

    // The string argument is intact and contains the markers.
    let root = repo.statement("/m.yang").unwrap();
    let desc = root.find_one(yrepo::StatementKind::Description).unwrap();
    let arg = desc.arg.as_ref().unwrap();
    assert_eq!(arg.logical, "see http://x and /* note */");
    assert_eq!(&src[arg.range.clone()], "\"see http://x and /* note */\"");
}

#[test]
fn comments_empty_docs_and_unknown_urls() {
    let mut repo = Repository::new();
    repo.upsert(
        "/no-comments.yang",
        "module n {\n  namespace \"urn:n\";\n  prefix n;\n}\n",
    );
    repo.upsert(
        "/with.yang",
        "// c\nmodule w {\n  namespace \"urn:w\";\n  prefix w;\n}\n",
    );

    // Unknown url -> None; a comment-free doc -> empty list.
    assert!(repo.comments("/nope.yang").is_none());
    assert_eq!(repo.comments("/no-comments.yang").unwrap().len(), 0);
    assert_eq!(repo.comments("/with.yang").unwrap().len(), 1);
}
