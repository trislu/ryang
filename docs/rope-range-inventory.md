# rope-range redesign — consumer inventory (2026-09-06)

Goal: size the blast radius of storing `Argument.logical` and `Token.text`
(and `Comment.text`) as byte ranges into the retained document source instead
of owned `String`s, extracting text on demand.

## Field storage & construction (yrepo)

- `Argument.logical: String` — set once in `syntax.rs` `logical_argument(raw)`
  (dequote + join `+` fragments) and stored on the Statement tree.
- `Token.text: String` (quoted-string runs keep their quotes) — set during
  `collect_tokens`.
- `Comment.text: String` — set during `collect_comments`.

## Consumers (read sites)

### `Argument.logical` — yrepo internal: 11
- `syntax.rs`: `Argument::name()` trims; char-level helper iterates it.
- `value.rs` (TypeFacets): copies trimmed value into `length/range/pattern/
  path/base` facets (owned `String`s on the node).
- `compile.rs`: `key` arg split_whitespace (805); `default` cloned onto node
  (821, 2484).
- LS src: 0 direct; tests: 5.

### `Token.text` — LS: ~7 sites, yrepo tests: several
- LS `semantic_token.rs`: clones `t.text` into semantic-token builders (583)
  and inspects chars (456); `comment.text` used by LS `format.rs` (31).
- yrepo tests assert exact token/comment strings.

### `Comment.text` — LS `format.rs` only (+ tests)

## Decision inputs

- Real-module decomposition: owned duplicate (arg+token) ≈ **142%** of source,
  with one heap allocation per token (~927/file) and per argument (~260/file).
- Keeping the source `Arc<str>` per document (already retained as `Text`) and
  storing `(Arc<str>, Range<usize>)` per token/argument removes those heap
  strings/allocations; Arc is shared (refcount) so per-item cost is 8–16 B
  plus the source is retained once (as today).
- Dequoted/joined `logical` values (quote stripping, `"a"+"b"` concatenation)
  are NOT simple byte ranges of the source — this is the main design
  complication for `Argument.logical` (offsets must be computed on demand or
  materialized lazily per argument, defeating part of the saving for that
  field). `Token.text` and `Comment.text` ARE raw source slices (token text
  with quotes = source slice; comment text = source slice) — safe range
  candidates with no extraction cost.

## Recommended plan

1. Range-ify `Token.text` and `Comment.text` first (true source slices): add
   getters (`text() -> &str` via the shared source) and keep construction in
   `collect_tokens/comments` trivial. Measure ingest RSS on the real RFC
   subset and synthetics (expect the token share, 78% of source + ~927
   allocations/file, to dominate the win).
2. Treat `Argument.logical` separately: value.rs facet copies mean an owned
   trimmed String is often needed anyway; prototype lazy materialization only
   if (1) shows the structural cost remains dominated elsewhere.
3. API: `Token.text`/`Comment.text` are public fields read in LS/tests —
   changing them to ranges is breaking for published API (release gate +
   version bump required) or can be introduced behind new accessors with the
   fields deprecated-then-removed.

## Open measurement still wanted
- Allocator-level split (bytes vs allocations) to confirm expected win before
  the breaking change; a cheap proxy: memstep delta when tokens/comments are
  dropped entirely (retention-policy upper bound).
