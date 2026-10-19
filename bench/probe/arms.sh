#!/bin/bash
# arms.sh <binary> <repo> <logdir> <label>  — the four arms, transcripts kept, recorded with a tag
set -u
B=$1; R=$2; L=$3; T=$4
# Off the script's own location, not this machine's checkout: a clone that cannot run the script
# in its own tree is the defect the ledger already named once.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
H="$ROOT/bench/history"
# Declared for the fixture's arms as well as the worktree's, and refused when absent: the model
# decides which floors `bench` grades against, and the small model the fixture's store was written
# with is the one those floors were measured with. embedder.sh says why it is not a file in the tree.
. "$HERE/embedder.sh" || exit 2
embedder_declared || exit 2
mkdir -p "$L"
# A row that never reached the history is the one failure this script cannot shrug off: `bench`'s
# own non-zero status is a reading and is recorded anyway, but a `track.py` that refused — a
# transcript with no summary line, because the binary died before it printed one — leaves nothing
# behind at all. Without this the script's status was whatever the last `record` happened to
# return, so three lost arms behind one that landed read as a clean sweep.
missed=0
for suite in rec dev; do
  for arm in dense lexical; do
    nd=""; [ "$arm" = lexical ] && nd="--no-dense"
    cases=""; [ "$suite" = dev ] && cases="--cases $ROOT/bench/dev-cases.jsonl"
    out="$L/$T-$suite-$arm.txt"
    # Above the arm's own transcript, which `track.py` reads past: `bench` prints `model=small` for
    # the floors it graded against, and this says the hub id those floors were keyed from.
    embedder_line > "$out"
    ZEROGRAPH_NO_SERVE=1 REPOGRAPH_NO_SERVE=1 "$B" --repo "$R" $nd bench $cases >> "$out" 2>&1
    rc=$?
    tail -3 "$out"
    # Recorded whatever the status: `bench` exits non-zero when a floor fails, and a failed floor
    # is a reading the history exists to hold. G32's rebuilt arms are expected to fail one, and
    # its gate names them as recorded arms — an arm dropped for exiting non-zero is a lost row.
    [ "$rc" = "0" ] || echo "arms: $suite/$arm exited $rc — recording it anyway, see $out" >&2
    python3 "$H/track.py" record "$out" --corpus beauty-crm --corpus-path "$R" --tag "$T" --note "next-version-levers $T" ||
      { echo "arms: $suite/$arm did NOT reach the history — track.py refused $out" >&2; missed=1; }
  done
done
exit "$missed"
