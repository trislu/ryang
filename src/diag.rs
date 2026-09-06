//! Diagnostics model.
//!
//! Per [D3] user-content problems are **always** reported as `Diagnostic`s,
//! never as `Err`; `Result` is reserved for programmer/invariant errors.
//!
//! [D3]: https://github.com/ — see `docs/architecture.md` decision log.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// Severity of a diagnostic (mirrors the LSP severity order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        };
        f.write_str(s)
    }
}

/// Stable machine-readable code for a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticCode {
    // -- syntax ---------------------------------------------------------
    ParseError,
    // -- structure / resolution ----------------------------------------
    NotYangDocument,
    DuplicateModule,
    UnresolvedImport,
    UnresolvedInclude,
    UnresolvedBelongsTo,
    IncludeCycle,
    ImportCycle,
    UnresolvedPrefix,
    // -- symbols --------------------------------------------------------
    UnresolvedGrouping,
    UnresolvedTypedef,
    UnresolvedIdentity,
    // -- schema ---------------------------------------------------------
    AugmentTargetNotFound,
    DeviationTargetNotFound,
    KeyLeafNotFound,
    InvalidKey,
    ListWithoutKey,
    DuplicateNode,
    UnresolvedLeafref,
}

impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        use DiagnosticCode::*;
        match self {
            ParseError => "parse-error",
            NotYangDocument => "not-a-yang-document",
            DuplicateModule => "duplicate-module",
            UnresolvedImport => "unresolved-import",
            UnresolvedInclude => "unresolved-include",
            UnresolvedBelongsTo => "unresolved-belongs-to",
            IncludeCycle => "include-cycle",
            ImportCycle => "import-cycle",
            UnresolvedPrefix => "unresolved-prefix",
            UnresolvedGrouping => "unresolved-grouping",
            UnresolvedTypedef => "unresolved-typedef",
            UnresolvedIdentity => "unresolved-identity",
            AugmentTargetNotFound => "augment-target-not-found",
            DeviationTargetNotFound => "deviation-target-not-found",
            KeyLeafNotFound => "key-leaf-not-found",
            InvalidKey => "invalid-key",
            ListWithoutKey => "list-without-key",
            DuplicateNode => "duplicate-node",
            UnresolvedLeafref => "unresolved-leafref-path",
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A problem in a user document. Never fatal.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Document the problem belongs to.
    pub url: Option<Arc<str>>,
    /// Byte range in that document, when known.
    pub range: Option<Range<usize>>,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
}

impl Diagnostic {
    pub fn new(
        url: Option<Arc<str>>,
        range: Option<Range<usize>>,
        severity: Severity,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            url,
            range,
            severity,
            code,
            message: message.into(),
        }
    }

    pub fn error(
        url: Option<Arc<str>>,
        range: Option<Range<usize>>,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(url, range, Severity::Error, code, message)
    }

    pub fn warning(
        url: Option<Arc<str>>,
        range: Option<Range<usize>>,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(url, range, Severity::Warning, code, message)
    }
}

/// A position inside a physical document: url + byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub url: Arc<str>,
    pub range: Range<usize>,
}
