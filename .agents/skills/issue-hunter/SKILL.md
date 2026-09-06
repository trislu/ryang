---
name: issue-hunter
description: "Audit a YANG module tree for yrepo problems: categorize yrepo diagnostics by root cause, hunt tree-sitter-yang parser/grammar bugs, profile ingest+compile time or memory (RSS). Trigger words: yrepo audit, yang tree, inspect, probe, perf, parse-error, whole-file collapse, unresolved-typedef cascade, memory footprint, max-elements, duplicate-module, not-a-yang-document."
---

# yrepo issue hunter

Find, categorize, and fix correctness and performance problems in `yrepo` (and
its grammar, `tree-sitter-yang`) by compiling real YANG corpora with the
project's own dev tools. Ground every claim in a run of those tools or a
minimal repro — never in speculation.

## Context / where the tools live

- **Paths are relative** — no absolute paths. `<grammar-repo>` = the sibling
  `tree-sitter-yang` checkout (`../tree-sitter-yang` from here); `<corpus>` = a
  YANG tree to audit, always passed explicitly — `scripts/audit.sh --corpus
  <corpus>`, the `<dir>` argument of `examples/inspect`, or the `YANG_CORPUS`
  environment variable. The workflow never assumes a corpus location relative
  to the repos.
- `yrepo` (this crate, currently 0.3.0) parses+resolves YANG. Batch ingest is
  `Repository::upsert_many_files((url, path)…)` (reads+parses in parallel with
  the `parallel` feature, one file in memory at a time); in-memory single-doc
  is `upsert`.
- The dev/audit tools live in **this repo's `examples/`** (`inspect`, `probe`,
  `perf`) and depend only on `yrepo`. Build them with the parallel feature for
  representative numbers — `cargo build --release --features parallel --examples`
  — then run from `target/release/examples/`:
  - `inspect <dir>` — whole-tree ingest+compile: per-code diagnostic counts,
    full-doc parse-error list, timing.
  - `probe <file…>` — per-file diagnostics with source context snippets.
  - `perf [--repeat N] [--csv out.csv] <dir…>` — wall/CPU time **and** RSS
    footprint (background sampler + VmHWM + CSV curve).
  (Without `--features parallel` the tools still run but ingest sequentially.)
- Grammar: `tree-sitter-yang` 0.3.0 comes from crates.io; the sibling
  `<grammar-repo>` checkout is the dev copy — fix `grammar.js` there,
  regenerate `src/parser.c` with `tree-sitter generate` (follow the
  `parser-regen` skill), verify through yrepo, and only bump versions/release
  when the user approves. `yrepo` already carries the local
  `[patch.crates-io]` override, so a local fix is live without extra wiring.
- Standard corpus to audit: `<corpus>` (2143 `.yang` files across 22 dirs,
  ~30 MB).

## Known baseline (2026-09-06)

- `inspect` whole tree, built against the **local grammar patches** (unpublished;
  released 0.3.0 grammar: 2984): **2721** diagnostics — unresolved-typedef 824,
  augment-target-not-found 879, unresolved-grouping 379, duplicate-module 277,
  parse-error 131, not-a-yang-document 90, unresolved-import 99,
  key-leaf-not-found 5, unresolved-prefix 18, unresolved-identity 15,
  unresolved-include 2, unresolved-belongs-to 2. (HTML/XML files mislabeled
  `*.yang` are reported once as `not-a-yang-document` without a spurious
  whole-file `parse-error`.)
- Grammar fixes (working tree, **unpublished** — no version bump/release without
  approval): (1) `max-elements unbounded;` parses again (`038_max_elements.rs`);
  (2) unknown/vendor extension statements accept any bare unquoted argument,
  e.g. `units m^-X` (`039_vendor_symbol_arg.rs`); (3) the `units` statement
  accepts bare symbol arguments such as `meter^2.second-1`
  (`040_units_symbol_arg.rs`); (4) `enum` names accept bare strings with symbols
  such as `n+1` (`041_enum_bare_symbol_name.rs`). Together they cleared the IEEE
  1906.1 modules, ietf-coms-core, ietf-routing-types, ietf-igmp-mld, iana-*
  registry modules, and other MIB/registry transcripts. The residual ~131
  parse-errors are remaining grammar gaps / invalid MIB transcripts, mostly in
  `experimental/ietf-extracted-YANG-modules`.
