//! Whole-tree reference index (references beyond the open closure).
//!
//! The serving model (`docs/serving-large-trees.md`) keeps full statement
//! trees only for the *open closure*, so "find all references" to a symbol in
//! a library module (e.g. a typedef in `ietf-yang-types`) cannot see the
//! modules that merely *import* that module — they are never materialized.
//!
//! [`ReferenceIndex`] closes that gap the cheap way: instead of expanding the
//! data tree, it walks each on-disk document's statement tree **once** during
//! a catalog-style scan, records a compact occurrence per definition /
//! reference the reference engine cares about (definition arg, `type`/`uses`/
//! `base`/`if-feature` args), resolves each occurrence to its target module
//! (via the document's own prefix map), and drops the statement trees again.
//! The retained index answers a whole-tree references query in one pass over
//! the occurrences, with no repository expansion and no per-document source
//! or tree retention.
//!
//! Semantics mirror the editor-side reference engine exactly:
//! * definitions: `typedef` / `grouping` / `identity` / `feature` /
//!   `extension` arguments, owned by the module (or, inside a submodule, the
//!   `belongs-to` parent — the module whose namespace they live in);
//! * references: `type` / `uses` / `base` / `if-feature` arguments, resolved
//!   through the owning module's prefix map;
//! * builtin `type` names (`string`, `uint32`, …) are skipped — they can
//!   never name a workspace symbol.
//!
//! The index is a whole-batch builder: `scan_many_files…` parses one file
//! batch (in parallel under the `parallel` feature, like the catalog) and
//! resolves prefixes after the batch is known, so per-file prefix maps across
//! the tree are available before any occurrence is committed.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::syntax::StatementKind;
use crate::yang::{UnitKind, Yang};

/// One document: its canonical url, referenced by [`Occ::url`].
#[derive(Debug)]
struct Doc {
    url: Arc<str>,
}

/// One recorded occurrence in [`ReferenceIndex::occ`].
#[derive(Debug)]
struct Occ {
    /// Index into [`ReferenceIndex::docs`].
    url: u32,
    /// Index into [`ReferenceIndex::modules`] (the resolved target module).
    module: u32,
    /// Byte range of the occurrence in its source document.
    start: u32,
    end: u32,
    /// Local (unprefixed) symbol name.
    local: String,
    /// True for a definition occurrence (returned only when a query asks for
    /// the declaration), false for a reference occurrence.
    def: bool,
}

/// A whole-tree definition/reference index over a scanned file batch.
///
/// Built with [`ReferenceIndex::scan_many_files_with`]; queried with
/// [`ReferenceIndex::references`]. Retains only the interned urls, target
/// module names, and occurrences — no statement trees or source text survive
/// the scan.
#[derive(Debug, Default)]
pub struct ReferenceIndex {
    docs: Vec<Doc>,
    modules: Vec<String>,
    module_ids: HashMap<String, u32>,
    occ: Vec<Occ>,
}

impl ReferenceIndex {
    /// The number of documents indexed.
    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    /// The number of recorded occurrences (definitions + references).
    pub fn len(&self) -> usize {
        self.occ.len()
    }

    pub fn is_empty(&self) -> bool {
        self.occ.is_empty()
    }

    /// Read and index every file path in `paths` (a whole tree handed as one
    /// batch, mirroring [`CatalogIndex::scan_many_files_with`]): files are
    /// read *and* statement-walked off-thread when the `parallel` feature is
    /// on (a plain sequential loop otherwise); each worker keeps only its
    /// in-flight document, so scan memory stays flat however large the tree.
    /// `url_for` maps each path to its canonical url (unreadable or
    /// non-YANG files are skipped, never an error). Returns how many
    /// documents were indexed.
    ///
    /// The index is *not* incremental: a later call replaces the previous
    /// contents with the new batch.
    pub fn scan_many_files_with<I, P, F>(&mut self, paths: I, url_for: F) -> usize
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
        F: Fn(&Path) -> Option<String> + Send + Sync,
    {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| p.as_ref().to_path_buf())
            .collect();
        let raws: Vec<Option<RawDoc>> = crate::compile::map_par(&paths, |p| {
            let url = url_for(p)?;
            let text = std::fs::read_to_string(p).ok()?;
            scan_raw(Arc::from(url), &text)
        });
        let n = raws.iter().flatten().count();
        *self = build(raws);
        n
    }

    /// Like [`ReferenceIndex::scan_many_files_with`], but the entry url for
    /// each file is its path string.
    pub fn scan_many_files<I, P>(&mut self, paths: I) -> usize
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        self.scan_many_files_with(paths, |p| Some(p.to_string_lossy().to_string()))
    }

    /// Every reference — and, when `include_declaration` is set, the
    /// definition occurrence — of `local` owned by `module`, across the whole
    /// indexed tree. The returned `(url, byte range)` spans point at the
    /// source text of each occurrence (the argument of a `typedef`/`type`/
    /// `uses`/`base`/`if-feature` statement), matching the editor-side
    /// reference engine's hit shape.
    pub fn references(
        &self,
        module: &str,
        local: &str,
        include_declaration: bool,
    ) -> Vec<(Arc<str>, Range<usize>)> {
        let Some(&mid) = self.module_ids.get(module) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for o in &self.occ {
            if o.module != mid || o.local != local {
                continue;
            }
            if !include_declaration && o.def {
                continue;
            }
            out.push((
                self.docs[o.url as usize].url.clone(),
                o.start as usize..o.end as usize,
            ));
        }
        out
    }

    fn intern_module(&mut self, module: String) -> u32 {
        if let Some(&id) = self.module_ids.get(&module) {
            return id;
        }
        let id = self.modules.len() as u32;
        self.modules.push(module.clone());
        self.module_ids.insert(module, id);
        id
    }
}

