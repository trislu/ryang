# Memory findings (giant-corpus goal, 2026-09-06)

Method: non-parallel stepwise probes (`examples/memstep.rs`, one file at a
time, RSS/VmHWM interval logs, `$MEMSTEP_LOG` for the exhaustion edge) plus
retention decomposition (`examples/memcomp.rs`). All numbers are DEBUG-build
sequential-process measurements on synthetic trees (real corpus numbers to be
collected separately when a giant sample is available; commit messages carry
no corpus statistics).

## Synthetic micro modules (2000 files, 314 B avg, 628 KB total)

| metric | with CST (before drop) | CST dropped | delta |
| --- | --- | --- | --- |
| ingest RSS at 2000 files | 49 152 KB | 40 144 KB | −18% |
| per-file ingest slope | ≈ 23.0 KB/file | ≈ 18.1 KB/file | −4.9 KB/file |
| peak incl. compile | 62 276 KB | 61 028 KB | −1.2 MB |

Decomposition (post-drop): source 628 KB; statements 38 000;
`Argument.logical` owned bytes = 32.5% of source; token text bytes = 81.5%;
owned duplicate (arg+token) = **114%** of source.

## Realistic-shaped synthetic (1500 files, 949 B avg, 1.42 MB total)

Decomposition: statements 44/file; arg bytes 45% of source; token bytes 85%;
owned duplicate = **130%** of source (tokens 201/file).
Waterline: ingest RSS ≈ 40 KB/file (42× source); compile adds a transient
≈ +22 MB at 1500 files (peak 86 MB), which scales with the arena/library.

## Real modules — ingest waterline (RFC dir subset sampling)

memstep (sequential, post-CST-drop) on 200 of 486 real RFC files: RSS grows
≈ 140–350 KB/file as later (larger) modules load — order of magnitude
**~0.1–0.35 MB per ~20 KB module** (~10× source). Even the smallest reading
makes a 163k-file full compile fundamentally multi-10-GB: holding full parse
views + built schema arenas for every file in one process cannot fit WSL RAM.
This is the load-bearing conclusion: rope-range (~≤15%) and CST-drop are
secondary; the necessary lever is RETENTION/SCOPE — compile (and parse) only
the closure of modules actually queried/open, with an index-only catalog for
the rest. See `rope-range-inventory.md` and the language-server design note
(closure-scoped compile) to follow.

## Real modules (RFC dir subset: 486 files, 19.5 KB avg, 9.5 MB total)

Decomposition: statements 260/file; `Argument.logical` owned bytes = **64%**
of source; token text = 78% of source (927 tokens/file); owned duplicate =
**142%** of source; comments 5/file. Real content therefore shows HIGHER
string duplication than the synthetic sets, so the rope-range redesign's
ceiling on realistic trees is above the synthetic estimate (string bytes +
one heap allocation per argument and per token).

## Catalog-only retention experiment (views dropped, header+source kept)

Local temporary patch (parse returns no statement/token/comment views), same
200 real RFC files: RSS at 200 files = 10 740 KB vs 49 408 KB with full
views → catalog-only retention is ≈ **22%** of full-parse retention
(**−78%**, ~34 KB/file vs ~235 KB/file; still ≈1.7× source because the full
source text is retained per document). Even ~34 KB/file over a 163k tree
(~5.5 GB) is too large to hold in full, so a catalog variant that also drops
the retained source (only extracted header fields per document) is the next
candidate to measure; on-demand re-read/re-parse supplies text when a
document is opened. Full-views compile (schema arenas for every module)
remains fundamentally out of scope for one process.

## No-source catalog experiment (full build then free)

Second local experiment: keep header fields but ALSO drop the retained source
after a FULL parse+build. Because the statement views were still constructed
before being freed, the allocator's retained high-water dominates: RSS at 200
real RFC files = 17 536 KB — HIGHER than the header+source-light path
(10 740 KB) and 3× lower than full retention (49 408 KB). Conclusion: the
catalog path must NOT build full statement/token views at all; it needs a
LEAN HEADER SCANNER (parse the CST only down to the module header: name,
revision, imports/includes, prefix, namespace) whose per-file footprint should
be a few KB — the next prototype to implement and measure.

## Interpretation

1. Dropping the retained tree-sitter CST (no consumer; committed) cut ingest
   ~18% on the micro set — real but not the whole story.
2. Owned-string duplication (arg + token text) is ~1.1–1.3× source, plus one
   heap allocation per token/argument. The rope-range redesign (byte `Range`
   into the retained source, extract on demand) bounds its saving at roughly
   the duplicate bytes + allocator headers: ≈ 10–15% of ingest RSS on these
   samples, with public-API breakage on `Argument.logical`/`Token.text` (and
   callers that copy them out today). Worth prototyping, not a silver bullet.
3. ~90–95% of per-file RSS is per-statement/per-token STRUCTURE (recursive
   `Statement` owning `Vec<Statement>` children, token structs, per-node
   allocations), not text. The larger levers are layout/arena and
   retention policy (e.g., an index-only workspace scan that keeps light
   views instead of full parse views for non-open documents), plus re-parsing
   on demand instead of keeping every document live.

## Open questions / next candidates

- Allocator-level profile (allocation counts/sizes per parse) to confirm the
  structural split before choosing rope-range vs arena/layout work.
- Retention policy for the language-server giant-workspace scan: full parse
  of every file at open time is the current design; an index-only mode (keep
  header/name/imports, drop statement/token/comment views for closed docs)
  would bound memory for 163k-scale trees.
- Re-run memstep/memcomp on the real-shaped corpus and at the giant scale
  when available; log per 10k files with sleeps for waterline curves.
