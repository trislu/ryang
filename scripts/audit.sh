#!/usr/bin/env bash
# Deterministic yrepo audit runbook: build, test both feature modes, ingest a
# YANG corpus, and record a diagnostics + memory report. Editor/agent agnostic —
# call it from Copilot, another extension, a CI job, or by hand.
#
# Report hygiene: persisted reports never hardcode the external corpus
# directory layout. The report identifies the corpus only by name
# (--corpus-name / $YANG_CORPUS_NAME; default: the corpus directory basename)
# and refers to probed files by basename. The real corpus path is printed to
# the console only (interactive), never baked into report.txt.
#
# Usage:
#   scripts/audit.sh [--corpus DIR] [--corpus-name NAME] [--out DIR]
#                    [--repeat N] [--no-perf] [--no-tests] [file.yang ...]
#
#   --corpus DIR      YANG tree to audit. If omitted, the $YANG_CORPUS
#                     environment variable is honored. The corpus location is
#                     an explicit input — this script never guesses it from the
#                     checkout's surroundings. With neither, whole-tree
#                     inspect/perf are skipped and only explicit probe files
#                     are processed.
#   --corpus-name NAME
#                     How the corpus is identified in the report (default:
#                     $YANG_CORPUS_NAME, else the corpus dir basename, else
#                     <none>). Use a stable name such as the upstream repo, not
#                     a local directory.
#   --out DIR         where report.txt / perf_rss.csv / prev.txt go
#                     (default: <repo>/target/audit).
#   --repeat N        perf repeat count (default 1).
#   --no-perf         skip the memory/perf measurement.
#   --no-tests        skip cargo test (both feature modes).
#   file.yang...      extra files to probe individually (optional).
#
# Exit code: 0 if tests pass and audit completed; 2 on usage/build error; 1 if
# tests failed.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

corpus=""
corpus_name=""
out="$REPO/target/audit"
repeat=1
do_perf=1
do_tests=1
probe_files=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --corpus)      corpus="$2";      shift 2 ;;
    --corpus-name) corpus_name="$2"; shift 2 ;;
    --out)         out="$2";         shift 2 ;;
    --repeat)      repeat="$2";      shift 2 ;;
    --no-perf)     do_perf=0;        shift ;;
    --no-tests)    do_tests=0;       shift ;;
    -h|--help) sed -n '1,26p' "$0"; exit 0 ;;
    --) shift; break ;;
    -*) echo "audit: unknown option $1" >&2; exit 2 ;;
    *) probe_files+=("$1"); shift ;;
  esac
done
probe_files+=("$@")

if [[ -z "$corpus" ]] && [[ -n "${YANG_CORPUS:-}" ]]; then
  corpus="$YANG_CORPUS"
fi
if [[ -z "$corpus" && ${#probe_files[@]} -eq 0 ]]; then
  echo "audit: no corpus — pass --corpus DIR or set \$YANG_CORPUS, or give files to probe" >&2
  exit 2
fi

if [[ -z "$corpus_name" ]] && [[ -n "${YANG_CORPUS_NAME:-}" ]]; then
  corpus_name="$YANG_CORPUS_NAME"
fi
if [[ -z "$corpus_name" ]]; then
  if [[ -n "$corpus" ]]; then corpus_name="$(basename "$corpus")"; else corpus_name="<none>"; fi
fi

mkdir -p "$out"
report="$out/report.txt"
csv="$out/perf_rss.csv"
prev="$out/prev.txt"

{
  echo "== yrepo audit =="
  echo "date:     $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "yrepo:    $(git -C "$REPO" describe --tags --always 2>/dev/null || echo '?') (Cargo $(grep -m1 '^version' Cargo.toml))"
  echo "grammar:  $(git -C "$REPO/../tree-sitter-yang" describe --tags --always 2>/dev/null || echo 'n/a') (Cargo $(grep -m1 '^version' "$REPO/../tree-sitter-yang/Cargo.toml" 2>/dev/null || echo 'n/a'))"
  echo "corpus:   $corpus_name"
  echo "out:      ${out#"$REPO"/}"
  echo
} | tee "$report"
echo "corpus-dir: ${corpus:-<none>}   (interactive; not recorded in the report)" >&2

# --- build ---
echo "-- build examples (release, parallel) --" | tee -a "$report"
cargo build --release --features parallel --examples

# --- tests (both feature modes) ---
tests_ok=1
if [[ "$do_tests" == 1 ]]; then
  for mode in "" "--features parallel"; do
    echo "-- cargo test $mode --" | tee -a "$report"
    if ! out_tests=$(cargo test $mode 2>&1); then
      tests_ok=0
    fi
    echo "$out_tests" | grep -E "test result:" | tail -n 5 >> "$report" || true
  done
else
  echo "-- tests skipped --" | tee -a "$report"
fi
echo "tests_ok=$tests_ok" | tee -a "$report"

# --- whole-tree inspect ---
if [[ -n "$corpus" ]]; then
  echo "-- inspect $corpus_name --" | tee -a "$report"
  insp=$(./target/release/examples/inspect "$corpus" 2>&1) || true
  echo "$insp" | sed -n '1,16p' | tee -a "$report"
  total=$(echo "$insp" | sed -n 's/^total diagnostics: //p')
  if [[ -n "$total" ]]; then
    if [[ -f "$prev" ]]; then
      prev_total=$(cat "$prev")
      echo "diag-delta: $prev_total -> $total ($(( total - prev_total >= 0 ? total - prev_total : prev_total - total )) diff)" | tee -a "$report"
    fi
    echo "$total" > "$prev"
  fi
fi

# --- perf (time + CPU + RSS) ---
if [[ "$do_perf" == 1 ]] && [[ -n "$corpus" ]]; then
  echo "-- perf --repeat $repeat (csv: ${out#"$REPO"/}/perf_rss.csv) --" | tee -a "$report"
  perf_out=$(./target/release/examples/perf --repeat "$repeat" --csv "$csv" "$corpus" 2>&1 || true)
  printf '%s\n' "$perf_out"
  printf '%s\n' "$perf_out" | sed "s|$out/|${out#"$REPO"/}/|g" >> "$report"
fi

# --- per-file probes: console keeps full paths, report keeps basenames only ---
sanitize() {
  python3 -c '
import sys, re
pat = re.compile(r"\S*\.yang")
for line in sys.stdin:
    sys.stdout.write(pat.sub(lambda m: m.group(0).split("/")[-1], line))
'
}
for f in "${probe_files[@]}"; do
  echo "-- probe $f --"
  probe_out=$(./target/release/examples/probe "$f" 2>&1 || true)
  printf '%s\n' "$probe_out"
  echo "-- probe $(basename "$f") --" >> "$report"
  printf '%s\n' "$probe_out" | sanitize >> "$report"
done

echo
echo "report: $report"
if [[ "$tests_ok" != 1 ]]; then
  echo "audit: TESTS FAILED (see report)" >&2
  exit 1
fi
exit 0
