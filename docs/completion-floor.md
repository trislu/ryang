# PHASE 1 completion floor (ratified 2026-09-06)

Ratified by the workspace owner. Reference categorization: workspace root
`COMPLETION_FLOOR.md`; full dual-track status: `STATUS.md`.

## Floor

Phase 1 ("every valid YANG parses and resolves with zero diagnostics") is
interpreted over the **self-consistent subset** of a YANG repository: each
document is validated on its own terms against the module instances it
declares (canonical-latest for unpinned imports, pinned revision-date for
pinned imports, per-instance symbol tables). The families below are recorded
as the completion floor — yrepo keeps reporting them as diagnostics, and they
do not count against the zero-error objective:

- content-level unresolved references (augment-target / grouping / typedef /
  identity / import) where no revision in the corpus provides the target;
- content artifacts: corrupted extraction text (unterminated/embedded quotes,
  orphan statements, MIB transcripts), placeholder modules, and
  filename-vs-module mismatch notes;
- duplicate sibling names that come from cross-module augment chains or the
  multi-revision merge snapshot (same-file authoring duplicates ARE errors,
  `024_duplicate_nodes.rs`; dangling absolute leafref paths are errors,
  `025_leafref_path.rs`).

## Regression gate

Any change that introduces errors on the self-consistent subset, or new
whole-file parse collapses, fails the gate. The corpus audit (`scripts/audit.sh`)
with its pinned baseline is the guard; content-level residue may change as the
corpus evolves without failing the gate.

## History

- Phase-0 error localization closed (root `RECOVERY.md`).
- Phase-1 policy + per-instance/pin attribution + grammar tolerance fixes
  landed (local commits; see `YREPO_PHASES.md` for the phase refactor).
- Phase refactor ② (memoized grouping fragments, net-zero) + ③ (six explicit
  phase functions, net-zero) complete; ② step-3 DAG-order driver evaluated and
  closed as unnecessary (see `YREPO_PHASES.md`).
