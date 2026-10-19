#!/bin/bash
# readers.sh BINARY WORKTREE LOGDIR [N]
# The reader suite, N runs per row (default 5), in the one writable worktree straight after
# reset.sh has put the pinned fixture's store there, the resident server bypassed so every row is
# a cold process. Rows are the 2026-09-07 suite less `import-legacy`, which writes the store, and
# less `ask-stale`, which is every ask row now: every command that could walk the tree is read
# `--stale`, so no row refreshes stamps, rewrites a mirror or resyncs the index under the rows
# after it — the index moving under the suite is the cause G23 named — and the suite writes
# nothing, which the plan proves with the store's checksums before and after. `changes` needs a
# diff to read, so it touches one source file for the row's duration and moves the original back
# with its stamp (`cp -p`). Writes LOGDIR/summary.txt (one line per run) and LOGDIR/medians.txt.
set -u
B=$1; F=$2; L=$3; N=${4:-5}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
M="$HERE/measure.sh"
# Which embedder every row here answers under, refused when absent rather than left to the binary's
# built-in default; embedder.sh says why that declaration is not a file in the worktree, and the
# `changes` rows below are the reason it matters most.
. "$HERE/embedder.sh" || exit 2
embedder_declared || exit 2
# Both names: a pinned pre-rename binary reads only the old one.
export ZEROGRAPH_NO_SERVE=1 REPOGRAPH_NO_SERVE=1
mkdir -p "$L"
# Absolute before the `cd` below, the way HERE is: a relative LOGDIR would be re-created by
# `measure.sh` *inside* the worktree, and then every transcript, `dump10.json` and the `cn.ts`
# backup are untracked files in the tree the `changes` rows read — the row would measure the log
# directory it is writing, which is the defect the backup's own comment exists to prevent — while
# `judge.py medians` at the end read the near-empty file left in the original directory.
L=$(cd "$L" && pwd -P) || exit 2
# The worktree too, and for a second reason: the EXIT trap below puts `cn.ts` back by `$F/$CN`, and
# it fires after this script has changed directory into `$F`. A relative WORKTREE then resolves
# against the worktree itself — `$F/$F/…` — so the restore misses and the appended line stays in
# the file, under every row of whatever runs next.
F=$(cd "$F" && pwd -P) || exit 2
# And the binary, for the first reason again: `measure.sh` runs `$B` after this script has changed
# directory into `$F`, so a relative BINARY resolves inside the worktree — every row exits 127 and
# `judge.py medians` refuses the suite, or, worse, a corpus that happens to carry `bin/zerograph`
# gets measured instead of the candidate. A bare name is left alone: that one is a PATH lookup and
# means the same thing from either directory, and a path whose directory does not exist is left to
# fail where it is read rather than refused here.
case "$B" in
  /*) ;;
  */*) d=$(cd "$(dirname "$B")" 2>/dev/null && pwd -P) && B="$d/$(basename "$B")" ;;
esac
# Truncated, not appended: `measure.sh` appends, so a second suite into the same log directory
# would pool its runs with the first one's and `medians` would take one median over both.
: > "$L/summary.txt"
# Not a pipeline: bash 3.2 has no pipefail here and a `| tee` would hand back tee's exit code.
if ! "$HERE/quiet.sh" --wait 600 > "$L/quiet.txt" 2>&1; then cat "$L/quiet.txt"; echo "refusing to measure on a machine that is not quiet" >&2; exit 2; fi
cat "$L/quiet.txt" | tee -a "$L/summary.txt"
# Beside the quiet line and above the rows, so the file the medians are taken from says which
# weights answered them. `judge.py medians` reads only lines shaped like a run, as it already does
# past the quiet line.
embedder_line | tee -a "$L/summary.txt"
cd "$F" || exit 2
Q="как отменить запись и кто платит штраф"
CN=packages/ui/src/lib/cn.ts
# A Ctrl-C between the touch and the restore would leave the tree dirty for whatever ran next;
# `reset.sh` puts it back before the following arm, but not before the rest of this suite.
trap 'if [ -f "$L/cn.ts.orig" ]; then cp -p "$L/cn.ts.orig" "$F/$CN" && rm -f "$L/cn.ts.orig"; fi' EXIT
for i in $(seq 1 "$N"); do
  "$M" ask-fused-$i "$L" -- "$B" --repo "$F" ask --stale $Q
  "$M" ask-nodense-$i "$L" -- "$B" --repo "$F" --no-dense ask --stale $Q
  "$M" ask-rerank-local-$i "$L" -- "$B" --repo "$F" ask --stale --rerank-local $Q
  "$M" ask-exact-id-$i "$L" -- "$B" --repo "$F" ask --stale FR-PAY-22
  "$M" impact-$i "$L" -- "$B" --repo "$F" impact --stale cn --depth 3
  # A pair with a path: `main`→`cn` has none within six hops, and that row measured a 0.02 s early
  # exit. Since 0.5.0 `trace` says so with the verdict status 3, which `judge.py` reads as an
  # answered row: the row is kept and `medians.txt` names it under `verdict=1`, so what a reader
  # sees is that the bar would be set on an answer found in 0.02 s and not on a walk. This chain is
  # five hops — applyAppointmentCreate → evaluateAvailability → loadAvailabilitySnapshot →
  # toWallClock → faceToWallClock → pad — so the row times a traversal that finds something.
  "$M" trace-$i "$L" -- "$B" --repo "$F" trace --stale applyAppointmentCreate pad --depth 6
  # The backup lives in the log directory, not beside the file: `changes` reads every untracked
  # path `git ls-files --others --exclude-standard` names, and a `cn.ts.orig` in the worktree is
  # one — the row would measure two changed files, the second a whole copy of the first.
  cp -p "$CN" "$L/cn.ts.orig" || exit 2
  printf '\n// touched for the changes measurement\n' >> "$CN"
  "$M" changes-$i "$L" -- "$B" --repo "$F" changes --stale --depth 2
  # Refused, not carried into the next iteration: a restore that failed silently would leave the
  # touch in the tree, and the next iteration's backup is then a copy of the *modified* file — the
  # edit becomes permanent and grows a line per run, under every row after it.
  cp -p "$L/cn.ts.orig" "$CN" || { echo "readers: $CN not restored from $L/cn.ts.orig — the tree is dirty" >&2; exit 2; }
  rm -f "$L/cn.ts.orig"
  "$M" bench-nodense-$i "$L" -- "$B" --repo "$F" --no-dense bench
  "$M" bench-dense-$i "$L" -- "$B" --repo "$F" bench
  "$M" dump10-$i "$L" -- "$B" --repo "$F" dump --queries "$HERE/queries10.jsonl" --out "$L/dump10.json" --depth 300
done
# Not a pipeline, for the reason the quiet gate above gives: `| tee` hands back tee's status, so a
# suite holding a row that measured a failure would exit 0 with `medians.txt` truncated to empty —
# a refusal that reads as a clean sweep. The refusal joins the medians in the file because the log
# directory is what gets copied out of the run, and an empty file says nothing about why.
#
# The embedder heads the file rather than only `summary.txt`: `medians.txt` is what is copied out as
# the reference every later reading on this branch is judged against, and a reference that does not
# say which weights answered it can be compared against a suite read under others. `read_medians`
# takes only lines shaped like a median, so the line travels without being read as a row.
embedder_line > "$L/medians.txt"
python3 "$HERE/judge.py" medians "$L/summary.txt" >> "$L/medians.txt" 2>&1; MED=$?
cat "$L/medians.txt"
exit "$MED"
