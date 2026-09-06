//! The compiler: turns parsed documents into effective `ModuleRecord`s and a
//! `Library`, reporting user-content problems as diagnostics (never `Err`,
//! [D3]).
//!
//! Pipeline (see `docs/architecture.md` §4):
//!   1. classify documents (module / submodule) and emit syntax diagnostics;
//!   2. attach submodules to their parent modules and detect import cycles
//!      (RFC 7950 §5.1 forbids circular chains of imports);
//!   3. scan symbols (grouping/typedef/identity) per module — order
//!      independent;
//!   4. expand each module's **effective tree** (groupings instantiated at
//!      `uses` sites, [D9]);
//!   5. apply cross-module `augment`/`deviation` targets;
//!   6. run the light validation pass (list keys, etc).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diag::{Diagnostic, DiagnosticCode, Location};
use crate::schema::{
    AppliedAugment, AppliedDeviation, DeviationOp, ExtensionDef, FeatureDef, Grouping, Identity,
    ImportInfo, ModuleRecord, NodeId, NodeKind, SchemaNode, SubmoduleRecord, Typedef,
};
use crate::syntax::{Statement, StatementKind};
use crate::value::TypeFacets;
use crate::yang::{UnitKind, Yang};

/// Result of a build: compiled modules + submodule records + diagnostics.
pub struct BuildOutcome {
    pub modules: Vec<ModuleRecord>,
    pub submodules: Vec<SubmoduleRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A grouping symbol discovered in symbol scan.
struct GroupSym<'a> {
    stmt: &'a Statement,
    file: &'a Yang,
}

/// A typedef's captured definition (owned; used to resolve type chains).
#[derive(Clone)]
struct TypedefDef {
    defining: Location,
    base: Option<String>,
    base_loc: Option<Location>,
    /// Facets written on the typedef's `type` statement (D31).
    facets: TypeFacets,
}

/// An identity's captured definition (owned; used to resolve derivation).
#[derive(Clone)]
struct IdentityDef {
    defining: Location,
    base: Option<String>,
    base_loc: Option<Location>,
}

/// An extension's captured definition.
#[derive(Clone)]
struct ExtensionDefDef {
    defining: Location,
    argument: Option<String>,
}

/// A feature's captured definition.
#[derive(Clone)]
struct FeatureDefDef {
    defining: Location,
}

/// Per-module symbol table.
struct SymTab<'a> {
    groupings: HashMap<String, GroupSym<'a>>,
    typedefs: HashMap<String, TypedefDef>,
    identities: HashMap<String, IdentityDef>,
    extensions: HashMap<String, ExtensionDefDef>,
    features: HashMap<String, FeatureDefDef>,
}

/// Immutable cross-module lookup tables shared by all expansion passes.
struct Index<'a> {
    /// Per module-instance symbol tables, keyed by instance url.
    syms: HashMap<String, SymTab<'a>>,
    /// prefix -> module name, per module INSTANCE url.
    pmaps_by_url: HashMap<String, HashMap<String, Arc<str>>>,
    /// prefix -> module name, per module name (canonical instance).
    prefix_maps: HashMap<String, HashMap<String, Arc<str>>>,
    /// module name -> index into `records` (canonical: highest revision).
    module_index: HashMap<String, usize>,
    /// module name -> canonical instance url.
    canon: HashMap<String, String>,
}

/// The scope in which statements are being expanded.
struct Scope<'a> {
    /// Module used to resolve unprefixed names and attributed as the
    /// `origin_module` (defining module) of nodes created here.
    module: Arc<str>,
    /// Module whose **namespace** owns nodes created here in instance data
    /// (`instance_module`). Equal to `module` everywhere except while expanding
    /// a grouping body via `uses`, where `module` switches to the grouping's
    /// defining module but `ns` keeps the *using* module (RFC 7950 §7.13).
    ns: Arc<str>,
    /// The physical document the statements being expanded belong to.
    file: &'a Yang,
}

type GroupKey = (Arc<str>, String);

/// Map over a slice, **preserving input order**, running in parallel when the
/// `parallel` cargo feature is on (a plain sequential map otherwise).
///
/// Both paths return results in the same order, so callers never observe
/// thread scheduling: downstream `records`/module ordering and diagnostics are
/// identical either way. The parallel path additionally needs the element /
/// closure / result to be `Sync`/`Send`; the sequential path needs none of
/// those bounds. `Repository::upsert_many_files` reuses this to read and parse
/// its file batch in parallel.
#[cfg(feature = "parallel")]
pub(crate) fn map_par<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    F: Fn(&T) -> R + Sync + Send,
    R: Send,
{
    use rayon::prelude::*;
    items.par_iter().map(f).collect()
}

/// Sequential fallback for [`map_par`] when the `parallel` feature is off.
#[cfg(not(feature = "parallel"))]
pub(crate) fn map_par<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    F: Fn(&T) -> R,
{
    items.iter().map(f).collect()
}

/// Run two closures, concurrently when the `parallel` feature is on.
#[cfg(feature = "parallel")]
fn join_par<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
where
    FA: FnOnce() -> A + Send,
    FB: FnOnce() -> B + Send,
    A: Send,
    B: Send,
{
    rayon::join(fa, fb)
}

/// Sequential fallback for [`join_par`].
#[cfg(not(feature = "parallel"))]
fn join_par<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
where
    FA: FnOnce() -> A,
    FB: FnOnce() -> B,
{
    (fa(), fb())
}

// ---------------------------------------------------------------------------
// Public entry
// ---------------------------------------------------------------------------

/// Fold a module's `include` (submodule) tree into `content`, depth-first.
///
/// RFC 7950 §5.2: a submodule may be included by several *siblings* (a diamond
/// include) — that is **not** a cycle, so a shared `*-base` submodule reached
/// a second time is folded once and silently skipped. A cycle only exists when
/// an include revisits a submodule already on the **current DFS path** (an
/// ancestor), which is reported as [`DiagnosticCode::IncludeCycle`].
#[allow(clippy::too_many_arguments)]
fn fold_submodules<'a>(
    doc: &'a Yang,
    parent_name: &str,
    sub_by_name: &HashMap<&str, Vec<&'a Yang>>,
    content: &mut Vec<&'a Yang>,
    folded_sub: &mut HashMap<String, String>,
    on_path: &mut Vec<&'a Yang>,
    done: &mut HashSet<String>,
    diags: &mut Vec<Diagnostic>,
) {
    for inc in &doc.includes {
        let candidates = sub_by_name
            .get(inc.name.as_str())
            .cloned()
            .unwrap_or_default();
        let matched = candidates.into_iter().find(|s| {
            s.belongs_to
                .as_ref()
                .map(|(p, _)| p == parent_name)
                .unwrap_or(false)
        });
        let Some(s) = matched else {
            let detail = if sub_by_name.contains_key(inc.name.as_str()) {
                "submodule belongs to a different module"
            } else {
                "no such submodule document is open"
            };
            diags.push(Diagnostic::error(
                Some(doc.url.clone()),
                Some(inc.range.clone()),
                DiagnosticCode::UnresolvedInclude,
                format!("include '{}': {detail}", inc.name),
            ));
            continue;
        };

        if on_path.iter().any(|d| d.url == s.url) {
            // Real cycle: `s` is an ancestor on the current include path.
            let pos = on_path.iter().position(|d| d.url == s.url).unwrap_or(0);
            let mut chain: Vec<String> = on_path[pos..]
                .iter()
                .filter_map(|d| d.name.clone())
                .collect();
            if let Some(name) = s.name.clone() {
                chain.push(name);
            }
            diags.push(Diagnostic::error(
                Some(doc.url.clone()),
                Some(inc.range.clone()),
                DiagnosticCode::IncludeCycle,
                format!("include cycle: {}", chain.join(" -> ")),
            ));
            continue;
        }

        if !done.insert(s.url.to_string()) {
            // Diamond include: already folded via another sibling — legal, skip.
            continue;
        }
        folded_sub.insert(s.url.to_string(), parent_name.to_owned());
        content.push(s);
        on_path.push(s);
        fold_submodules(
            s,
            parent_name,
            sub_by_name,
            content,
            folded_sub,
            on_path,
            done,
            diags,
        );
        on_path.pop();
    }
}

