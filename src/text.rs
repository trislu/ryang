//! `Text`: raw source with line/offset access.
//!
//! Byte offsets are the canonical addressing unit used by the CST
//! (tree-sitter) and by every range/`Location`. Row/column conversions are
//! provided on demand for LSP-style queries.

use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
pub struct Text {
    source: Arc<str>,
    /// Byte offset of the start of each line (0-based lines).
    line_starts: Vec<usize>,
}

impl Text {
    pub(crate) fn new(source: Arc<str>) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Text {
            source,
            line_starts,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// (0-based row, char column) -> byte offset.
    pub fn linecol_to_byte(&self, row: usize, col: usize) -> Option<usize> {
        let start = *self.line_starts.get(row)?;
        let end = self
            .line_starts
            .get(row + 1)
            .copied()
            .unwrap_or(self.source.len());
        let mut byte = start;
        for (index, ch) in self.source[start..end].chars().enumerate() {
            if index == col {
                break;
            }
            byte += ch.len_utf8();
        }
        Some(byte)
    }

    pub fn slice(&self, range: Range<usize>) -> &str {
        let start = range.start.min(self.source.len());
        let end = range.end.min(self.source.len()).max(start);
        &self.source[start..end]
    }
}
