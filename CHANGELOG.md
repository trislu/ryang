# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Effective-tree queries for instance documents** (data path ≠ schema path):
  `ModuleRecord::data_children` / `data_child` (instance-visible children
  through `choice`/`case` wrappers), `rpc_input`/`rpc_output` (always present),
  `SchemaNode::instance_module` (the module whose **namespace** owns a node —
  equals `origin_module` except for grouping-born nodes via `uses`), and
  `Library::modules_by_namespace` / `Library::schema_nodeid` (canonical
  wrapper-inclusive nodeid, instance-module prefixes).
- **Leaf value typing** (`src/value.rs`): `TypeFacets` captured from each
  `type` statement (leaf and typedef), and `Library::value_type` which reduces a
  leaf/leaf-list type through the typedef chain to a `ValueType` — a scalar
  builtin (`String`/`Integer`/`Decimal64`/`Boolean`/`Empty`/`Binary`/
  `Enumeration`/`Bits`) with the facets accumulated along the chain, or
  `Leafref`/`Identityref`/`InstanceIdentifier`/`Union`/`Unknown`.
- **Semantic `identityref` check**: `Library::check_identityref(module, base,
  value) -> IdentityStatus` (`Ok`/`UnknownIdentity`/`NotDerived`) — the value
  must name an existing identity that is the `base` or derived from it.

## [0.1.0] - 2026-09-05

Initial public release of `yrepo`, an LSP-friendly YANG schema toolkit
(parse + resolved semantic model) for `*.yang` documents.

### Added

- **Document management & compile**: `Repository::upsert` / `remove` /
  `contains` / `len` by `url`, and whole-workspace `Repository::compile` that
  returns diagnostics plus a snapshot `Library` (`Arc`), so each edit yields a
  consistent resolved view of the workspace.
- **Diagnostics, never `Err`** for user content: parse recovery plus
  unresolved import/include/belongs-to, import & include cycles, unresolved
  type/grouping/identity/prefix, and list-`key` validation.
- **Semantic model & resolution**:
  - modules (submodules folded in, each node keeping its defining file) with
    latest/exact `(name, revision)` lookup;
  - typedef chains resolved to builtins, identity derivation and
    identityref value sets, extension/feature definitions, prefix→module lookup;
  - effective/expanded schema tree: groupings instantiated per `uses`,
    shorthand `case`s materialized, `rpc`/`action` `input`/`output` always
    present, and cross-module `augment`/`deviation` applied
    order-independently.
- **Syntax view for LSP work**: a tree-sitter-free statement tree
  (`statement` / `statement_at`), comments, and the grammar-precise raw token
  stream for semantic tokens/highlighting.
- **Completion candidates**: builtin + local/imported typedefs, and
  identity candidates.

### Notes

- `*.yin` (XML), XPath/leafref evaluation, and RFC 7950 restriction-subset
  semantics are not yet implemented.
