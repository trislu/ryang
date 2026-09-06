# yrepo — repository instructions

`yrepo` is a library crate that parses + resolves YANG (`*.yang`). Public
surface: `Repository` (`upsert` / `upsert_many_files` / `remove` / `compile`)
and the `Library` snapshot.

## Build & test

- Test **both** feature modes: `cargo test` and `cargo test --features parallel`.
- Keep `cargo fmt` and `clippy` clean.
- The `parallel` cargo feature gates rayon (bulk file ingest via
  `upsert_many_files`, per-module compile phases). Off by default to keep the
  published dependency tree lean.

## Dev/audit tools live in `examples/`

`examples/{inspect,probe,perf}.rs` are yrepo-only tools. Build and run them
**with the `parallel` feature** for representative numbers:

```
cargo build --release --features parallel --examples
target/release/examples/inspect <dir>          # whole-tree diagnostic histogram
target/release/examples/probe <file…>          # per-file diagnostics + context
target/release/examples/perf [--repeat N] [--csv f.csv] <dir…>  # time + CPU + RSS
```

## Bench baseline

Corpus: an explicit input — the workflow never assumes where it lives. Pass
`--corpus DIR` to `scripts/audit.sh` or set `YANG_CORPUS` (see the workspace
root `AGENTS.md` for the machine-local example). Baselines below are for the
YangModels `yang` tree (**2143 files / ~30.5 MB**), release build, `parallel`
feature (2026-09-06):

| tool | result |
| --- | --- |
| `inspect` | ~0.45–0.6 s; **1975** diagnostics (1639 errors / 336 warnings) with the local tree-sitter-yang grammar fixes (all unpublished; released 0.3.0 grammar: 2984) — top: augment-target-not-found 829, unresolved-grouping 317, duplicate-module 278 (warnings), unresolved-typedef 193, parse-error 100, unresolved-import 85, unresolved-identity 85, not-a-yang-document 58, unresolved-prefix 18, key-leaf-not-found 9 |
| `perf` (single) | ~0.45–0.65 s wall / ~3.8–4.5 s CPU; RSS ~3 MB → **~721 MB peak** (VmHWM) |

Treat these as a regression guard: if parse/compile/retention changes move time
or RSS significantly, re-check with `perf` and keep the CSV. Peak RSS is driven
by per-document tree-sitter CST retention — the current biggest lever.

## Conventions

- Diagnostics: files whose first non-whitespace char is `<` (HTML/XML mislabeled
  `*.yang`) are reported once as `not-a-yang-document` — their meaningless
  whole-file `parse-error` is suppressed (regression test `016_html_not_yang.rs`).
  Duplicate modules with the same (name, revision) are reported as **warnings**
  (visible, non-blocking — the later copy is ignored); among equal copies a
  parse-clean copy wins over a broken one (`017_duplicate_prefers_clean.rs`);
  modules with different revisions coexist. `inspect` prints `errors:` /
  `warnings:` counts and a `whole-file parse collapses:` count (parse-error
  covering >=95% of a document — the PHASE 0 error-localization metric) in
  addition to the by-code histogram.
- Report user-content problems as **diagnostics**, never `Err`.
- Versioning: public-API changes → bump version + CHANGELOG. A change is only
  "breaking" if it affects something that actually shipped in a published
  version (never-released APIs can be dropped freely).
- Parser behavior changes belong in `tree-sitter-yang`: fix `grammar.js` there
  and regenerate (use the `parser-regen` skill; never hand-edit `parser.c`),
  then patch `yrepo` to the fixed grammar and bump the dependency.
- Name-based (prefixed) references — unversioned imports, typedef/identity
  bases, prefixed `type` refs — resolve to the HIGHEST revision of a module
  (canonical-latest); an import pinned with `revision-date` resolves to that
  exact revision first. Each revision keeps its own symbol table. Internal
  (unprefixed) references resolve against the owning module instance's OWN
  table only — a non-canonical revision is validated on its own terms and
  never sees another revision's typedefs (`021_typedef_own_revision.rs`).
  Augment/deviation target paths are resolved from the DECLARING instance's
  prefix map and import pins, so a pinned import augments the pinned
  revision's tree (`022_augment_pinned_revision.rs`).
- The local `[patch.crates-io]` override pointing `tree-sitter-yang` at
  `../tree-sitter-yang` is a TEMPORARY dev convenience — never commit it;
  remove it once the grammar fixes are version-bumped and published, then
  switch `yrepo` to the released `tree-sitter-yang` version.
- Sync README / CHANGELOG / docs/architecture.md whenever behavior or the API
  changes.
- Grammar fixes in progress (unpublished; released 0.3.0 grammar: 2984):
  `max-elements unbounded;` parses again (`038_max_elements.rs`), unknown/vendor
  extension statements accept bare symbol arguments such as `m^-X`
  (`039_vendor_symbol_arg.rs`), the `units` statement accepts bare symbol
  arguments such as `meter^2.second-1` (`040_units_symbol_arg.rs`; cleared all
  IEEE 1906.1 modules), `enum` names accept bare strings with symbols such as
  `n+1` (`041_enum_bare_symbol_name.rs`; cleared coms-core, routing-types,
  igmp-mld, iana-* registry modules), the `default` statement accepts bare
  symbol arguments such as `00:00:15.0` or `syslogtypes:local7`
  (`042_default_symbol_arg.rs`; cleared ietf-netconf-time, ietf-syslog), and
  double-quoted strings accept arbitrary backslash escapes (`\*`, `\S`, `\.`,
  `043_escape_sequences.rs`; cleared ietf-netconf-acm, ietf-ipfix-psamp,
  DRAFT ietf-isis). All are wired into yrepo via the local `[patch.crates-io]`
  override — keep that patch until the fixes are version-bumped/published.
  Residual corpus parse-errors (~100) are remaining grammar gaps / invalid
  MIB transcripts, mostly in `experimental/ietf-extracted-YANG-modules` — see
  the `issue-hunter` skill.

## Workflow skills

- `issue-hunter` — audit/report over a YANG tree; ground every claim in a run
  of `examples/{inspect,probe,perf}` or `scripts/audit.sh`.
- `audit-and-fix` — the full audit → categorize → fix → re-audit loop;
  `scripts/audit.sh [--corpus DIR]` is the deterministic runner
  (report at `target/audit/report.txt`, delta in `target/audit/prev.txt`).