- Release `perf` single run on the whole tree: ~0.6 s wall, ~4.1 s CPU,
  RSS ~3 MB → ~718 MB peak (VmHWM) — high because `yrepo` retains full
  tree-sitter CST per document; a candidate improvement is to drop/compact the
  CST after building the statement/token views.

## Approach

1. **Run the tools first.** `inspect <tree>` for the code histogram and
   full-doc list; then `probe` the flagged files (whole-file parse-error set,
   plus one sample per top diagnostic code) to read actual messages.
2. **Bisect to a minimal repro.** When a file fails to parse, shrink it to the
   smallest module that still fails (add one construct at a time from a known
   good header). Always write repro files with real newlines — heredocs, or
   `echo` per line — never `printf '%s'` with literal `\n` in the payload (that
   silently creates invalid files). Confirm the construct is valid YANG (RFC
   7950 §14) before blaming the grammar; distinguish "invalid input, bad error
   UX" from "valid input the grammar rejects".
3. **Categorize.** Group issues with counts and a root-cause per group, e.g.:
   - grammar gaps in `tree-sitter-yang` (whole-file collapse instead of a
     localized error);
   - error-reporting/recovery quality in `yrepo` (one ERROR swallows the file →
     coarse `0..len` ranges + false `not-a-yang-document`);
   - cascading `unresolved-import/typedef/grouping/augment-target-not-found`
     from modules whose imports failed to parse;
   - `duplicate-module` policy on corpora with both `name.yang` and
     `name@rev.yang` (and copies across org dirs);
   - memory footprint (CST retention).
   For each: a concrete **lean/solution** (e.g. add `unbounded` to the
   `max-elements` grammar rule and regenerate; localize error spans; suppress
   unresolved-* that descend from a fatal import; dedupe by `(name, rev)`).
4. **Performance.** Use `perf` and report wall/CPU/files-per-s and RSS
   (baseline → peak sampled → VmHWM), plus the CSV curve when asked. Prefer
   release builds; note that `--repeat` inflates RSS via allocator retention
   and debug builds inflate time/RSS.
5. **Implement only when asked or clearly in scope** (default scope is
   audit-and-report). If fixing: edit `yrepo` and/or the local
   `tree-sitter-yang` grammar (regenerate `parser.c`, never hand-edit it)
   and/or the dev tools. Verify with `cargo test` in `yrepo` (default **and**
   `--features parallel`) and by re-running `inspect`/`probe` on the corpus.
   Sync docs (README/CHANGELOG/architecture) and bump versions when the public
   API or grammar changes; judge breaking changes against what actually
   shipped (never-released APIs are free to drop).

## Constraints

- DO NOT claim a module is invalid without checking the RFC grammar and,
  ideally, a second tool (pyang) for cross-checks.
- DO NOT report corpus noise (e.g. genuinely non-module `.yang` files, or
  dated+undated duplicates) as yrepo bugs without saying why.
- DO NOT hand-edit `parser.c`; change `grammar.js` and regenerate.
- DO NOT leave dead code or stale docs behind a fix (run `cargo fmt`/`clippy`
  and re-sync README/CHANGELOG/docs).
- DO NOT bump versions or publish until the user explicitly approves.
- DO NOT hardcode external corpus directory paths in written reports or
  artifacts: reference corpus files by corpus name (github.com/YangModels/yang)
  and basename (interactive probe output may keep full paths).
- ONLY work within the three repos named in the workspace `AGENTS.md` unless
  asked otherwise.

## Output Format

Return a categorized report:

```
## yrepo issues over <tree> (<date>)
Baseline: <n> files, <m> diagnostics in <t>s  (compare to known baseline)
### <Category>  — count
- Repro: <path or minimal module>
- Root cause: ...
- Lean/solution: ...
### Performance
- time: ...  cpu: ...  files/s: ...
- memory: base -> peak (VmHWM) ...  (CSV: <path>)
```
If you made fixes: list changed files, the tests you ran (both feature modes),
and any docs/version bumps.
