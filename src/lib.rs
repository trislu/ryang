use indexmap::IndexMap;
use intervaltree::IntervalTree;
use ropey::Rope;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tree_sitter_yang::yang::statement::StatementKind;
use tree_sitter_yang::yang::token::{Token, TokenKind, tokenize};

#[derive(Error, Debug)]
/// Errors returned by YANG parsing and token lookup operations.
pub enum YangError {
    #[error("Position {0}:{1} out of range")]
    OutOfRange(usize, usize),
    #[error("Parse error: UID {0}: {1}")]
    ParseError(u64, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Top-level YANG statement kind and byte range of the module identifier.
pub enum ModuleKind {
    Module(Range<usize>),
    Submodule(Range<usize>),
}

#[derive(Debug)]
/// Parsed representation of a single YANG module or submodule document.
pub struct Yang {
    pub uid: u64,
    document: Document,
    module_kind: ModuleKind,
    token_interval_tree: IntervalTree<usize, Token>,
    token_dict: IndexMap<TokenKind, Vec<Token>>,
    tokens: Vec<Token>,
}

impl Yang {
    /// Returns the source document associated with this parsed module.
    pub fn get_document(&self) -> &Document {
        &self.document
    }

    /// Returns whether this entry is a `module` or `submodule`.
    pub fn module_kind(&self) -> &ModuleKind {
        &self.module_kind
    }

    /// Returns the module or submodule name.
    pub fn module_name(&self) -> String {
        match &self.module_kind {
            ModuleKind::Module(range) | ModuleKind::Submodule(range) => {
                self.document.get_ranged_text(range.clone())
            }
        }
    }

    /// Returns all tokens produced during parsing.
    pub fn list_token(&self) -> &[Token] {
        &self.tokens
    }

    /// Returns all tokens matching the specified kind.
    pub fn search_token(&self, kind: TokenKind) -> Vec<Token> {
        self.token_dict.get(&kind).cloned().unwrap_or_else(Vec::new)
    }

    /// Returns the narrowest token that contains the given row/column position.
    pub fn search_narrowest_token(&self, row: usize, column: usize) -> Result<Token, YangError> {
        let offset = self.document.rope.line_to_byte(row) + column;
        let mut narrowest: Option<Token> = None;
        for element in self.token_interval_tree.query(offset..offset + 1) {
            if narrowest.is_none() {
                narrowest = Some(element.value.clone());
            } else {
                let current_narrowest = narrowest.as_ref().unwrap();
                if current_narrowest.range.len() < element.value.range.len() {
                    narrowest = Some(element.value.clone());
                }
            }
        }
        narrowest.ok_or(YangError::OutOfRange(row, column))
    }
}

#[derive(Debug)]
/// Immutable index of compiled YANG modules grouped by module name.
pub struct Ryang {
    modules: IndexMap<String, Vec<Arc<Yang>>>,
}

impl Ryang {
    /// Returns all compiled modules and submodules.
    pub fn list(&self) -> Vec<Arc<Yang>> {
        self.modules.values().flatten().cloned().collect()
    }

    /// Returns all compiled entries matching a module name.
    pub fn search(&self, name: &str) -> Vec<Arc<Yang>> {
        self.modules.get(name).cloned().unwrap_or_else(Vec::new)
    }

    /// Returns one compiled entry by module name and exact `revision` argument.
    pub fn search1(&self, name: &str, rev: &str) -> Option<Arc<Yang>> {
        let candidates: Vec<Arc<Yang>> = self.search(name);
        candidates.into_iter().find(|m| {
            m.search_token(TokenKind::Argument(StatementKind::Revision))
                .iter()
                .any(|t| {
                    let rev_text = m.get_document().get_ranged_text(t.range.clone());
                    rev_text == rev
                })
        })
    }
}

#[derive(Debug, Default)]
/// Mutable builder used to create and compile an in-memory YANG workspace.
pub struct RyangBuild {
    documents: IndexMap<u64, Document>,
    next_uid: AtomicU64,
}

impl RyangBuild {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self {
            documents: IndexMap::new(),
            next_uid: AtomicU64::new(0),
        }
    }

    /// Inserts a UTF-8 document and returns its unique identifier.
    pub fn create(&mut self, utf8_text: &str) -> u64 {
        let uid = self.next_uid.fetch_add(1, Ordering::Relaxed);
        let doc = Document::new(utf8_text);
        self.documents.insert(uid, doc);
        uid
    }

    /// Replaces a document by identifier and returns `Some(true)` on success.
    pub fn update(&mut self, uid: u64, utf8_text: &str) -> Option<bool> {
        self.documents.get_mut(&uid).map(|doc| {
            doc.rope = Rope::from_str(utf8_text);
            true
        })
    }

    /// Removes a document by identifier and returns the removed identifier.
    pub fn delete(&mut self, uid: u64) -> Option<u64> {
        self.documents.shift_remove(&uid).map(|_| uid)
    }

    /// Compiles all stored documents into a searchable index.
    ///
    /// Returns a map of per-document parse errors keyed by `uid` when any
    /// document fails tokenization.
    pub fn compile(&mut self) -> Result<Arc<Ryang>, YangError> {
        let mut modules: IndexMap<String, Vec<Arc<Yang>>> = IndexMap::new();
        for (uid, doc) in &self.documents {
            let mut module_kind: Option<ModuleKind> = None;
            match tokenize(&doc.rope.to_string(), |token| {
                if token.kind == TokenKind::Argument(StatementKind::Module) {
                    module_kind = Some(ModuleKind::Module(token.range.clone()));
                } else if token.kind == TokenKind::Argument(StatementKind::Submodule) {
                    module_kind = Some(ModuleKind::Submodule(token.range.clone()));
                }
            }) {
                Ok(tokens) => {
                    if let Some(kind) = module_kind {
                        let yang = Yang {
                            uid: *uid,
                            document: doc.clone(),
                            module_kind: kind,
                            token_interval_tree: tokens
                                .iter()
                                .map(|t| (t.range.clone(), t.clone()))
                                .collect(),
                            token_dict: tokens.iter().fold(IndexMap::new(), |mut acc, t| {
                                acc.entry(t.kind.clone()).or_default().push(t.clone());
                                acc
                            }),
                            tokens,
                        };
                        modules
                            .entry(yang.module_name().to_string())
                            .or_default()
                            .push(Arc::new(yang));
                    }
                }
                Err(error) => {
                    return Err(YangError::ParseError(*uid, format!("{:?}", error)));
                }
            }
        }
        Ok(Arc::new(Ryang { modules }))
    }
}

#[derive(Debug, Clone)]
/// Rope-backed UTF-8 document helper with line/offset utilities.
pub struct Document {
    pub rope: Rope,
}

impl Document {
    /// Creates a new document from UTF-8 text.
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    /// Returns number of lines in the document.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Returns text for the given line index.
    pub fn get_text_of_line(&self, line: usize) -> Option<String> {
        self.rope.get_line(line).map(|s| s.to_string())
    }