/// True when the document's first non-whitespace character is `<` — i.e. the
/// file is HTML/XML (or similar markup), never a YANG document (YANG starts
/// with the `module`/`submodule` keyword, possibly after comments).
fn starts_with_angle(y: &Yang) -> bool {
    let src = y.source().strip_prefix('\u{feff}').unwrap_or(y.source());
    src.chars().find(|c| !c.is_whitespace()) == Some('<')
}

pub fn build(docs: &[&Yang]) -> BuildOutcome {
    let mut diags = Vec::new();

    // ---- 1. classify ----------------------------------------------------
    for y in docs {
        for e in &y.parse_errors {
            // A document whose first non-whitespace byte is '<' cannot be a
            // YANG module/submodule (those start with the `module`/`submodule`
            // keyword, possibly after comments). Such files (e.g. HTML/XML
            // saved as `*.yang`) are reported once as `not-a-yang-document`
            // below; the whole-file parse error adds only noise.
            if starts_with_angle(y) {
                continue;
            }
            diags.push(Diagnostic::error(
                Some(y.url.clone()),
                Some(e.range.clone()),
                DiagnosticCode::ParseError,
                e.message.clone(),
            ));
        }
    }

    let mut module_docs: Vec<&Yang> = Vec::new();
    let mut sub_docs: Vec<&Yang> = Vec::new();
    for y in docs {
        match y.kind {
            Some(UnitKind::Module) if y.name.is_some() => module_docs.push(y),
            Some(UnitKind::Submodule) if y.name.is_some() => sub_docs.push(y),
            _ => {
                diags.push(Diagnostic::warning(
                    Some(y.url.clone()),
                    Some(0..y.source().len()),
                    DiagnosticCode::NotYangDocument,
                    "document does not contain a module or submodule",
                ));
            }
        }
    }

    // Deduplicate modules with the same (name, revision). Among equal copies
    // prefer one that parsed cleanly: a broken copy (whole-file parse
    // failure) must not shadow a healthy copy and drag down every module
    // that imports the name. Dropped copies are reported as warnings —
    // visible but non-blocking (like pyang, which dedupes silently).
    let mut best: HashMap<(String, Option<String>), usize> = HashMap::new();
    for (i, y) in module_docs.iter().enumerate() {
        let key = (y.name.clone().unwrap(), y.revision.clone());
        let mut warn_drop = |dropped: &Yang| {
            diags.push(Diagnostic::warning(
                Some(dropped.url.clone()),
                root_range(dropped),
                DiagnosticCode::DuplicateModule,
                format!(
                    "duplicate module '{}' (same name and revision); this copy is ignored",
                    key.0
                ),
            ));
        };
        match best.get(&key) {
            None => {
                best.insert(key, i);
            }
            Some(&j) => {
                let keep = module_docs[j];
                if !keep.parse_errors.is_empty() && y.parse_errors.is_empty() {
                    warn_drop(keep);
                    best.insert(key, i);
                } else {
                    warn_drop(y);
                }
            }
        }
    }
    let mut chosen: Vec<&Yang> = best.values().map(|&i| module_docs[i]).collect();
    // Deterministic order (urls are unique), preserving ingest independence.
    chosen.sort_by(|a, b| a.url.cmp(&b.url));
    let to_compile = chosen;

    let mut sub_by_name: HashMap<&str, Vec<&Yang>> = HashMap::new();
    for s in &sub_docs {
        sub_by_name
            .entry(s.name.as_ref().unwrap().as_str())
            .or_default()
            .push(s);
    }

    // ---- 2. attach submodules (include tree) ---------------------------
    let mut content_of: HashMap<String, Vec<&Yang>> = HashMap::new();
    let mut folded_sub: HashMap<String, String> = HashMap::new(); // submodule url -> parent module
    for m in &to_compile {
        let parent_name = m.name.clone().unwrap();
        let mut content: Vec<&Yang> = Vec::new();
        let mut on_path: Vec<&Yang> = Vec::new();
        let mut done: HashSet<String> = HashSet::new();
        fold_submodules(
            m,
            &parent_name,
            &sub_by_name,
            &mut content,
            &mut folded_sub,
            &mut on_path,
            &mut done,
            &mut diags,
        );
        content_of.insert(parent_name.clone(), content);
    }

    // Unresolved belongs-to: submodule whose parent module is not open.
    for s in &sub_docs {
        let parent = s.belongs_to.as_ref().map(|(p, _)| p.clone());
        let parent_open = parent
            .as_ref()
            .map(|p| {
                module_docs
                    .iter()
                    .any(|m| m.name.as_deref() == Some(p.as_str()))
            })
            .unwrap_or(false);
        if !parent_open {
            let range = s
                .root()
                .and_then(|r| r.find_one(StatementKind::BelongsTo))
                .map(|b| b.range.clone());
            diags.push(Diagnostic::error(
                Some(s.url.clone()),
                range,
                DiagnosticCode::UnresolvedBelongsTo,
                format!(
                    "submodule '{}' belongs-to '{}' but that module is not open",
                    s.name.clone().unwrap_or_default(),
                    parent.unwrap_or_default()
                ),
            ));
        }
    }

    // Import cycles are forbidden by RFC 7950 §5.1 — report each one.
    detect_import_cycles(&to_compile, &content_of, &mut diags);

    // ---- 3. symbol scan -------------------------------------------------
    let mut index = Index {
        syms: HashMap::new(),
        pmaps_by_url: HashMap::new(),
        prefix_maps: HashMap::new(),
        module_index: HashMap::new(),
        canon: HashMap::new(),
    };
    // `module_index` (name -> position in `to_compile` = future `records`
    // index) is filled up front: augment/deviation resolution consults it only
    // after every record exists, and `records` is built in `to_compile` order.
    for (i, m) in to_compile.iter().enumerate() {
        let n = m.name.clone().unwrap();
        let rev = m.revision.clone().unwrap_or_default();
        match index.module_index.get(&n) {
            Some(&j) => {
                let cur = to_compile[j].revision.clone().unwrap_or_default();
                if rev > cur {
                    index.module_index.insert(n, i);
                }
            }
            None => {
                index.module_index.insert(n, i);
            }
        }
    }

    // Symbol/prefix scan is independent per module — run in parallel when the
    // `parallel` feature is on. `map_par` preserves module order, so the
    // resulting tables and downstream diagnostics are unchanged.
    let scanned = map_par(&to_compile, |m| {
        let name = m.name.clone().unwrap();
        let content: Vec<&Yang> = std::iter::once(*m)
            .chain(content_of.get(&name).cloned().unwrap_or_default())
            .collect();

        // prefix map: own + belongs-to prefixes -> self; imports -> module
        let mut pmap: HashMap<String, Arc<str>> = HashMap::new();
        for doc in &content {
            if let Some(own) = &doc.own_prefix {
                pmap.insert(own.clone(), Arc::from(name.as_str()));
            }
            for imp in &doc.imports {
                pmap.insert(imp.prefix.clone(), Arc::from(imp.module.as_str()));
            }
        }

        // symbols
        let mut syms = SymTab {
            groupings: HashMap::new(),
            typedefs: HashMap::new(),
            identities: HashMap::new(),
            extensions: HashMap::new(),
            features: HashMap::new(),
        };
        for doc in &content {
            if let Some(root) = doc.root() {
                collect_symbols(root, doc, &name, &mut syms);
            }
        }
        (
            m.url.to_string(),
            name,
            m.revision.clone().unwrap_or_default(),
            pmap,
            syms,
        )
    });
    // Symbol tables are keyed per module INSTANCE (url): each revision of a
    // name keeps its own symbols. `prefix_maps` keeps ONE canonical instance
    // per name — the one with the highest revision — because all name-based
    // reference resolution below is canonical-latest.
    let mut canon: HashMap<String, (String, String)> = HashMap::new(); // name -> (rev, url)
    for (url, name, rev, pmap, syms) in scanned {
        index.syms.insert(url.clone(), syms);
        index.pmaps_by_url.insert(url.clone(), pmap);
        let better = match canon.get(&name) {
            None => true,
            Some((cur, _)) => rev > *cur,
        };
        if better {
            canon.insert(name, (rev, url));
        }
    }
    for (name, (_rev, url)) in canon {
        index.canon.insert(name.clone(), url.clone());
        index
            .prefix_maps
            .insert(name, index.pmaps_by_url[&url].clone());
    }

    // ---- 4+5. expand + apply augment/deviation --------------------------
    // Effective-tree expansion reads only `index`/`content_of` and is
    // independent per module — parallelized when the `parallel` feature is on,
    // with per-module diagnostics collected in module order so `records` and
    // diagnostics keep their existing ordering. The cross-module augment /
    // deviation fixpoint below stays sequential: it mutates the shared
    // `records` and one augment may target another augment's node (D17).
    let built = map_par(&to_compile, |m| {
        let name = m.name.clone().unwrap();
        let content: Vec<&Yang> = std::iter::once(*m)
            .chain(content_of.get(&name).cloned().unwrap_or_default())
            .collect();
        let mut local = Vec::new();
        let rec = build_module(&index, &content, &name, &mut local);
        (rec, local)
    });
    let mut records: Vec<ModuleRecord> = Vec::with_capacity(built.len());
    for (rec, mut local) in built {
        diags.append(&mut local);
        records.push(rec);
    }

    // Apply module-level augments after every base tree exists.
    let mut pending_augs: Vec<(Arc<str>, &Yang, &Statement)> = Vec::new();
    let mut pending_devs: Vec<(Arc<str>, &Yang, &Statement, DeviationOp)> = Vec::new();
    for m in &to_compile {
        let name = m.name.clone().unwrap();
        let content: Vec<&Yang> = std::iter::once(*m)
            .chain(content_of.get(&name).cloned().unwrap_or_default())
            .collect();
        for doc in &content {
            if let Some(root) = doc.root() {
                for stmt in &root.children {
                    if stmt.kind == StatementKind::Augment {
                        pending_augs.push((Arc::from(name.as_str()), doc, stmt));
                    } else if stmt.kind == StatementKind::Deviation {
                        let op = deviation_op(stmt);
                        if let Some(op) = op {
                            pending_devs.push((Arc::from(name.as_str()), doc, stmt, op));
                        }
                    }
                }
            }
        }
    }
    apply_augments(&index, &mut records, &pending_augs, &mut diags);
    apply_deviations(&index, &mut records, &pending_devs, &mut diags);

    // ---- 6. light validation -------------------------------------------
    // Both passes read the finished `records` and only report; run them
    // concurrently when the `parallel` feature is on and append in the
    // original order (lists first, then symbols) so diagnostics are unchanged.
    let (list_diags, sym_diags) =
        join_par(|| validate_lists(&records), || validate_symbols(&records));
    diags.extend(list_diags);
    diags.extend(sym_diags);

    // ---- submodule records ---------------------------------------------
    let mut submodules = Vec::new();
    for s in &sub_docs {
        let url = s.url.to_string();
        let parent_module = folded_sub.get(&url).cloned();
        submodules.push(SubmoduleRecord {
            name: s.name.clone().unwrap(),
            revision: s.revision.clone(),
            belongs_to: s.belongs_to.clone(),
            url: s.url.clone(),
            parent_module,
        });
    }

    BuildOutcome {
        modules: records,
        submodules,
        diagnostics: diags,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn root_range(y: &Yang) -> Option<std::ops::Range<usize>> {
    y.root().map(|r| r.range.clone()).or_else(|| {
        if y.source().is_empty() {
            None
        } else {
            Some(0..y.source().len())
        }
    })
}

fn collect_symbols<'a>(stmt: &'a Statement, file: &'a Yang, _module: &str, syms: &mut SymTab<'a>) {
    match stmt.kind {
        StatementKind::Grouping => {
            let name = stmt
                .arg
                .as_ref()
                .map(|a| a.name().to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                syms.groupings
                    .entry(name)
                    .or_insert_with(|| GroupSym { stmt, file });
            }
        }
        StatementKind::Typedef => {
            if let Some(a) = stmt.arg.as_ref() {
                let name = a.name();
                if !name.is_empty() {
                    let (base, base_loc) = child_arg(stmt, file, StatementKind::Type);
                    let facets = stmt
                        .find_one(StatementKind::Type)
                        .map(TypeFacets::from_type_stmt)
                        .unwrap_or_default();
                    syms.typedefs
                        .entry(name.to_string())
                        .or_insert_with(|| TypedefDef {
                            defining: Location {
                                url: file.url.clone(),
                                range: a.range.clone(),
                            },
                            base,
                            base_loc,
                            facets,
                        });
                }
            }
        }
        StatementKind::Identity => {
            if let Some(a) = stmt.arg.as_ref() {
                let name = a.name();
                if !name.is_empty() {
                    let (base, base_loc) = child_arg(stmt, file, StatementKind::Base);
                    syms.identities
                        .entry(name.to_string())
                        .or_insert_with(|| IdentityDef {
                            defining: Location {
                                url: file.url.clone(),
                                range: a.range.clone(),
                            },
                            base,
                            base_loc,
                        });
                }
            }
        }
        StatementKind::Extension => {
            if let Some(a) = stmt.arg.as_ref() {
                let name = a.name();
                if !name.is_empty() {
                    let argument = stmt
                        .find_one(StatementKind::Argument)
                        .and_then(|arg_stmt| arg_stmt.arg.as_ref())
                        .map(|aa| aa.name().to_string());
                    syms.extensions
                        .entry(name.to_string())
                        .or_insert_with(|| ExtensionDefDef {
                            defining: Location {
                                url: file.url.clone(),
                                range: a.range.clone(),
                            },
                            argument,
                        });
                }
            }
        }
        StatementKind::Feature => {
            if let Some(a) = stmt.arg.as_ref() {
                let name = a.name();
                if !name.is_empty() {
                    syms.features
                        .entry(name.to_string())
                        .or_insert_with(|| FeatureDefDef {
                            defining: Location {
                                url: file.url.clone(),
                                range: a.range.clone(),
                            },
                        });
                }
            }
        }
        _ => {}
    }
    for c in &stmt.children {
        collect_symbols(c, file, _module, syms);
    }
}

/// One import edge: `to` module name, plus the url and byte range of the
/// import statement that makes the edge.
type ImportEdge = (String, Arc<str>, std::ops::Range<usize>);

/// Detect circular chains of imports (RFC 7950 §5.1 forbids them) and report
/// each one at the import statement that closes the cycle.
fn detect_import_cycles(
    to_compile: &[&Yang],
    content_of: &HashMap<String, Vec<&Yang>>,
    diags: &mut Vec<Diagnostic>,
) {
    let compiled: HashSet<String> = to_compile.iter().filter_map(|m| m.name.clone()).collect();
    // from -> (to module, url + range of the import statement making the edge)
    let mut graph: HashMap<String, Vec<ImportEdge>> = HashMap::new();
    for m in to_compile {
        let Some(name) = m.name.clone() else { continue };
        let content: Vec<&Yang> = std::iter::once(*m)
            .chain(content_of.get(&name).cloned().unwrap_or_default())
            .collect();
        for doc in &content {
            for imp in &doc.imports {
                if compiled.contains(&imp.module) {
                    graph.entry(name.clone()).or_default().push((
                        imp.module.clone(),
                        doc.url.clone(),
                        imp.range.clone(),
                    ));
                }
            }
        }
    }

    let mut color: HashMap<String, u8> = compiled.iter().cloned().map(|n| (n, 0)).collect();
    let mut stack: Vec<String> = Vec::new();
    for m in to_compile {
        let Some(name) = m.name.clone() else { continue };
        if color.get(&name).copied().unwrap_or(0) == 0 {
            visit_import(&name, &graph, &mut color, &mut stack, diags);
        }
    }
}

/// DFS for `detect_import_cycles`; reports a diagnostic on each back edge.
fn visit_import(
    cur: &str,
    graph: &HashMap<String, Vec<ImportEdge>>,
    color: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
    diags: &mut Vec<Diagnostic>,
) {
    color.insert(cur.to_string(), 1);
    stack.push(cur.to_string());
    if let Some(edges) = graph.get(cur) {
        for (to, url, range) in edges {
            match color.get(to).copied().unwrap_or(0) {
                0 => visit_import(to, graph, color, stack, diags),
                1 => {
                    // `to` is already on the stack → a cycle exists.
                    let pos = stack.iter().position(|m| m == to).unwrap_or(0);
                    let mut cycle: Vec<&str> = stack[pos..].iter().map(|s| s.as_str()).collect();
                    cycle.push(to);
                    diags.push(Diagnostic::error(
                        Some(url.clone()),
                        Some(range.clone()),
                        DiagnosticCode::ImportCycle,
                        format!("import cycle: {}", cycle.join(" -> ")),
                    ));
                }
                _ => {}
            }
        }
    }
    stack.pop();
    color.insert(cur.to_string(), 2);
}

/// Return the argument text + `Location` of the first direct child statement
/// of `kind` (used to capture a typedef's `type` / an identity's `base`).
fn child_arg(
    stmt: &Statement,
    file: &Yang,
    kind: StatementKind,
) -> (Option<String>, Option<Location>) {
    for c in &stmt.children {
        if c.kind == kind
            && let Some(a) = c.arg.as_ref()
        {
            return (
                Some(a.name().to_string()),
                Some(Location {
                    url: file.url.clone(),
                    range: a.range.clone(),
                }),
            );
        }
    }
    (None, None)
}

fn node_kind(k: &StatementKind) -> Option<NodeKind> {
    use StatementKind as S;
    Some(match k {
        S::Container => NodeKind::Container,
        S::Leaf => NodeKind::Leaf,
        S::LeafList => NodeKind::LeafList,
        S::List => NodeKind::List,
        S::Choice => NodeKind::Choice,
        S::Case => NodeKind::Case,
        S::Anyxml => NodeKind::Anyxml,
        S::Anydata => NodeKind::Anydata,
        S::Rpc => NodeKind::Rpc,
        S::Action => NodeKind::Action,
        S::Notification => NodeKind::Notification,
        S::Input => NodeKind::Input,
        S::Output => NodeKind::Output,
        _ => return None,
    })
}

fn is_top_level_stmt(k: &StatementKind) -> bool {
    use StatementKind as S;
    matches!(
        k,
        S::Container
            | S::Leaf
            | S::LeafList
            | S::List
            | S::Choice
            | S::Anyxml
            | S::Anydata
            | S::Rpc
            | S::Notification
            | S::Action
            | S::Input
            | S::Output
    )
}

fn is_body_child(k: &StatementKind) -> bool {
    use StatementKind as S;
    matches!(
        k,
        S::Container
            | S::Leaf
            | S::LeafList
            | S::List
            | S::Choice
            | S::Case
            | S::Anyxml
            | S::Anydata
            | S::Rpc
            | S::Action
            | S::Notification
            | S::Input
            | S::Output
            | S::Uses
    )
}

fn location_of(file: &Yang, stmt: &Statement) -> Location {
    Location {
        url: file.url.clone(),
        range: stmt.range.clone(),
    }
}

/// Resolve `text` (maybe `prefix:name`) against a module's scope.
fn qualify(index: &Index, module: &str, text: &str) -> (Arc<str>, String) {
    match text.split_once(':') {
        Some((prefix, local)) => {
            let m = index
                .prefix_maps
                .get(module)
                .and_then(|pm| pm.get(prefix))
                .cloned()
                .unwrap_or_else(|| Arc::from(prefix));
            (m, local.to_string())
        }
        None => (Arc::from(module), text.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Module record construction
// ---------------------------------------------------------------------------

fn build_module(
    index: &Index,
    content: &[&Yang],
    name: &str,
    diags: &mut Vec<Diagnostic>,
) -> ModuleRecord {
    let m = content[0];
    let mut arena: Vec<SchemaNode> = Vec::new();
    let mut top: Vec<NodeId> = Vec::new();
    let mut stack: Vec<GroupKey> = Vec::new();

    for doc in content {
        // unresolved imports (of this module or folded submodules)
        for imp in &doc.imports {
            if !index.module_index.contains_key(imp.module.as_str()) {
                diags.push(Diagnostic::error(
                    Some(doc.url.clone()),
                    Some(imp.range.clone()),
                    DiagnosticCode::UnresolvedImport,
                    format!(
                        "module '{}' imports '{}' but it is not open",
                        name, imp.module
                    ),
                ));
            }
        }
        if let Some(root) = doc.root() {
            for stmt in &root.children {
                if is_top_level_stmt(&stmt.kind) {
                    let scope = Scope {
                        module: Arc::from(name),
                        ns: Arc::from(name),
                        file: doc,
                    };
                    let id = expand_node(index, &mut arena, diags, &scope, None, stmt, &mut stack);
                    if let Some(id) = id {
                        top.push(id);
                    }
                }
            }
        }
    }

    let mut rec = ModuleRecord {
        name: name.to_string(),
        revision: m.revision.clone(),
        namespace: m.namespace.clone(),
        prefix: m.own_prefix.clone(),
        source_urls: content.iter().map(|d| d.url.clone()).collect(),
        imports: Vec::new(),
        includes: Vec::new(),
        prefix_map: index
            .pmaps_by_url
            .get(m.url.as_ref())
            .map(|pm| pm.iter().map(|(k, v)| (k.clone(), v.to_string())).collect())
            .unwrap_or_default(),
        nodes: arena,
        top,
        groupings: Vec::new(),
        typedefs: Vec::new(),
        identities: Vec::new(),
        extensions: Vec::new(),
        features: Vec::new(),
        augments: Vec::new(),
        deviations: Vec::new(),
    };

    // imports as written on the module header only
    for imp in &m.imports {
        rec.imports.push(ImportInfo {
            module: imp.module.clone(),
            prefix: imp.prefix.clone(),
            revision: imp.revision.clone(),
        });
    }
    // includes (folded submodule names)
    for doc in &content[1..] {
        if let Some(n) = &doc.name {
            rec.includes.push(n.clone());
        }
    }

    // materialize symbols from THIS module instance's symbol table (url key)
    if let Some(syms) = index.syms.get(m.url.as_ref()) {
        for (gname, g) in &syms.groupings {
            rec.groupings.push(Grouping {
                name: gname.clone(),
                defining: Location {
                    url: g.file.url.clone(),
                    range: g.stmt.range.clone(),
                },
            });
        }
        for (tname, def) in &syms.typedefs {
            rec.typedefs.push(Typedef {
                name: tname.clone(),
                defining: def.defining.clone(),
                base: def.base.clone(),
                base_loc: def.base_loc.clone(),
                facets: def.facets.clone(),
            });
        }
        for (iname, def) in &syms.identities {
            rec.identities.push(Identity {
                name: iname.clone(),
                defining: def.defining.clone(),
                base: def.base.clone(),
                base_loc: def.base_loc.clone(),
            });
        }
        for (ename, def) in &syms.extensions {
            rec.extensions.push(ExtensionDef {
                name: ename.clone(),
                defining: def.defining.clone(),
                argument: def.argument.clone(),
            });
        }
        for (fname, def) in &syms.features {
            rec.features.push(FeatureDef {
                name: fname.clone(),
                defining: def.defining.clone(),
            });
        }
    }
    // deterministic ordering for the public API
    rec.groupings.sort_by(|a, b| a.name.cmp(&b.name));
    rec.typedefs.sort_by(|a, b| a.name.cmp(&b.name));
    rec.identities.sort_by(|a, b| a.name.cmp(&b.name));
    rec.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    rec.features.sort_by(|a, b| a.name.cmp(&b.name));

    mark_keys(&mut rec.nodes);
    rec
}

/// Create one node and recurse into its body.
fn expand_node(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    parent: Option<NodeId>,
    stmt: &Statement,
    stack: &mut Vec<GroupKey>,
) -> Option<NodeId> {
    let kind = node_kind(&stmt.kind)?;
    let name = match kind {
        // `input`/`output` are schema nodes referenced by those exact names in
        // augment/refine/deviation node-ids (RFC 7950 §7.14/§7.15).
        NodeKind::Input => "input".to_owned(),
        NodeKind::Output => "output".to_owned(),
        _ => stmt
            .arg
            .as_ref()
            .map(|a| a.name().to_string())
            .unwrap_or_default(),
    };

    let mut node = SchemaNode {
        kind,
        name,
        parent,
        children: Vec::new(),
        defining: location_of(scope.file, stmt),
        used_from: None,
        origin_module: scope.module.clone(),
        instance_module: scope.ns.clone(),
        config: None,
        mandatory: false,
        presence: None,
        default: None,
        status: None,
        ordered_by: None,
        min_elements: None,
        max_elements: None,
        keys: Vec::new(),
        is_key: false,
        type_name: None,
        facets: TypeFacets::default(),
        removed: false,
    };

    // light per-node properties
    for c in &stmt.children {
        match c.kind {
            StatementKind::Key => {
                if let Some(a) = c.arg.as_ref() {
                    node.keys = a
                        .logical
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                }
            }
            StatementKind::Config => {
                node.config = c.arg.as_ref().map(|a| a.name() == "true");
            }
            StatementKind::Mandatory => {
                node.mandatory = c.arg.as_ref().map(|a| a.name() == "true").unwrap_or(true);
            }
            StatementKind::Presence => {
                node.presence = c.arg.as_ref().map(|a| a.name().to_string());
            }
            StatementKind::Default => {
                node.default = c.arg.as_ref().map(|a| a.logical.clone());
            }
            StatementKind::Status => {
                node.status = c.arg.as_ref().map(|a| a.name().to_string());
            }
            StatementKind::MinElements => {
                node.min_elements = c.arg.as_ref().map(|a| a.name().to_string());
            }
            StatementKind::MaxElements => {
                node.max_elements = c.arg.as_ref().map(|a| a.name().to_string());
            }
            StatementKind::Type => {
                if let Some(a) = c.arg.as_ref() {
                    node.type_name = Some(a.name().to_string());
                    // D31: capture the leaf's own type-statement restrictions
                    // (a builtin `type string { length … }`, a direct `leafref`
                    // `path`, an inline `enumeration`/`bits`, …).
                    node.facets = TypeFacets::from_type_stmt(c);
                    // report unknown prefixes in type references
                    let t = a.name();
                    if let Some((prefix, _)) = t.split_once(':') {
                        let known = index
                            .prefix_maps
                            .get(scope.module.as_ref())
                            .map(|pm| pm.contains_key(prefix))
                            .unwrap_or(false);
                        if !known {
                            diags.push(Diagnostic::error(
                                Some(scope.file.url.clone()),
                                Some(a.range.clone()),
                                DiagnosticCode::UnresolvedPrefix,
                                format!("unresolved prefix '{prefix}' in type '{t}'"),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let id = arena.len() as NodeId;
    arena.push(node);

    let child_ids = match kind {
        NodeKind::Choice => expand_choice_body(index, arena, diags, scope, id, stmt, stack),
        NodeKind::Rpc | NodeKind::Action => {
            expand_rpc_action_body(index, arena, diags, scope, id, stmt, stack)
        }
        _ => expand_generic_body(index, arena, diags, scope, id, stmt, stack),
    };
    arena[id].children = child_ids;
    Some(id)
}

fn expand_generic_body(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    parent: NodeId,
    stmt: &Statement,
    stack: &mut Vec<GroupKey>,
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for c in &stmt.children {
        if !is_body_child(&c.kind) {
            continue;
        }
        if c.kind == StatementKind::Uses {
            let used = expand_uses(index, arena, diags, scope, parent, c, stack);
            ids.extend(used);
        } else if let Some(id) = expand_node(index, arena, diags, scope, Some(parent), c, stack) {
            ids.push(id);
        }
    }
    ids
}

/// Expand an `rpc`/`action` body into its canonical `input` and `output`
/// children, in that order.
///
/// RFC 7950 §7.14/§7.15: an RPC/action always has an `input` and an `output`
/// schema node — empty when the statement is absent. Augments commonly target
/// an (implicit) `input` (e.g. adding a `destination-address` leaf), so the
/// node must exist even when the module omits the block, and it must be
/// findable under the name `"input"`/`"output"`.
fn expand_rpc_action_body(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    parent: NodeId,
    stmt: &Statement,
    stack: &mut Vec<GroupKey>,
) -> Vec<NodeId> {
    let mut input_stmt: Option<&Statement> = None;
    let mut output_stmt: Option<&Statement> = None;
    for c in &stmt.children {
        match c.kind {
            StatementKind::Input => input_stmt = Some(c),
            StatementKind::Output => output_stmt = Some(c),
            _ => {}
        }
    }

    let mut ids = Vec::with_capacity(2);
    let parts: [(NodeKind, Option<&Statement>); 2] = [
        (NodeKind::Input, input_stmt),
        (NodeKind::Output, output_stmt),
    ];
    for (kind, part) in parts {
        let name = match kind {
            NodeKind::Input => "input",
            NodeKind::Output => "output",
            _ => unreachable!(),
        };
        match part {
            Some(s) => {
                if let Some(id) = expand_node(index, arena, diags, scope, Some(parent), s, stack) {
                    ids.push(id);
                }
            }
            None => {
                // Synthesize an empty input/output node; point its definition at
                // the enclosing rpc/action so goto still lands somewhere sane.
                let node = SchemaNode {
                    kind,
                    name: name.to_owned(),
                    parent: Some(parent),
                    children: Vec::new(),
                    defining: location_of(scope.file, stmt),
                    used_from: None,
                    origin_module: scope.module.clone(),
                    instance_module: scope.ns.clone(),
                    config: None,
                    mandatory: false,
                    presence: None,
                    default: None,
                    status: None,
                    ordered_by: None,
                    min_elements: None,
                    max_elements: None,
                    keys: Vec::new(),
                    is_key: false,
                    type_name: None,
                    facets: TypeFacets::default(),
                    removed: false,
                };
                let id = arena.len() as NodeId;
                arena.push(node);
                ids.push(id);
            }
        }
    }
    ids
}

fn expand_choice_body(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    parent: NodeId,
    stmt: &Statement,
    stack: &mut Vec<GroupKey>,
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for c in &stmt.children {
        match c.kind {
            StatementKind::Case => {
                if let Some(id) = expand_node(index, arena, diags, scope, Some(parent), c, stack) {
                    ids.push(id);
                }
            }
            StatementKind::Container
            | StatementKind::Leaf
            | StatementKind::LeafList
            | StatementKind::List
            | StatementKind::Anyxml
            | StatementKind::Anydata
            | StatementKind::Choice
            | StatementKind::Action
            | StatementKind::Notification => {
                // shorthand case: synthesize a `case` whose name is the node's name
                let cname = c
                    .arg
                    .as_ref()
                    .map(|a| a.name().to_string())
                    .unwrap_or_default();
                let case = SchemaNode {
                    kind: NodeKind::Case,
                    name: cname,
                    parent: Some(parent),
                    children: Vec::new(),
                    defining: location_of(scope.file, c),
                    used_from: None,
                    origin_module: scope.module.clone(),
                    instance_module: scope.ns.clone(),
                    config: None,
                    mandatory: false,
                    presence: None,
                    default: None,
                    status: None,
                    ordered_by: None,
                    min_elements: None,
                    max_elements: None,
                    keys: Vec::new(),
                    is_key: false,
                    type_name: None,
                    facets: TypeFacets::default(),
                    removed: false,
                };
                let case_id = arena.len() as NodeId;
                arena.push(case);
                let child = expand_node(index, arena, diags, scope, Some(case_id), c, stack);
                if let Some(child) = child {
                    arena[case_id].children = vec![child];
                }
                ids.push(case_id);
            }
            _ => {}
        }
    }
    ids
}

/// Expand a `uses` statement: instantiate the grouping's nodes under `parent`.
fn expand_uses(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    parent: NodeId,
    stmt: &Statement,
    stack: &mut Vec<GroupKey>,
) -> Vec<NodeId> {
    let Some(arg) = stmt.arg.as_ref() else {
        return Vec::new();
    };
    let (module, local) = qualify(index, scope.module.as_ref(), arg.name());
    // Resolve the grouping in the CANONICAL instance of the referenced module.
    let group = index
        .canon
        .get(module.as_ref())
        .and_then(|url| index.syms.get(url.as_str()))
        .and_then(|syms| syms.groupings.get(&local));

    let Some(group) = group else {
        diags.push(Diagnostic::error(
            Some(scope.file.url.clone()),
            Some(arg.range.clone()),
            DiagnosticCode::UnresolvedGrouping,
            format!("grouping '{local}' not found in module '{module}'"),
        ));
        return Vec::new();
    };

    let key = (module.clone(), local.clone());
    if stack.contains(&key) {
        diags.push(Diagnostic::error(
            Some(scope.file.url.clone()),
            Some(arg.range.clone()),
            DiagnosticCode::UnresolvedGrouping,
            format!("grouping '{local}' is (transitively) recursive"),
        ));
        return Vec::new();
    }
    stack.push(key);

    let inner_scope = Scope {
        module: module.clone(),
        ns: scope.ns.clone(),
        file: group.file,
    };
    let mut created = Vec::new();
    for c in &group.stmt.children {
        if !is_body_child(&c.kind) {
            continue;
        }
        if c.kind == StatementKind::Uses {
            created.extend(expand_uses(
                index,
                arena,
                diags,
                &inner_scope,
                parent,
                c,
                stack,
            ));
        } else if let Some(id) =
            expand_node(index, arena, diags, &inner_scope, Some(parent), c, stack)
        {
            created.push(id);
        }
    }
    stack.pop();

    // The nodes born from this `uses` carry the uses-site location.
    let uses_loc = location_of(scope.file, stmt);
    for id in &created {
        if let Some(n) = arena.get_mut(*id) {
            n.used_from = Some(uses_loc.clone());
        }
    }

    // refine / uses-augment children of this `uses`
    for c in &stmt.children {
        match c.kind {
            StatementKind::Refine => {
                apply_refine(index, arena, diags, scope, &created, c, stack);
            }
            StatementKind::UsesAugment => {
                apply_uses_augment(index, arena, diags, scope, &created, c, stack);
            }
            _ => {}
        }
    }

    created
}

fn find_node_at_path(
    index: &Index,
    records: &[ModuleRecord],
    source_module: &str,
    raw_path: &str,
) -> Option<(usize, NodeId)> {
    let segments: Vec<&str> = raw_path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    let (module, first_local) = qualify(index, source_module, segments[0]);
    let mi = index.module_index.get(module.as_ref()).copied()?;
    let rec = &records[mi];
    let start = rec
        .top
        .iter()
        .copied()
        .find(|&id| rec.nodes[id].name == first_local)?;
    let mut current = start;
    for seg in &segments[1..] {
        let local = seg.rsplit(':').next().unwrap_or(seg);
        let rec = &records[mi];
        let next = rec.nodes[current]
            .children
            .iter()
            .copied()
            .find(|&id| rec.nodes[id].name == *local && !rec.nodes[id].removed)?;
        current = next;
    }
    Some((mi, current))
}

fn apply_augments<'a>(
    index: &Index<'a>,
    records: &mut [ModuleRecord],
    pending: &[(Arc<str>, &'a Yang, &'a Statement)],
    diags: &mut Vec<Diagnostic>,
) {
    // One augment may target a node that *another* augment installs (e.g. an
    // augment chain across modules). Applying in document order therefore
    // depends on upsert order: an augment whose dependency has not run yet
    // would spuriously report `AugmentTargetNotFound`. Apply to a **fixpoint**
    // — keep passing over the not-yet-applied augments until a full pass adds
    // nothing — so resolution only depends on the final schema, never on order.
    let mut applied = vec![false; pending.len()];
    loop {
        let mut any = false;
        for (i, (source_module, file, stmt)) in pending.iter().enumerate() {
            if applied[i] {
                continue;
            }
            let Some(arg) = stmt.arg.as_ref() else {
                continue;
            };
            let path = arg.path();
            let Some((mi, target_node)) = find_node_at_path(index, records, source_module, &path)
            else {
                continue;
            };
            // Expand content in the *augmenting* module's scope.
            let scope = Scope {
                module: source_module.clone(),
                ns: source_module.clone(),
                file,
            };
            let mut stack = Vec::new();
            let rec = &mut records[mi];
            let created = expand_generic_body(
                index,
                &mut rec.nodes,
                diags,
                &scope,
                target_node,
                stmt,
                &mut stack,
            );
            rec.nodes[target_node]
                .children
                .extend(created.iter().copied());
            // Record on the source module.
            if let Some(src) = index
                .module_index
                .get(source_module.as_ref())
                .map(|&i| &mut records[i])
            {
                src.augments.push(AppliedAugment {
                    target: path.clone(),
                    target_node,
                    source_module: source_module.clone(),
                    defining: location_of(file, stmt),
                });
            }
            applied[i] = true;
            any = true;
        }
        if !any {
            break;
        }
    }

    // Whatever is still unresolved after the fixpoint genuinely has no target.
    for (i, (_source_module, file, stmt)) in pending.iter().enumerate() {
        if applied[i] {
            continue;
        }
        let Some(arg) = stmt.arg.as_ref() else {
            continue;
        };
        diags.push(Diagnostic::error(
            Some(file.url.clone()),
            Some(arg.range.clone()),
            DiagnosticCode::AugmentTargetNotFound,
            format!("augment target '{}' not found", arg.path()),
        ));
    }
}

fn deviation_op(stmt: &Statement) -> Option<DeviationOp> {
    for c in &stmt.children {
        match c.kind {
            StatementKind::DeviateAdd => return Some(DeviationOp::Add),
            StatementKind::DeviateDelete => return Some(DeviationOp::Delete),
            StatementKind::DeviateReplace => return Some(DeviationOp::Replace),
            StatementKind::DeviateNotSupported => return Some(DeviationOp::NotSupported),
            _ => {}
        }
    }
    None
}

/// Apply module-level `deviation` statements.
///
/// A deviation may target a node in a *different* (base) module, so — like
/// augments — the target is resolved by honoring the first path segment's
/// prefix → module and walking that module's **effective** tree
/// (`find_node_at_path`), not by scanning the deviating module's own tree.
/// Per the audit decision A2, only the target must resolve (so goto/hover on
/// the argument works); no `deviate` sub-statement semantics are modelled
/// beyond the existing `not-supported` removal.
fn apply_deviations<'a>(
    index: &Index<'a>,
    records: &mut [ModuleRecord],
    pending: &[(Arc<str>, &'a Yang, &'a Statement, DeviationOp)],
    diags: &mut Vec<Diagnostic>,
) {
    for (source_module, file, stmt, op) in pending {
        let op = *op;
        let Some(arg) = stmt.arg.as_ref() else {
            continue;
        };
        let path = arg.path();
        let Some((mi, node_id)) = find_node_at_path(index, records, source_module, &path) else {
            diags.push(Diagnostic::error(
                Some(file.url.clone()),
                Some(arg.range.clone()),
                DiagnosticCode::DeviationTargetNotFound,
                format!("deviation target '{path}' not found"),
            ));
            continue;
        };

        if op == DeviationOp::NotSupported {
            let rec = &mut records[mi];
            let node = &mut rec.nodes[node_id];
            node.removed = true;
            // detach from parent
            if let Some(parent) = node.parent {
                if let Some(p) = rec.nodes.get_mut(parent) {
                    p.children.retain(|&c| c != node_id);
                }
            } else {
                rec.top.retain(|&t| t != node_id);
            }
        }

        // Record on the deviating (source) module.
        if let Some(src) = index
            .module_index
            .get(source_module.as_ref())
            .map(|&i| &mut records[i])
        {
            src.deviations.push(AppliedDeviation {
                target: path.clone(),
                target_node: Some(node_id),
                op,
                defining: location_of(file, stmt),
            });
        }
    }
}

/// Mark the leaf children of a list that are its keys.
fn mark_keys(nodes: &mut [SchemaNode]) {
    // snapshot list ids + keys
    let list_keys: Vec<(NodeId, Vec<String>)> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::List && !n.keys.is_empty())
        .map(|(i, n)| (i as NodeId, n.keys.clone()))
        .collect();
    for (list_id, keys) in list_keys {
        let children = nodes[list_id].children.clone();
        for k in keys {
            for cid in &children {
                if nodes[*cid].kind == NodeKind::Leaf && nodes[*cid].name == k {
                    nodes[*cid].is_key = true;
                }
            }
        }
    }
}

fn push_symbol_err(
    at: Option<&Location>,
    fallback: &Location,
    code: DiagnosticCode,
    message: String,
    diags: &mut Vec<Diagnostic>,
) {
    let loc = at.unwrap_or(fallback);
    diags.push(Diagnostic::error(
        Some(loc.url.clone()),
        Some(loc.range.clone()),
        code,
        message,
    ));
}

/// Outcome of resolving a `[prefix:]name` against a scope module.
enum Resolve {
    PrefixUnknown,
    Module(String),
}

/// Resolve `text` (maybe `prefix:name`) against `scope`'s prefix map.
fn resolve_symbol_module(scope: &ModuleRecord, text: &str) -> Resolve {
    match text.split_once(':') {
        None => Resolve::Module(scope.name.clone()),
        Some((p, _)) => match scope.prefix_map.get(p) {
            Some(m) => Resolve::Module(m.clone()),
            None => Resolve::PrefixUnknown,
        },
    }
}

fn symbol_local(text: &str) -> &str {
    text.rsplit(':').next().unwrap_or(text)
}

/// Existence diagnostics for identity derivation and typedef chains
/// (no RFC 7950 restriction-subset semantics).
fn validate_symbols(records: &[ModuleRecord]) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        match by_name.get(r.name.as_str()) {
            None => {
                by_name.insert(r.name.as_str(), i);
            }
            Some(&j) => {
                let rev = r.revision.as_deref().unwrap_or("");
                let cur = records[j].revision.as_deref().unwrap_or("");
                if rev > cur {
                    by_name.insert(r.name.as_str(), i);
                }
            }
        }
    }

    let mut rev_idx: HashMap<(String, String), usize> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        rev_idx.insert((r.name.clone(), r.revision.clone().unwrap_or_default()), i);
    }
    // Resolve an import prefix to the record instance this module actually
    // pinned (import revision-date); fall back to canonical-latest.
    let pick = |rec: &ModuleRecord, module: &str, prefix: &str| -> Option<usize> {
        if let Some(imp) = rec
            .imports
            .iter()
            .find(|i| i.prefix == prefix && i.module == module)
        {
            if let Some(rv) = &imp.revision {
                let key = (module.to_string(), rv.clone());
                if let Some(&i2) = rev_idx.get(&key) {
                    return Some(i2);
                }
            }
        }
        by_name.get(module).copied()
    };

    // typedef -> its base type
    for rec in records {
        for t in &rec.typedefs {
            if let Some(base) = &t.base {
                if crate::schema::is_builtin_type(base) {
                    continue;
                }
                let at = t.base_loc.as_ref();
                let fb = &t.defining;
                match resolve_symbol_module(rec, base) {
                    Resolve::PrefixUnknown => push_symbol_err(
                        at,
                        fb,
                        DiagnosticCode::UnresolvedPrefix,
                        format!("typedef '{}': unknown prefix in base '{base}'", t.name),
                        &mut diags,
                    ),
                    Resolve::Module(m) => {
                        let found = by_name.get(m.as_str()).map(|&i| {
                            records[i]
                                .typedefs
                                .iter()
                                .any(|x| x.name == symbol_local(base))
                        });
                        if found == Some(false) {
                            push_symbol_err(
                                at,
                                fb,
                                DiagnosticCode::UnresolvedTypedef,
                                format!(
                                    "typedef '{}' is based on unknown type '{}'",
                                    t.name,
                                    symbol_local(base)
                                ),
                                &mut diags,
                            );
                        }
                    }
                }
            }
        }
    }

    // identity -> its base identity
    for rec in records {
        for id in &rec.identities {
            if let Some(base) = &id.base {
                let at = id.base_loc.as_ref();
                let fb = &id.defining;
                match resolve_symbol_module(rec, base) {
                    Resolve::PrefixUnknown => push_symbol_err(
                        at,
                        fb,
                        DiagnosticCode::UnresolvedPrefix,
                        format!("identity '{}': unknown prefix in base '{base}'", id.name),
                        &mut diags,
                    ),
                    Resolve::Module(m) => {
                        let pfx = base.split(':').next().unwrap_or(base);
                        let found = pick(rec, m.as_str(), pfx).map(|i| {
                            records[i]
                                .identities
                                .iter()
                                .any(|x| x.name == symbol_local(base))
                        });
                        if found == Some(false) {
                            push_symbol_err(
                                at,
                                fb,
                                DiagnosticCode::UnresolvedIdentity,
                                format!(
                                    "identity '{}' is based on unknown identity '{}'",
                                    id.name,
                                    symbol_local(base)
                                ),
                                &mut diags,
                            );
                        }
                    }
                }
            }
        }
    }

    // leaf / leaf-list type references (scope = the module owning the node)
    for rec in records {
        for n in &rec.nodes {
            if !matches!(n.kind, NodeKind::Leaf | NodeKind::LeafList) || n.removed {
                continue;
            }
            let Some(t) = n.type_name.as_deref() else {
                continue;
            };
            if crate::schema::is_builtin_type(t) {
                continue;
            }
            let Some(&si) = by_name.get(n.origin_module.as_ref()) else {
                continue;
            };
            let scope_rec = &records[si];
            let local = symbol_local(t);
            match resolve_symbol_module(scope_rec, t) {
                Resolve::PrefixUnknown => {}
                Resolve::Module(m) => {
                    let prefix = t.split(':').next().unwrap_or(t);
                    if let Some(i) = pick(scope_rec, m.as_str(), prefix)
                        && !records[i].typedefs.iter().any(|x| x.name == local)
                    {
                        push_symbol_err(
                            None,
                            &n.defining,
                            DiagnosticCode::UnresolvedTypedef,
                            format!("type '{t}' is not a builtin and no such typedef is in scope"),
                            &mut diags,
                        );
                    }
                }
            }
        }
    }
    diags
}

/// `/a/b/c` schema path of `id`, for readable diagnostics.
fn schema_path(nodes: &[SchemaNode], id: NodeId) -> String {
    let mut names: Vec<&str> = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        if !nodes[c].name.is_empty() {
            names.push(&nodes[c].name);
        }
        cur = nodes[c].parent;
    }
    names.reverse();
    if names.is_empty() {
        return "/".to_owned();
    }
    let mut s = String::new();
    for n in names {
        s.push('/');
        s.push_str(n);
    }
    s
}

/// Should a key-less `list` be exempt from the RFC `key` requirement?
///
/// - **A5** — `config` is inherited from the nearest ancestor that sets it, and
///   `config false` propagates down the subtree. A list that is `config false`
///   (itself or via any ancestor) may omit `key`.
/// - **A6** — a list born from a `grouping` is only judged when the grouping
///   itself pins an explicit `config` (on the list or an ancestor inside the
///   grouping). If the grouping leaves `config` to the uses-site, we cannot
///   tell there, so we do not flag it — the grouping author is responsible for
///   using it only in a `config false` tree.
fn keyless_list_exempt(nodes: &[SchemaNode], list_id: NodeId) -> bool {
    // Content of an `rpc`/`action` (`input`/`output`) or a `notification` is
    // NOT configuration: `key` is not required there, regardless of `config`
    // statements. (RFC 7950 §7.1: `config` applies to the configuration tree.)
    let mut cur = Some(list_id);
    while let Some(c) = cur {
        match nodes[c].kind {
            NodeKind::Rpc
            | NodeKind::Action
            | NodeKind::Notification
            | NodeKind::Input
            | NodeKind::Output => return true,
            _ => {}
        }
        cur = nodes[c].parent;
    }

    // Nearest grouping-instantiation root: an ancestor (or the list) carrying
    // `used_from` (i.e. born from a `uses`).
    let mut grouping_root: Option<NodeId> = None;
    let mut cur = Some(list_id);
    while let Some(c) = cur {
        if nodes[c].used_from.is_some() {
            grouping_root = Some(c);
            break;
        }
        cur = nodes[c].parent;
    }

    // First explicit `config` from the list up to (and including) the grouping
    // root — or to the tree root when the list is not grouping-born.
    let mut explicit: Option<bool> = None;
    let mut cur = Some(list_id);
    while let Some(c) = cur {
        if let Some(cfg) = nodes[c].config {
            explicit = Some(cfg);
            break;
        }
        if Some(c) == grouping_root {
            break;
        }
        cur = nodes[c].parent;
    }

    match (grouping_root, explicit) {
        // Grouping never pinned `config` → cannot judge here (A6).
        (Some(_), None) => true,
        // Explicit `config false` on the list or an ancestor → key-less is OK (A5).
        (_, Some(false)) => true,
        // Defined directly in a module with default `config true` → judge.
        (None, None) => false,
        // `config true` is explicit → judge.
        (_, Some(true)) => false,
    }
}

fn validate_lists(records: &[ModuleRecord]) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for rec in records {
        let lists: Vec<(NodeId, Vec<String>, Location, String, String)> = rec
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == NodeKind::List && !n.removed)
            .map(|(i, n)| {
                let id = i as NodeId;
                (
                    id,
                    n.keys.clone(),
                    n.defining.clone(),
                    n.name.clone(),
                    schema_path(&rec.nodes, id),
                )
            })
            .collect();
        for (list_id, keys, loc, name, path) in lists {
            let children: Vec<(NodeId, NodeKind, String)> = rec.nodes[list_id]
                .children
                .iter()
                .filter_map(|&cid| {
                    let n = &rec.nodes[cid];
                    if n.removed {
                        None
                    } else {
                        Some((cid, n.kind, n.name.clone()))
                    }
                })
                .collect();
            if keys.is_empty() {
                if !keyless_list_exempt(&rec.nodes, list_id) {
                    diags.push(Diagnostic::warning(
                        Some(loc.url.clone()),
                        Some(loc.range.clone()),
                        DiagnosticCode::ListWithoutKey,
                        format!("list '{name}' has no 'key' statement (path '{path}')"),
                    ));
                }
                continue;
            }
            for k in &keys {
                match children.iter().find(|(_, _, n)| n == k) {
                    None => diags.push(Diagnostic::error(
                        Some(loc.url.clone()),
                        Some(loc.range.clone()),
                        DiagnosticCode::KeyLeafNotFound,
                        format!("key leaf '{k}' is not a child of the list"),
                    )),
                    Some((_, kind, _)) => {
                        if *kind != NodeKind::Leaf {
                            diags.push(Diagnostic::error(
                                Some(loc.url.clone()),
                                Some(loc.range.clone()),
                                DiagnosticCode::InvalidKey,
                                format!("key '{k}' must reference a leaf"),
                            ));
                        }
                    }
                }
            }
        }
    }
    diags
}

// ---------------------------------------------------------------------------
// refine / uses-augment (best effort)
// ---------------------------------------------------------------------------

fn apply_refine(
    index: &Index,
    arena: &mut [SchemaNode],
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    created: &[NodeId],
    refine: &Statement,
    _stack: &mut Vec<GroupKey>,
) {
    let Some(arg) = refine.arg.as_ref() else {
        return;
    };
    let path = arg.path();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(target) = find_in_created(arena, created, &segments) else {
        diags.push(Diagnostic::error(
            Some(scope.file.url.clone()),
            Some(arg.range.clone()),
            DiagnosticCode::AugmentTargetNotFound,
            format!(
                "refine target '{}' not found in grouping instance",
                arg.name()
            ),
        ));
        return;
    };
    let node = &mut arena[target];
    for c in &refine.children {
        match c.kind {
            StatementKind::Default => node.default = c.arg.as_ref().map(|a| a.logical.clone()),
            StatementKind::Mandatory => {
                node.mandatory = c.arg.as_ref().map(|a| a.name() == "true").unwrap_or(true)
            }
            StatementKind::Presence => node.presence = c.arg.as_ref().map(|a| a.name().to_string()),
            StatementKind::Config => node.config = c.arg.as_ref().map(|a| a.name() == "true"),
            StatementKind::MinElements => {
                node.min_elements = c.arg.as_ref().map(|a| a.name().to_string())
            }
            StatementKind::MaxElements => {
                node.max_elements = c.arg.as_ref().map(|a| a.name().to_string())
            }
            _ => {}
        }
    }
    let _ = index;
}

fn apply_uses_augment(
    index: &Index,
    arena: &mut Vec<SchemaNode>,
    diags: &mut Vec<Diagnostic>,
    scope: &Scope,
    created: &[NodeId],
    augment: &Statement,
    stack: &mut Vec<GroupKey>,
) {
    let Some(arg) = augment.arg.as_ref() else {
        return;
    };
    let path = arg.path();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(target) = find_in_created(arena, created, &segments) else {
        diags.push(Diagnostic::error(
            Some(scope.file.url.clone()),
            Some(arg.range.clone()),
            DiagnosticCode::AugmentTargetNotFound,
            format!("uses-augment target '{}' not found", arg.name()),
        ));
        return;
    };
    let child_ids = expand_generic_body(index, arena, diags, scope, target, augment, stack);
    arena[target].children.extend(child_ids);
}

/// Find a node among the just-created subtree by a descendant path.
fn find_in_created(arena: &[SchemaNode], roots: &[NodeId], segments: &[&str]) -> Option<NodeId> {
    if segments.is_empty() {
        return None;
    }
    let first = segments[0].rsplit(':').next().unwrap_or(segments[0]);
    let mut current = roots.iter().copied().find(|&id| arena[id].name == first)?;
    for seg in &segments[1..] {
        let name = seg.rsplit(':').next().unwrap_or(seg);
        let children: Vec<NodeId> = arena[current].children.clone();
        current = children.into_iter().find(|&id| arena[id].name == name)?;
    }
    Some(current)
}
