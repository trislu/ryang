//! Catalog-only ingestion (memory goal): a minimal per-document record with
//! just the header facts a resolver needs (name, revision, prefix, namespace,
//! imports, includes, parse status), used to index very large trees WITHOUT
//! retaining full parse views or source text. Full documents are re-parsed on
//! demand when a module is actually opened/queried (see docs/memory-findings.md).
//!
//! `Catalog::scan` parses the document (tree-sitter CST is transient and
//! dropped) and copies out the header fields. Only the strings on the
//! returned `Catalog` are retained.

use std::sync::Arc;

/// The light per-document catalog record.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Source document url.
    pub url: Arc<str>,
    /// Module or submodule name.
    pub name: String,
    /// Latest `revision` date, if present.
    pub revision: Option<String>,
    /// The module's own prefix (or, for a submodule, the belongs-to prefix).
    pub prefix: Option<String>,
    /// Imported `(module, prefix)` pairs.
    pub imports: Vec<(String, String)>,
    /// Included submodule names.
    pub includes: Vec<String>,
    /// True when the document parsed without a whole-file collapse.
    pub parse_ok: bool,
}

impl Catalog {
    /// Parse `source` and retain only the header facts. The parse is
    /// transient: nothing but the returned fields is kept alive.
    pub fn scan(url: impl Into<Arc<str>>, source: impl Into<String>) -> Catalog {
        let url = url.into();
        let source = source.into();
        let yang = crate::yang::Yang::new(url.clone(), source);
        let parse_ok = yang.parse_errors.is_empty();
        Catalog {
            url,
            name: yang.name.clone().unwrap_or_default(),
            revision: yang.revision.clone(),
            prefix: yang.own_prefix.clone(),
            imports: yang
                .imports
                .iter()
                .map(|i| (i.module.clone(), i.prefix.clone()))
                .collect(),
            includes: yang.includes.iter().map(|i| i.name.clone()).collect(),
            parse_ok,
        }
    }
}