    /// Returns text in the given byte range.
    pub fn get_ranged_text(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    /// Returns the character at row/column if available.
    pub fn get_char_at(&self, row: usize, column: usize) -> Option<char> {
        self.rope
            .get_line(row)
            .and_then(|line| line.get_char(column))
    }

    /// Calls `f` for each line with `(line_index, line_text)`.
    pub fn enumerate_lines<F>(&self, mut f: F)
    where
        F: FnMut(usize, &str),
    {
        for (i, line) in self.rope.lines().enumerate() {
            f(i, line.as_str().unwrap_or(""));
        }
    }

    /// Converts a byte offset into `(line, column)`.
    pub fn byte_offset_to_point(&self, offset: usize) -> (usize, usize) {
        let line = self.rope.byte_to_line(offset);
        let column = offset - self.rope.line_to_byte(line);
        (line, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_new() {
        let doc = Document::new("hello\nworld");
        assert_eq!(doc.line_count(), 2);
    }

    #[test]
    fn test_document_get_text_of_line() {
        let doc = Document::new("line1\nline2\nline3");
        assert_eq!(doc.get_text_of_line(0), Some("line1\n".to_string()));
        assert_eq!(doc.get_text_of_line(2), Some("line3".to_string()));
        assert_eq!(doc.get_text_of_line(3), None);
    }

    #[test]
    fn test_document_get_ranged_text() {
        let doc = Document::new("hello world");
        assert_eq!(doc.get_ranged_text(0..5), "hello");
        assert_eq!(doc.get_ranged_text(6..11), "world");
    }

    #[test]
    fn test_document_get_char_at() {
        let doc = Document::new("ab\ncd");
        assert_eq!(doc.get_char_at(0, 0), Some('a'));
        assert_eq!(doc.get_char_at(1, 1), Some('d'));
        assert_eq!(doc.get_char_at(2, 0), None);
    }

    #[test]
    fn test_document_enumerate_lines() {
        let doc = Document::new("line1\nline2");
        let mut lines = Vec::new();
        doc.enumerate_lines(|i, s| lines.push((i, s.to_string())));
        assert_eq!(
            lines,
            vec![(0, "line1\n".to_string()), (1, "line2".to_string())]
        );
    }

    #[test]
    fn test_document_byte_offset_to_point() {
        let doc = Document::new("hello\nworld");
        assert_eq!(doc.byte_offset_to_point(0), (0, 0));
        assert_eq!(doc.byte_offset_to_point(5), (0, 5)); // after 'o'
        assert_eq!(doc.byte_offset_to_point(6), (1, 0)); // 'w'
    }

    #[test]
    fn test_ryang_build_new() {
        let rb = RyangBuild::new();
        assert!(rb.documents.is_empty());
        assert_eq!(rb.next_uid.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_ryang_build_create() {
        let mut rb = RyangBuild::new();
        let uid = rb.create("module test {}");
        assert_eq!(uid, 0);
        assert!(rb.documents.contains_key(&0));
    }

    #[test]
    fn test_ryang_build_update() {
        let mut rb = RyangBuild::new();
        let uid = rb.create("module test {}");
        assert!(rb.documents.contains_key(&uid)); // New document should be present
        let result = rb.update(uid, "module updated {}");
        assert_eq!(result, Some(true));
        assert!(rb.documents.contains_key(&uid)); // Old document should be replaced
        //rb.documents.get(&uid).map(|doc| {
        if let Some(doc) = rb.documents.get(&uid) {
            assert_eq!(doc.rope.to_string(), "module updated {}");
        }
    }

    #[test]
    fn test_ryang_build_update_missing_uid() {
        let mut rb = RyangBuild::new();
        assert_eq!(rb.update(999, "module updated {}"), None);
    }

    #[test]
    fn test_ryang_build_delete() {
        let mut rb = RyangBuild::new();
        let uid = rb.create("module test {}");
        let result = rb.delete(uid);
        assert_eq!(result, Some(uid));
        assert!(!rb.documents.contains_key(&uid));
    }

    #[test]
    fn test_ryang_build_delete_missing_uid() {
        let mut rb = RyangBuild::new();
        assert_eq!(rb.delete(999), None);
    }

    #[test]
    fn test_ryang_build_compile() {
        let mut rb = RyangBuild::new();
        rb.create("module test {\n  namespace \"urn:test\";\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        assert_eq!(ryang.list().len(), 1);
        let module = &ryang.list()[0];
        assert_eq!(module.module_name(), "test");
    }

    #[test]
    fn test_ryang_list() {
        let mut rb = RyangBuild::new();
        rb.create("module test1 {}");
        rb.create("module test2 {}");
        let ryang = rb.compile().expect("Compilation should succeed");
        assert_eq!(ryang.list().len(), 2);
    }

    #[test]
    fn test_ryang_search() {
        let mut rb = RyangBuild::new();
        rb.create("module test1 {}");
        rb.create("module test2 {}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let results = ryang.search("test1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].module_name(), "test1");
    }

    #[test]
    fn test_ryang_search1_by_revision() {
        let mut rb = RyangBuild::new();
        rb.create("module testrev {\n  namespace \"urn:testrev\";\n  revision 2024-01-01;\n}");
        rb.create("module testrev {\n  namespace \"urn:testrev\";\n  revision 2023-01-01;\n}");
        let ryang = rb.compile().expect("Compilation should succeed");

        let found = ryang.search1("testrev", "2024-01-01");
        assert!(found.is_some());
        assert_eq!(found.expect("should have one").module_name(), "testrev");

        let missing = ryang.search1("testrev", "1999-01-01");
        assert!(missing.is_none());
    }

    #[test]
    fn test_yang_module_name() {
        let mut rb = RyangBuild::new();
        rb.create("module mymodule {\n  namespace \"urn:mymodule\";\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let module = &ryang.list()[0];
        assert_eq!(module.module_name(), "mymodule");
    }

    #[test]
    fn test_yang_get_document_and_list_token_and_module_kind() {
        let mut rb = RyangBuild::new();
        rb.create("module sample {\n  namespace \"urn:sample\";\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let module = &ryang.list()[0];

        assert!(module.get_document().line_count() >= 1);
        assert!(!module.list_token().is_empty());

        match module.module_kind() {
            ModuleKind::Module(_) => {}
            ModuleKind::Submodule(_) => panic!("Unexpected module kind"),
        }
    }

    #[test]
    fn test_yang_search_narrowest_token() {
        let mut rb = RyangBuild::new();
        rb.create("module test {\n  prefix t\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let modules = ryang.list();
        let module = modules[0].clone();
        // Assuming tokens are parsed, test search_narrowest_token
        // This might need adjustment based on actual tokenization
        let token = module.search_narrowest_token(0, 0);
        assert!(token.is_ok());
    }

    #[test]
    fn test_yang_search_token() {
        let mut rb = RyangBuild::new();
        rb.create("module test {\n  prefix t;\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let module = &ryang.list()[0];
        let statements = module.search_token(TokenKind::Keyword(StatementKind::Prefix));
        assert!(!statements.is_empty());
    }
}
