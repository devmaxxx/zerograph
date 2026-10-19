#!/usr/bin/env bash
# One zerograph-only benchmark run against the pinned fixture, recorded into the history.
#
# The fixture is a corpus checkout that never moves, so two runs of this script differ only by
# zerograph's own code. That is the whole point: `bench/compare/run.py` answers "how do the
# three tools compare", which needs the other two indexes and half an hour; this answers "did
# my change help", which needs neither.
set -euo pipefail

FIXTURE="${FIXTURE:-$HOME/bench/beauty-crm-502e8a6d}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ARMS="${ARMS:-dense lexical}"
NOTE="${NOTE:-}"
# Another case file, e.g. bench/dev-cases.jsonl: measured and recorded under its own arm name,
# never graded -- see `bench::run` on which shapes carry floors. A relative path is resolved
# against the repo, not left to the caller's working directory: the script never cd's, so the
# documented `CASES=bench/dev-cases.jsonl` would otherwise run from the repo root and nowhere
# else.
CASES="${CASES:-}"
if [ -n "$CASES" ]; then
  case "$CASES" in /*) ;; *) CASES="$REPO/$CASES" ;; esac
fi
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

# A worktree's `.git` is a file pointing at the real one, never a directory.
[ -e "$FIXTURE/.git" ] || { echo "no fixture at $FIXTURE — see bench/history/README.md" >&2; exit 1; }
[ -f "$FIXTURE/.zerograph/graph.json" ] || { echo "fixture has no store — see bench/history/README.md" >&2; exit 1; }

# A run recorded against a fixture that moved is a run whose history rows cannot be compared,
# and the comparison would silently look like a regression in zerograph.
# A fixed length: the default abbreviation grows with the object count and stops matching.
PINNED=$(git -C "$FIXTURE" rev-parse --short=8 HEAD)
[ "$PINNED" = "502e8a6d" ] || { echo "fixture is at $PINNED, expected 502e8a6d" >&2; exit 1; }
# `graphify-out/` is tracked in the corpus and rebuilt in place by graphify, so it is dirty by
# design and says nothing about the sources. Everything else being dirty does.
DIRTY=$(git -C "$FIXTURE" status --porcelain -- ':(exclude)graphify-out')
[ -z "$DIRTY" ] || { echo "fixture has edited sources — not comparable to earlier runs:" >&2; echo "$DIRTY" >&2; exit 1; }

echo "building zerograph at $(git -C "$REPO" rev-parse --short HEAD)"
cargo build --release --manifest-path "$REPO/Cargo.toml" >/dev/null
BIN="$REPO/target/release/zerograph"

status=0
for arm in $ARMS; do
  case "$arm" in
    dense)   flags=() ;;
    lexical) flags=(--no-dense) ;;
    *) echo "unknown arm $arm (expected: dense, lexical)" >&2; exit 1 ;;
  esac
  echo "--- $arm ---"
  # The exit code is captured rather than allowed to end the script: a red arm is a result to
  # record, not a reason to skip the arms after it. `${flags[@]+...}` because bash 3.2, which
  # is what macOS ships, calls an empty array unbound under `set -u`.
  rc=0
  if [ -n "$CASES" ]; then flags+=(bench --cases "$CASES"); else flags+=(bench); fi
  "$BIN" --repo "$FIXTURE" ${flags[@]+"${flags[@]}"} > "$OUT/$arm.txt" 2>&1 || rc=$?
  tail -2 "$OUT/$arm.txt"
  # A crash leaves no summary line, and a run with no summary must not enter the history at
  # all rather than entering it as a row of zeroes that later reads as a regression.
  if grep -qE '^(\S+ [0-9]+/[0-9]+  )+p90 [0-9]+ tok' "$OUT/$arm.txt"; then
    python3 "$HERE/track.py" record "$OUT/$arm.txt" \
      --corpus beauty-crm --corpus-path "$FIXTURE" --note "$NOTE"
    [ "$rc" -eq 0 ] || status=$rc
  else
    echo "$arm produced no summary line — not recorded" >&2
    status=1
  fi
done

echo
python3 "$HERE/track.py" report
exit "$status"
