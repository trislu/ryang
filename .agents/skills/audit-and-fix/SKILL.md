---
name: audit-and-fix
description: "Run the yrepo audit-and-fix loop: run scripts/audit.sh over a YANG corpus, read the diagnostics/perf report, categorize remaining issues, fix a grammar gap in tree-sitter-yang, and re-audit until converged. Trigger words: run audit, audit-and-fix, yrepo loop, parse-error cluster, reduce diagnostics, memory regression."
---

# yrepo audit-and-fix runbook

Parameters (substitute before running):
- `<corpus>` — YANG tree to audit, always supplied explicitly:
  `scripts/audit.sh --corpus <corpus>`, or set the `YANG_CORPUS` environment
  variable once and omit the flag. The workflow never assumes a corpus
  location relative to the repo layout.
- `<grammar-repo>` — sibling `tree-sitter-yang` checkout (`../tree-sitter-yang`).
- `<out>` — where the runbook report is written (default `target/audit`).

## Loop

1. **Baseline.** Run `scripts/audit.sh --corpus <corpus>` (add `--repeat 3` or
   `--csv` if you want a memory curve). Read `<out>/report.txt` and
   `<out>/perf_rss.csv`. Note the totals: diagnostics by code, parse-error and
   not-a-yang counts, wall/CPU/RSS. Compare to the pinned baseline in
   `AGENTS.md` and to `<out>/prev.txt` (delta since last run).
2. **Categorize.** Group the diagnostics by root cause (grammar gaps that
   collapse whole files, unresolved-import/typedef/grouping cascades,
   duplicate-module corpus noise, memory/CST retention). Pick ONE issue.
3. **Prove it.** For a parser failure, shrink to the smallest repro with
   `target/release/examples/probe <file>` and minimal modules. Check the
   construct against RFC 7950 §14 before blaming the grammar.
4. **Fix (grammar issues).** In `<grammar-repo>` follow the `parser-regen`
   skill: edit `grammar.js`, `tree-sitter generate`, add a `tests/0NN_*.rs`
   case, `cargo test` there. yrepo already carries the
   `[patch.crates-io]` override so the fix is live locally — do not add it again.
5. **Re-audit.** In `yrepo`: `cargo test` (default and `--features parallel`),
   rebuild examples, then re-run `scripts/audit.sh --corpus <corpus>` and
   confirm the target counts dropped and nothing else regressed (watch for
   other codes increasing).
6. **Commit per concern.** Keep commits small and split by repo/issue (e.g.
   grammar fix commit in `<grammar-repo>`; separate dev-tooling commits in
   `yrepo`). Never mix an unrelated change into a fix commit.

## Constraints

- DO NOT bump versions or publish until the user explicitly approves.
- DO NOT hand-edit generated parser files; always regenerate.
- DO NOT claim a construct is invalid without checking RFC 7950 §14 (cross-check
  with `pyang` when unsure).
- Report user-content problems as yrepo **diagnostics**, never errors.
- Keep every claim grounded in an actual `scripts/audit.sh`/`probe`/`perf` run.

## Output

```
## audit run over <corpus>
baseline: <prev> -> <now> diagnostics (<t>s); perf: <wall>/<cpu>/<rss>
chosen issue: <category> (<count>)
repro: <path or minimal module>
root cause: ...
fix (if applied): <files changed in <grammar-repo> / yrepo>
verify: tests ok (both modes); audit now <now2> diags
commits: <list of proposed commits>
```
