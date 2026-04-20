use indexmap::IndexMap;
use intervaltree::IntervalTree;
use ropey::Rope;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tree_sitter_yang::yang::statement::StatementKind;
use tree_sitter_yang::yang::token::{Token, TokenKind, tokenize};

#[derive(Error, Debug)]
pub enum YangError {
    #[error("Position {0}:{1} out of range")]
    OutOfRange(usize, usize),
    #[error("Parse error: {0}")]
    ParseError(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    Module(Range<usize>),
    Submodule(Range<usize>),
}

#[derive(Debug)]
pub struct Yang {
    pub uid: u64,
    document: Document,
    module_kind: ModuleKind,
    token_interval_tree: IntervalTree<usize, Token>,
    statement_dict: IndexMap<StatementKind, Vec<Token>>,
    tokens: Vec<Token>,
}

impl Yang {
    pub fn module_kind(&self) -> &ModuleKind {
        &self.module_kind
    }

    pub fn module_name(&self) -> String {
        match &self.module_kind {
            ModuleKind::Module(range) | ModuleKind::Submodule(range) => {
                self.document.get_ranged_text(range.clone())
            }
        }
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn closest_token(&self, row: usize, column: usize) -> Result<Token, YangError> {
        let offset = self.document.rope.line_to_byte(row) + column;
        let mut query_iter = self.token_interval_tree.query(offset..offset + 1);
        let mut closest: Option<Token> = None;
        while let Some(element) = query_iter.next() {
            if closest.is_none() {
                closest = Some(element.value.clone());
            } else {
                let current_closest = closest.as_ref().unwrap();
                if current_closest.range.len() < element.value.range.len() {
                    closest = Some(element.value.clone());
                }
            }
        }
        closest.ok_or(YangError::OutOfRange(row, column))
    }

    pub fn find_statement(&self, _kind: StatementKind) -> Vec<Token> {
        self.statement_dict
            .get(&_kind)
            .cloned()
            .unwrap_or_else(Vec::new)
    }

    pub fn get_document(&self) -> &Document {
        &self.document
    }
}

#[derive(Debug)]
pub struct Ryang {
    modules: IndexMap<u64, Arc<Yang>>,
}

impl Ryang {
    pub fn list(&self) -> Vec<Arc<Yang>> {
        self.modules.values().cloned().collect()
    }

    pub fn search(&self, name: &str) -> Vec<Arc<Yang>> {
        self.modules
            .iter()
            .filter(|(_, m)| name == m.module_name())
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn search1(&self, _name: &str, _rev: &str) -> Option<Arc<Yang>> {
        // Implement search with revision
        None // Placeholder
    }
}

#[derive(Debug)]
pub struct RyangBuild {
    documents: IndexMap<u64, Document>,
    next_uid: AtomicU64,
}

impl RyangBuild {
    pub fn new() -> Self {
        Self {
            documents: IndexMap::new(),
            next_uid: AtomicU64::new(0),
        }
    }

    pub fn create(&mut self, utf8_text: &str) -> u64 {
        let uid = self.next_uid.fetch_add(1, Ordering::Relaxed);
        let doc = Document::new(utf8_text);
        self.documents.insert(uid, doc);
        uid
    }

    pub fn update(&mut self, uid: u64, utf8_text: &str) -> Option<bool> {
        self.documents.get_mut(&uid).map(|doc| {
            doc.rope = Rope::from_str(utf8_text);
            true
        })
    }

    pub fn delete(&mut self, uid: u64) -> Option<u64> {
        self.documents.shift_remove(&uid).map(|_| uid)
    }

    pub fn compile(&mut self) -> Result<Arc<Ryang>, HashMap<u64, YangError>> {
        let mut modules = IndexMap::new();
        let mut errors = HashMap::new();
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
                            statement_dict: tokens.iter().fold(IndexMap::new(), |mut acc, t| {
                                if let TokenKind::Keyword(stmt_kind) = &t.kind {
                                    acc.entry(*stmt_kind).or_default().push(t.clone());
                                }
                                acc
                            }),
                            tokens,
                        };
                        modules.insert(uid.clone(), Arc::new(yang));
                    } else {
                    }
                }
                Err(error) => {
                    errors.insert(
                        uid.clone(),
                        YangError::ParseError(format!("UID {}: {:?}", uid, error)),
                    );
                }
            }
        }
        if errors.is_empty() {
            Ok(Arc::new(Ryang { modules }))
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    pub rope: Rope,
}

impl Document {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn get_text_of_line(&self, line: usize) -> Option<String> {
        self.rope.get_line(line).map(|s| s.to_string())
    }

    pub fn get_ranged_text(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn get_char_at(&self, row: usize, column: usize) -> Option<char> {
        self.rope
            .get_line(row)
            .and_then(|line| line.get_char(column))
    }

    pub fn enumerate_lines<F>(&self, mut f: F)
    where
        F: FnMut(usize, &str),
    {
        for (i, line) in self.rope.lines().enumerate() {
            f(i, line.as_str().unwrap_or(""));
        }
    }

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
        rb.documents.get(&uid).map(|doc| {
            assert_eq!(doc.rope.to_string(), "module updated {}");
        });
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
    fn test_yang_module_name() {
        let mut rb = RyangBuild::new();
        rb.create("module mymodule {\n  namespace \"urn:mymodule\";\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let module = &ryang.list()[0];
        assert_eq!(module.module_name(), "mymodule");
    }

    #[test]
    fn test_yang_closest_token() {
        let mut rb = RyangBuild::new();
        rb.create("module test {\n  prefix t\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let modules = ryang.list();
        let module = modules[0].clone();
        // Assuming tokens are parsed, test closest_token
        // This might need adjustment based on actual tokenization
        let token = module.closest_token(0, 0);
        assert!(token.is_ok());
    }

    #[test]
    fn test_yang_find_statement() {
        let mut rb = RyangBuild::new();
        rb.create("module test {\n  prefix t;\n}");
        let ryang = rb.compile().expect("Compilation should succeed");
        let module = &ryang.list()[0];
        let statements = module.find_statement(StatementKind::Prefix);
        assert!(!statements.is_empty());
    }
}
