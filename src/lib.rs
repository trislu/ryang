use intervaltree::IntervalTree;
use ropey::Rope;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tree_sitter_yang::yang::statement::StatementKind;
use tree_sitter_yang::yang::token::{Token, TokenKind, tokenize};

static YANG_NEXT_UID: AtomicU64 = AtomicU64::new(0);

#[derive(Error, Debug)]
/// Errors returned by YANG parsing and token lookup operations.
pub enum YangError {
    #[error("Position {0}:{1} out of range")]
    OutOfRange(usize, usize),
    #[error("Parse error: UID {0}: {1}")]
    ParseError(u64, String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Top-level YANG statement kind and byte range of the module identifier.
pub enum ModuleKind {
    Module(Range<usize>),
    Submodule(Range<usize>),
}

#[derive(Clone, Debug)]
struct SyntaticData {
    // syntatic information about the module/submodule statement, if present
    module_kind: Option<ModuleKind>,
    // syntatic information about all tokens in the document, indexed by byte range and token kind
    token_interval_tree: IntervalTree<usize, Token>,
    // syntatic information about all tokens in the document, indexed by token kind
    token_dict: HashMap<TokenKind, Vec<Token>>,
}

#[derive(Clone, Debug)]
/// Parsed representation of a single YANG module or submodule document.
pub struct Yang {
    // raw text with Rope utilities
    rope: Rope,
    // syntatic information about the module/submodule statement, if present
    syntatic_data: SyntaticData,
}

impl Yang {
    /// new creates a Yang instance from a UTF-8 text and its associated tokens.
    pub(crate) fn new(text: &str) -> Self {
        let rope = Rope::from_str(text);
        Self {
            rope,
            syntatic_data: Yang::parse(text),
        }
    }

    pub(crate) fn update(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.syntatic_data = Yang::parse(text);
    }

    fn parse(text: &str) -> SyntaticData {
        let mut module_kind: Option<ModuleKind> = None;
        let tokens = tokenize(text, |token| {
            if token.kind == TokenKind::Argument(StatementKind::Module) {
                module_kind = Some(ModuleKind::Module(token.range.clone()));
            } else if token.kind == TokenKind::Argument(StatementKind::Submodule) {
                module_kind = Some(ModuleKind::Submodule(token.range.clone()));
            }
        })
        .unwrap_or_else(|_| vec![]);
        SyntaticData {
            module_kind,
            token_interval_tree: IntervalTree::from_iter(
                tokens.iter().map(|t| (t.range.clone(), t.clone())),
            ),
            token_dict: tokens.iter().fold(
                HashMap::<TokenKind, Vec<Token>>::new(),
                |mut acc, t| {
                    acc.entry(t.kind.clone()).or_default().push(t.clone());
                    acc
                },
            ),
        }
    }

    /// Returns number of lines in the document.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Returns text for the given line index.
    pub fn get_line(&self, line: usize) -> Option<String> {
        self.rope.get_line(line).map(|s| s.to_string())
    }

    /// Returns text in the given byte range.
    pub fn get_slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    /// Returns the character at row/column if available.
    pub fn get_char(&self, row: usize, column: usize) -> Option<char> {
        self.rope
            .get_line(row)
            .and_then(|line| line.get_char(column))
    }

    /// Calls `f` for each line with `(line_index, line_text)`.
    pub fn foreach_line<F>(&self, mut f: F)
    where
        F: FnMut(usize, &str),
    {
        for (i, line) in self.rope.lines().enumerate() {
            f(i, line.as_str().unwrap_or(""));
        }
    }

    /// Converts a byte offset into `(line, column)`.
    pub fn byte_to_point(&self, offset: usize) -> (usize, usize) {
        let line = self.rope.byte_to_line(offset);
        let column = offset - self.rope.line_to_byte(line);
        (line, column)
    }

    /// Returns whether this entry is a `module` or `submodule`.
    pub fn module_kind(&self) -> Option<ModuleKind> {
        self.syntatic_data.module_kind.clone()
    }

    /// Returns the module or submodule name.
    pub fn module_name(&self) -> Option<String> {
        match self.module_kind() {
            Some(ModuleKind::Module(range)) | Some(ModuleKind::Submodule(range)) => {
                Some(self.get_slice(range.clone()))
            }
            None => None,
        }
    }

    /// Returns all tokens matching the specified kind.
    pub fn search_token(&self, kind: TokenKind) -> Vec<Token> {
        self.syntatic_data
            .token_dict
            .get(&kind)
            .cloned()
            .unwrap_or_else(Vec::new)
    }

    /// Returns the narrowest token that contains the given row/column position.
    pub fn search_narrowest_token(&self, row: usize, column: usize) -> Result<Token, YangError> {
        let offset = self.rope.line_to_byte(row) + column;
        let mut narrowest: Option<Token> = None;
        for element in self
            .syntatic_data
            .token_interval_tree
            .query(offset..offset + 1)
        {
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

#[derive(Debug, Default)]
/// Immutable index of compiled YANG modules grouped by module name.
pub struct Ryang {
    uid_dict: HashMap<String, u64>,
    name_dict: HashMap<String, Vec<u64>>,
    yang_dict: HashMap<u64, Yang>,
}

impl Ryang {
    /// Inserts a UTF-8 document and returns its unique identifier.
    pub fn parse(&mut self, uri: &str, source: &str) -> Result<(), YangError> {
        if let Some(existing_uid) = self.uid_dict.get(uri) {
            if let Some(existing_yang) = self.yang_dict.get_mut(existing_uid) {
                if let Some(name) = existing_yang.module_name() {
                    // Remove the existing UID from the name_dict for the old module name, if it existss
                    if let Some(uids) = self.name_dict.get_mut(&name) {
                        uids.retain(|&x| x != *existing_uid);
                        if uids.is_empty() {
                            self.name_dict.remove(&name);
                        }
                    }
                }
                existing_yang.update(source);
                // Add the existing UID to the name_dict for the new module name, if it exists
                if let Some(new_name) = existing_yang.module_name() {
                    self.name_dict
                        .entry(new_name.clone())
                        .or_default()
                        .push(*existing_uid);
                }
                return Ok(());
            }
            return Err(YangError::Internal(format!(
                "UID {} exists but no corresponding Yang found",
                existing_uid
            )));
        }
        let uid = YANG_NEXT_UID.fetch_add(1, Ordering::Relaxed);
        let yang = Yang::new(source);
        self.uid_dict.insert(uri.to_string(), uid);
        if let Some(name) = &yang.module_name() {
            self.name_dict.entry(name.clone()).or_default().push(uid);
        }
        self.yang_dict.insert(uid, yang);
        Ok(())
    }

    /// Removes a document by identifier.
    pub fn remove(&mut self, uri: &str) -> Result<String, YangError> {
        if let Some(uid) = self.uid_dict.remove(uri) {
            if let Some(yang) = self.yang_dict.remove(&uid) {
                if let Some(name) = yang.module_name()
                    && let Some(uids) = self.name_dict.get_mut(&name)
                {
                    uids.retain(|&x| x != uid);
                    if uids.is_empty() {
                        self.name_dict.remove(&name);
                    }
                }
                return Ok(uri.to_string());
            }
            Err(YangError::Internal(format!(
                "UID {} exists but no corresponding Yang found",
                uid
            )))
        } else {
            Err(YangError::NotFound(uri.to_string()))
        }
    }

    /// Returns all modules and submodules.
    pub fn list(&self) -> Vec<&Yang> {
        self.yang_dict.values().collect()
    }

    /// Returns all modules matching a module name.
    pub fn search(&self, name: &str) -> Vec<&Yang> {
        if let Some(uids) = self.name_dict.get(name) {
            uids.iter()
                .filter_map(|uid| self.yang_dict.get(uid))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Returns one module by module name and exact `revision` argument.
    pub fn search1(&self, name: &str, rev: &str) -> Option<&Yang> {
        let candidates = self.search(name);
        candidates.into_iter().find(|m| {
            m.search_token(TokenKind::Argument(StatementKind::Revision))
                .iter()
                .any(|t| {
                    let rev_text = m.get_slice(t.range.clone());
                    rev_text == rev
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yang_new() {
        let yang = Yang::new("hello\nworld");
        assert_eq!(yang.line_count(), 2);
    }

    #[test]
    fn test_yang_get_line() {
        let yang: Yang = Yang::new("line1\nline2\nline3");
        assert_eq!(yang.get_line(0), Some("line1\n".to_string()));
        assert_eq!(yang.get_line(2), Some("line3".to_string()));
        assert_eq!(yang.get_line(3), None);
    }

    #[test]
    fn test_yang_get_slice() {
        let yang = Yang::new("hello world");
        assert_eq!(yang.get_slice(0..5), "hello");
        assert_eq!(yang.get_slice(6..11), "world");
    }

    #[test]
    fn test_yang_get_char() {
        let yang = Yang::new("ab\ncd");
        assert_eq!(yang.get_char(0, 0), Some('a'));
        assert_eq!(yang.get_char(1, 1), Some('d'));
        assert_eq!(yang.get_char(2, 0), None);
    }

    #[test]
    fn test_yang_enumerate_lines() {
        let yang = Yang::new("line1\nline2");
        let mut lines = Vec::new();
        yang.foreach_line(|i, s| lines.push((i, s.to_string())));
        assert_eq!(
            lines,
            vec![(0, "line1\n".to_string()), (1, "line2".to_string())]
        );
    }

    #[test]
    fn test_yang_byte_offset_to_point() {
        let yang = Yang::new("hello\nworld");
        assert_eq!(yang.byte_to_point(0), (0, 0));
        assert_eq!(yang.byte_to_point(5), (0, 5)); // after 'o'
        assert_eq!(yang.byte_to_point(6), (1, 0)); // 'w'
    }

    #[test]
    fn test_ryang_parse() {
        let mut ryang = Ryang::default();
        assert!(ryang.parse("/foo/test.yang", "module test {}").is_ok());
        assert!(ryang.uid_dict.contains_key("/foo/test.yang"));
        assert!(ryang.search("test").len() == 1);
        assert!(ryang.search1("test", "2024-01-01").is_none()); // No revision, should not find
    }

    #[test]
    fn test_ryang_parse_update() {
        let mut ryang = Ryang::default();
        assert!(ryang.parse("/foo/test.yang", "module test {}").is_ok());
        assert!(ryang.uid_dict.contains_key("/foo/test.yang"));
        assert!(ryang.search("test").len() == 1); // "test" module should be present
        assert!(ryang.parse("/foo/test.yang", "module updated {}").is_ok());
        assert!(ryang.uid_dict.contains_key("/foo/test.yang"));
        assert!(ryang.search("updated").len() == 1); // "updated" module should be present after update
        assert!(ryang.search("test").is_empty());
    }

    #[test]
    fn test_ryang_remove_ok() {
        let mut ryang = Ryang::default();
        assert!(ryang.parse("/foo/test.yang", "module test {}").is_ok());
        let result = ryang.remove("/foo/test.yang");
        assert!(result.is_ok_and(|uri| uri == "/foo/test.yang"));
    }

    #[test]
    fn test_ryang_remove_error() {
        let mut ryang = Ryang::default();
        let result = ryang.remove("/foo/nonexistent.yang");
        assert!(result.is_err());
    }

    #[test]
    fn test_ryang_list() {
        let mut ryang = Ryang::default();
        assert!(ryang.parse("/foo/test1.yang", "module test1 {}").is_ok());
        assert!(ryang.parse("/foo/test2.yang", "module test2 {}").is_ok());
        assert_eq!(ryang.list().len(), 2);
    }

    #[test]
    fn test_ryang_search() {
        let mut ryang = Ryang::default();
        assert!(ryang.parse("/foo/test1.yang", "module test1 {}").is_ok());
        assert!(ryang.parse("/foo/test2.yang", "module test2 {}").is_ok());
        let results = ryang.search("test1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].module_name(), Some("test1".to_owned()));
        let no_results = ryang.search("nonexistent");
        assert!(no_results.is_empty());
    }

    #[test]
    fn test_ryang_search1_by_revision() {
        let mut ryang = Ryang::default();
        assert!(
            ryang
                .parse(
                    "/foo/testrev1.yang",
                    "module testrev {\n  namespace \"urn:testrev\";\n  revision 2024-01-01;\n}"
                )
                .is_ok()
        );
        assert!(
            ryang
                .parse(
                    "/foo/testrev2.yang",
                    "module testrev {\n  namespace \"urn:testrev\";\n  revision 2023-01-01;\n}"
                )
                .is_ok()
        );
        let found = ryang.search1("testrev", "2024-01-01");
        assert!(found.is_some());
        assert_eq!(found.unwrap().module_name(), Some("testrev".to_owned()));
        let notfound = ryang.search1("testrev", "1999-01-01");
        assert!(notfound.is_none());
    }

    #[test]
    fn test_yang_module_name() {
        let mut ryang = Ryang::default();
        assert!(
            ryang
                .parse(
                    "/foo/mymodule.yang",
                    "module mymodule {\n  namespace \"urn:mymodule\";\n}"
                )
                .is_ok()
        );
        let module = ryang.list()[0];
        assert_eq!(module.module_name(), Some("mymodule".to_owned()));
    }

    #[test]
    fn test_yang_get_document_and_list_token_and_module_kind() {
        let mut ryang = Ryang::default();
        assert!(
            ryang
                .parse(
                    "/foo/sample.yang",
                    "module sample {\n  namespace \"urn:sample\";\n}",
                )
                .is_ok()
        );
        let module = ryang.list()[0];
        assert!(module.line_count() >= 1);

        match module.module_kind() {
            Some(ModuleKind::Module(_)) => {}
            _ => panic!("Unexpected module kind"),
        }
    }

    #[test]
    fn test_yang_search_narrowest_token() {
        let mut ryang = Ryang::default();
        assert!(
            ryang
                .parse("/foo/test.yang", "module test {\n  prefix t\n}")
                .is_ok()
        );
        let modules = ryang.list();
        let module = modules[0];
        // Assuming tokens are parsed, test search_narrowest_token
        // This might need adjustment based on actual tokenization
        let token = module.search_narrowest_token(0, 0);
        assert!(token.is_ok());
    }

    #[test]
    fn test_yang_search_token() {
        let mut ryang = Ryang::default();
        assert!(
            ryang
                .parse("/foo/test.yang", "module test {\n  prefix t;\n}")
                .is_ok()
        );
        let module = &ryang.list()[0];
        let statements = module.search_token(TokenKind::Keyword(StatementKind::Prefix));
        assert!(!statements.is_empty());
    }
}