/// One document's scan result, held only while the batch is being combined.
struct RawDoc {
    url: Arc<str>,
    /// True for a `module` document (a submodule contributes its parent
    /// module's symbols but does not itself define an importable module).
    is_module: bool,
    /// Effective module scope: the module's own name, or (for a submodule)
    /// the `belongs-to` parent module's name.
    scope: String,
    /// `(prefix, module)` entries this document contributes to its scope's
    /// import map (its own prefix plus its `import` statements).
    prefixes: Vec<(String, String)>,
    /// Raw occurrences; references keep their full (possibly prefixed) text.
    occ: Vec<RawOcc>,
}

/// A definition/reference occurrence before prefix resolution.
struct RawOcc {
    /// The argument text: a bare name for definitions, possibly `prefix:local`
    /// for references.
    text: String,
    start: usize,
    end: usize,
    def: bool,
}

fn is_def_kind(k: &StatementKind) -> bool {
    use StatementKind as K;
    matches!(
        k,
        K::Typedef | K::Grouping | K::Identity | K::Feature | K::Extension
    )
}

fn is_ref_kind(k: &StatementKind) -> bool {
    use StatementKind as K;
    matches!(k, K::Type | K::Uses | K::Base | K::IfFeature)
}

fn split_ref(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, name),
    }
}

fn scan_raw(url: Arc<str>, text: &str) -> Option<RawDoc> {
    let yang = Yang::new(url.clone(), text.to_owned());
    let doc_name = yang.name.as_deref()?;
    let kind = yang.kind?;
    let is_module = kind == UnitKind::Module;
    let scope = match kind {
        UnitKind::Module => doc_name.to_owned(),
        UnitKind::Submodule => yang.belongs_to.as_ref()?.0.clone(),
    };
    let root = yang.root()?;

    let mut prefixes = Vec::new();
    if let Some(own) = yang.own_prefix.as_deref()
        && !own.is_empty()
    {
        prefixes.push((own.to_owned(), scope.clone()));
    }
    for imp in &yang.imports {
        if !imp.prefix.is_empty() {
            prefixes.push((imp.prefix.clone(), imp.module.clone()));
        }
    }

    let mut occ = Vec::new();
    for stmt in root.preorder() {
        if is_def_kind(&stmt.kind) {
            if let Some(a) = &stmt.arg {
                let name = a.name();
                if !name.is_empty() {
                    occ.push(RawOcc {
                        text: name.to_owned(),
                        start: a.range.start,
                        end: a.range.end,
                        def: true,
                    });
                }
            }
            continue;
        }
        if !is_ref_kind(&stmt.kind) {
            continue;
        }
        let Some(a) = &stmt.arg else { continue };
        let name = a.name();
        if name.is_empty() {
            continue;
        }
        // Builtin `type` names never name a workspace symbol (mirrors the
        // editor-side reference engine).
        let (_, local) = split_ref(name);
        if stmt.kind == StatementKind::Type && crate::schema::is_builtin_type(local) {
            continue;
        }
        occ.push(RawOcc {
            text: name.to_owned(),
            start: a.range.start,
            end: a.range.end,
            def: false,
        });
    }

    Some(RawDoc {
        url,
        is_module,
        scope,
        prefixes,
        occ,
    })
}

