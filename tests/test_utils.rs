//! Shared test utilities for the `yrepo` integration tests.
#![allow(dead_code)]
use std::fs;
use std::path::Path;
use std::sync::Arc;

use yrepo::{Diagnostic, Library, ModuleRecord, Repository, SchemaNode};

/// Directory containing sample YANG fixture files.
pub const SAMPLE_DIR: &str = "tests/sample_yang";

/// Build a path to a sample YANG file.
pub fn sample(name: &str) -> String {
    format!("{SAMPLE_DIR}/{name}")
}

/// Load all `.yang` files from a directory into a Repository.
pub fn load_dir(path: &str) -> Repository {
    let mut repo = Repository::new();
    let dir = Path::new(path);
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("yang") {
            let content = fs::read_to_string(&p).unwrap();
            repo.upsert(p.to_string_lossy().to_string(), content);
        }
    }
    repo
}

/// Load a single `.yang` fixture file into a Repository.
pub fn load_file(name: &str) -> Repository {
    let content = fs::read_to_string(sample(name)).unwrap();
    let mut repo = Repository::new();
    repo.upsert(sample(name), content);
    repo
}

/// Load several `.yang` fixtures into one Repository.
pub fn load_files(names: &[&str]) -> Repository {
    let mut repo = Repository::new();
    for name in names {
        let content = fs::read_to_string(sample(name)).unwrap();
        repo.upsert(sample(name), content);
    }
    repo
}

/// Compile and return the library (Arc), panicking if no module compiled.
pub fn library(repo: &Repository) -> (Arc<Library>, Vec<Diagnostic>) {
    let out = repo.compile();
    let lib = out.library.expect("expected a compiled library");
    (lib, out.diagnostics)
}

/// Find a top-level effective node of `module` by name.
pub fn top<'m>(module: &'m ModuleRecord, name: &str) -> Option<&'m SchemaNode> {
    module
        .top_nodes()
        .iter()
        .find_map(|&id| module.node(id).filter(|n| n.name() == name))
}

/// Find a direct effective child of `node` by name.
pub fn child<'m>(
    module: &'m ModuleRecord,
    node: &SchemaNode,
    name: &str,
) -> Option<&'m SchemaNode> {
    node.children()
        .iter()
        .find_map(|&id| module.node(id).filter(|n| n.name() == name))
}

/// Walk an effective path (`container` -> ... -> `leaf`) by name.
pub fn find<'m>(module: &'m ModuleRecord, path: &[&str]) -> Option<&'m SchemaNode> {
    let mut current: Option<&SchemaNode> = None;
    for (i, seg) in path.iter().enumerate() {
        current = if i == 0 {
            top(module, seg)
        } else {
            current.and_then(|n| child(module, n, seg))
        };
    }
    current
}

/// Names of the effective children of `node`.
pub fn child_names<'m>(module: &'m ModuleRecord, node: &SchemaNode) -> Vec<&'m str> {
    node.children()
        .iter()
        .filter_map(|&id| module.node(id))
        .map(|n| n.name())
        .collect()
}

/// True if any diagnostic has the given code.
pub fn has_code(diags: &[Diagnostic], code: yrepo::DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}