/// Combine a scanned batch into the final index: per-scope prefix maps first,
/// then every occurrence resolved to its target module.
fn build(mut raws: Vec<Option<RawDoc>>) -> ReferenceIndex {
    let mut scope_maps: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut defined_modules: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw in raws.iter().flatten() {
        if raw.is_module {
            defined_modules.insert(raw.scope.clone());
        }
        let map = scope_maps.entry(raw.scope.clone()).or_default();
        for (prefix, module) in &raw.prefixes {
            map.entry(prefix.clone()).or_insert_with(|| module.clone());
        }
    }

    let mut ix = ReferenceIndex::default();
    for raw in raws.drain(..).flatten() {
        let url_id = ix.docs.len() as u32;
        ix.docs.push(Doc { url: raw.url });
        let map = scope_maps.get(&raw.scope);
        for o in raw.occ {
            let (prefix, local) = split_ref(&o.text);
            let target = if o.def {
                Some(raw.scope.clone())
            } else {
                match prefix {
                    None => Some(raw.scope.clone()),
                    Some(p) => map.and_then(|m| m.get(p).cloned()),
                }
            };
            let Some(target) = target else { continue };
            // A single document cannot exceed u32 byte space; guard anyway.
            if o.end > u32::MAX as usize {
                continue;
            }
            // Only keep occurrences whose target module is actually defined
            // in the batch (a dangling import resolves to nothing at compile
            // time; its references are not real).
            if !defined_modules.contains(&target) {
                continue;
            }
            let module = ix.intern_module(target);
            ix.occ.push(Occ {
                url: url_id,
                module,
                start: o.start as u32,
                end: o.end as u32,
                local: local.to_owned(),
                def: o.def,
            });
        }
    }
    ix
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(sources: &[(&str, &str)]) -> ReferenceIndex {
        let raws: Vec<Option<RawDoc>> = sources
            .iter()
            .map(|(url, src)| scan_raw(Arc::from(*url), src))
            .collect();
        build(raws)
    }

    fn refs(
        ix: &ReferenceIndex,
        module: &str,
        local: &str,
        decl: bool,
    ) -> Vec<(String, Range<usize>)> {
        ix.references(module, local, decl)
            .into_iter()
            .map(|(u, r)| (u.to_string(), r))
            .collect()
    }

    fn in_file<'a>(hits: &'a [(String, Range<usize>)], url: &str) -> Vec<&'a Range<usize>> {
        hits.iter()
            .filter(|(u, _)| u == url)
            .map(|(_, r)| r)
            .collect()
    }

    const A: &str = "module liba { namespace \"urn:a\"; prefix a;\n\
      typedef speed { type uint32; }\n\
      grouping gear { leaf g { type uint8; } }\n\
      identity mode;\n\
      feature turbo;\n\
      container c { leaf s { type speed; }\n\
      uses gear; leaf m { type identityref { base mode; } }\n\
      leaf f { if-feature turbo; type string; } }\n\
    }\n";
    const B: &str = "module libb { namespace \"urn:b\"; prefix b;\n\
      import liba { prefix a; }\n\
      typedef own { type a:speed; }\n\
      leaf s2 { type own; }\n\
      leaf s3 { type a:speed; }\n\
      container g2 { uses a:gear; }\n\
    }\n";
    // A second module that happens to define a typedef also named `speed`.
    const C: &str = "module libc { namespace \"urn:c\"; prefix c;\n\
      typedef speed { type int16; }\n\
      leaf s { type speed; }\n\
    }\n";

    #[test]
    fn cross_module_typedef_and_grouping_references() {
        let ix = index_of(&[("/a.yang", A), ("/b.yang", B), ("/c.yang", C)]);

        // `liba:speed`: declared in liba, used inside liba (leaf s) and twice
        // in libb (typedef own's a:speed + leaf s3's a:speed) — NOT libc's
        // local speed.
        let hits = refs(&ix, "liba", "speed", true);
        assert_eq!(
            in_file(&hits, "/a.yang").len(),
            2,
            "declaration + leaf s in liba"
        );
        assert_eq!(
            in_file(&hits, "/b.yang").len(),
            2,
            "two a:speed references in libb"
        );
        assert!(
            in_file(&hits, "/c.yang").is_empty(),
            "libc's speed is local"
        );

        // Without the declaration, only liba's own usage remains there.
        let hits = refs(&ix, "liba", "speed", false);
        assert_eq!(in_file(&hits, "/a.yang").len(), 1, "leaf s type speed");
        assert_eq!(in_file(&hits, "/b.yang").len(), 2);

        // Grouping gear: declaration + `uses gear` in liba, `uses a:gear` in
        // libb.
        let hits = refs(&ix, "liba", "gear", true);
        assert_eq!(in_file(&hits, "/a.yang").len(), 2, "decl + uses gear");
        assert_eq!(in_file(&hits, "/b.yang").len(), 1);
    }

    #[test]
    fn module_aware_matching_does_not_conflate_identities() {
        let ix = index_of(&[("/a.yang", A), ("/c.yang", C)]);
        // liba defines identity `mode`; libc defines none. Nothing false.
        assert!(
            refs(&ix, "liba", "mode", true)
                .iter()
                .all(|(u, _)| u == "/a.yang")
        );
    }

    #[test]
    fn builtin_type_arguments_are_not_indexed() {
        let ix = index_of(&[("/a.yang", A), ("/b.yang", B)]);
        // `string`, `uint32`, `uint8` are builtins and must produce no hits
        // under any module.
        assert!(refs(&ix, "liba", "string", true).is_empty());
        assert!(refs(&ix, "liba", "uint32", true).is_empty());
        assert!(
            !ix.occ
                .iter()
                .any(|o| o.local == "uint32" || o.local == "string"),
            "builtin type arguments must never be recorded"
        );
    }

    #[test]
    fn submodule_symbols_belong_to_the_parent_module() {
        let srcs = &[
            (
                "/m.yang",
                "module m { namespace \"urn:m\"; prefix m;\n  include m-sub;\n  leaf s { type speed; }\n}\n",
            ),
            (
                "/m-sub.yang",
                "submodule m-sub { belongs-to m { prefix m; }\n  typedef speed { type uint16; }\n}\n",
            ),
            (
                "/n.yang",
                "module n { namespace \"urn:n\"; prefix n;\n  import m { prefix m; }\n  leaf x { type m:speed; }\n}\n",
            ),
        ];
        let ix = index_of(srcs);
        // The typedef lives in m-sub but is owned by module m (its parent);
        // references from m (unprefixed) and n (m:speed) both resolve to m.
        let hits = refs(&ix, "m", "speed", true);
        assert_eq!(in_file(&hits, "/m-sub.yang").len(), 1, "declaration");
        assert_eq!(in_file(&hits, "/m.yang").len(), 1);
        assert_eq!(in_file(&hits, "/n.yang").len(), 1);
    }

    #[test]
    fn dangling_prefixes_are_dropped() {
        // libb imports ghost and references ghost:thing — no such module in
        // the tree, so the occurrence resolves to nothing and is dropped.
        let srcs = &[(
            "/g.yang",
            "module g { namespace \"urn:g\"; prefix g;\n  import ghost { prefix h; }\n  leaf x { type h:thing; }\n}\n",
        )];
        let ix = index_of(srcs);
        assert!(refs(&ix, "ghost", "thing", false).is_empty());
    }

    #[test]
    fn identity_base_and_feature_references() {
        // liba defines identity `mode` (used as an identityref base) and
        // feature `turbo` (used in an if-feature).
        let srcs = &[("/a.yang", A), ("/b.yang", B)];
        let ix = index_of(srcs);
        let hits = refs(&ix, "liba", "mode", true);
        assert_eq!(
            in_file(&hits, "/a.yang").len(),
            2,
            "identity decl + base in liba"
        );
        assert!(
            in_file(&hits, "/b.yang").is_empty(),
            "libb never references liba:mode"
        );
        let hits = refs(&ix, "liba", "turbo", true);
        assert_eq!(
            in_file(&hits, "/a.yang").len(),
            2,
            "feature decl + if-feature in liba"
        );
    }

    #[test]
    fn scan_many_files_builds_a_whole_batch_index() {
        use std::fs;
        use std::path::PathBuf;
        let dir = std::env::temp_dir().join(format!(
            "yrepo-refidx-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("types.yang");
        fs::write(
            &lib,
            "module types { namespace \"urn:types\"; prefix t;\n  typedef speed { type uint32; }\n}\n",
        )
        .unwrap();
        let user = dir.join("app.yang");
        fs::write(
            &user,
            "module app { namespace \"urn:app\"; prefix a;\n  import types { prefix t; }\n  leaf s { type t:speed; }\n}\n",
        )
        .unwrap();
        let files: Vec<PathBuf> = vec![lib.clone(), user.clone()];

        let mut ix = ReferenceIndex::default();
        let n = ix.scan_many_files_with(&files, |p| Some(format!("file://{}", p.display())));
        assert_eq!(n, 2);
        assert_eq!(ix.doc_count(), 2);
        assert!(!ix.is_empty());

        let hits = ix.references("types", "speed", true);
        let lib_url = format!("file://{}", lib.display());
        let user_url = format!("file://{}", user.display());
        assert_eq!(
            hits.iter().filter(|(u, _)| u.as_ref() == lib_url).count(),
            1,
            "typedef declaration"
        );
        assert_eq!(
            hits.iter().filter(|(u, _)| u.as_ref() == user_url).count(),
            1,
            "t:speed reference from the importing module"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
