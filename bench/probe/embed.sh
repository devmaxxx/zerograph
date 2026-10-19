#!/bin/bash
# embed.sh NAME BINARY WORKTREE LOGDIR
# A whole-store embed under the sampler, the store's vectors removed first, every stderr line
# stamped with the wall clock so the cadence is read from when lines arrived and not from the
# rate the line itself quotes. `caffeinate -di` keeps the machine and the display awake: the run
# that closed G19 lost its intervals to a laptop that slept, and `judge.py cadence` refuses a
# transcript with no `start` stamp.
#
# Exit 4 is the one refusal that leaves the store mid-write: the vectors were already removed and
# the embed was cut off partway through writing them back. Every other refusal exits 2 and leaves
# the store either untouched or whole. 4 and not 3 because 3 is the binary's own verdict status —
# `judge.py` and `measure.sh` read a 3 as a row that answered and was worth timing, and a store
# left half written is the opposite of a reading.
set -u

# A wrapper and the two levels under it, deepest first: `caffeinate -di cmd` exec's `time` in
# place today, and a future one that forks instead puts the binary a level deeper.
process_tree() {
  local c g
  for c in $(pgrep -P "$1" 2>/dev/null); do
    for g in $(pgrep -P "$c" 2>/dev/null); do printf '%s\n' "$g"; done
    printf '%s\n' "$c"
  done
  printf '%s\n' "$1"
}

# SIGKILL behind SIGTERM, and neither of them optional: a refusal that only signalled the wrapper
# would leave the binary embedding under a script that has stopped watching it, and one that
# waited on a process which ignored the first signal would sit there for the whole 1,930 s embed —
# which is the reading being refused, taken anyway, with nobody reading it.
kill_tree() {
  local p pids
  pids=$(process_tree "$1")
  for p in $pids; do kill "$p" 2>/dev/null; done
  sleep 0.5
  for p in $pids; do kill -9 "$p" 2>/dev/null; done
}

# The refusal that has to be read before the store is: this script removes `vectors.f32` before it
# starts, so an embed killed partway through leaves neither the old vectors nor a whole set of new
# ones — and the next thing anyone does with a store is trust it.
# The store a binary reads in a tree: `.zerograph` for a current binary, else the `.repograph` a
# pinned pre-rename binary built.
store_of() { if [ -e "$1/.zerograph" ]; then echo "$1/.zerograph"; else echo "$1/.repograph"; fi; }

refuse_partial_store() {
  echo "refusing: $1" >&2
  echo "  the embed was killed mid-run, so $(store_of "$2")/ holds a partial set of vectors and no whole one — reset.sh before anything reads this store" >&2
  return 4
}

# The summary row, or a refusal instead of one. `judge.py compare` reads a `peak_cpu` of 0 as
# "never sampled": it carries `None` and leaves that column unjudged, so §9's clause about peak CPU
# would be satisfied by a row with nothing behind it at all. The reader rows of `measure.sh` read 0
# honestly — they are over before `top -l 2` reports twice — which is why this refusal belongs here,
# in the script that measures the one row that column judges, and not in `judge.py`. The count of
# samples the peak was taken over travels on the row under the name `measure.sh` gives it, so a
# reader sees how many readings are behind the number and `judge.py` still parses one shape.
summary_row() {
  local name=$1 wall=$2 usr=$3 sys=$4 rss=$5 rc=$6 file=$7 n peak
  n=$(awk 'END{print NR+0}' "$file")
  peak=$(awk '{gsub("%","",$2); if ($2+0>m) m=$2+0} END{print m+0}' "$file")
  [ "$n" != "0" ] || { echo "refusing: no samples in $file — the peak CPU column would read 0, which compare leaves unjudged" >&2; return 2; }
  [ "$peak" != "0" ] || { echo "refusing: $n samples in $file and a peak of 0% — a whole-store embed that burns no CPU is a sampler that read the wrong process" >&2; return 2; }
  # `sys` rides beside `user` because `judge.py` derives its average-CPU column from the pair; a
  # row without it keeps every other reading and carries no average at all, and this is the one row
  # of the kit that column was added for.
  printf '%s  wall=%ss user=%ss sys=%ss maxrss=%sGB peak_cpu=%s%% samples=%s rc=%s\n' "$name" "$wall" "$usr" "$sys" "$rss" "$peak" "$n" "$rc"
}

# Sourcing this file defines the function above and runs nothing, so `test_embed.py` reads the
# row's shape and both refusals without an embed under them.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then return 0; fi

NAME=$1; B=$2; F=$3; L=$4
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The one script here that writes vectors, so the model it writes them with is refused rather than
# defaulted: an absent declaration would fill the store with whatever the built-in default is today
# and the cadence would name no model at all. embedder.sh says why it is not a file in the tree.
. "$HERE/embedder.sh" || exit 2
embedder_declared || exit 2
# The one script here that deletes store files, so it refuses the pinned fixture by path: nothing
# writes there, and a mistyped argument is how that rule would get broken. Both sides go through
# `pwd -P` the way reset.sh compares its two directories — comparing a resolved path against an
# unresolved `$HOME/…` literal misses the moment anything above `bench/` is a symlink, and the
# miss deletes the vectors of the one store that is never rebuilt.
PINNED=${PINNED:-$HOME/bench/beauty-crm-502e8a6d}
pinned=$(cd "$PINNED" 2>/dev/null && pwd -P) || pinned=$PINNED
target=$(cd "$F" 2>/dev/null && pwd -P) || { echo "refusing: WORKTREE=$F is not a directory" >&2; exit 2; }
[ "$target" != "$pinned" ] || { echo "refusing: $F is the pinned fixture" >&2; exit 2; }
mkdir -p "$L"
# Before the vectors go, so a refusal here leaves the store whole: three runs pool into one
# `summary.txt` on purpose — §9's median is taken over them — but the file is keyed by nothing, so
# a run repeated into the same log directory puts a second `control-1` beside the first and
# `judge.py medians` groups both under `control` at n = 6. `readers.sh` truncates its summary for
# this reason and this script cannot, having one row a call; refusing the repeat is the same guard
# from the other end. Remove the row, or use another log directory, and the run goes again.
if [ -f "$L/summary.txt" ] && grep -q "^$NAME  " "$L/summary.txt"; then
  echo "refusing: $L/summary.txt already holds a row for $NAME — a second one would pool into its median at n+1" >&2
  exit 2
fi
if ! "$HERE/quiet.sh" --wait 600 > "$L/$NAME.quiet" 2>&1; then cat "$L/$NAME.quiet"; echo "refusing to measure on a machine that is not quiet" >&2; exit 2; fi
cat "$L/$NAME.quiet" | tee -a "$L/summary.txt"
# One row a call, so this row's own line: §9's three runs pool into one summary and a reader of it
# should not have to assume the three were embedded by the same model.
embedder_line | tee -a "$L/summary.txt"
S=$(store_of "$F")
rm -f "$S/vectors.f32" "$S/vectors.json"
# `$| = 1`: the stamper's own stdout is a file, so perl block-buffers it, and nobody waits for the
# stamper — it is a process substitution, and `wait` below waits on the wrapper. A 4 KB boundary
# falling inside `time`'s report then leaves the last lines of the transcript unwritten when the
# fields are read off it half a second later: `wall=s maxrss=GB`, and a 32-minute embed that
# succeeded is refused for not having completed. The stamp is taken when the line arrives either
# way, so autoflushing costs the reading nothing.
STAMP='perl -MTime::HiRes=time -ne '"'"'BEGIN { $| = 1 } printf "%.2f %s", time, $_'"'"''
perl -MTime::HiRes=time -e 'printf "%.2f start\n", time' > "$L/$NAME.err"
caffeinate -di /usr/bin/time -l "$B" --repo "$F" embed >"$L/$NAME.out" 2> >(eval "$STAMP" >> "$L/$NAME.err") &
WRAP=$!
# The sampled process is found under the wrapper by the binary's own name, not by a regex over
# two paths: a path holding an ERE metacharacter makes `pgrep -f` match nothing, and the old
# fallback then sampled the wrapper, which burns no CPU. Either miss reads `peak_cpu = 0`, and 0
# is the one value `judge.py compare` takes for "never sampled" and leaves unjudged, so §9's gate
# would go green with the column it names never read; a run that cannot be sampled refuses.
# `caffeinate -di cmd` exec's `time` in place and forks one child that does nothing but hold the
# power assertion — measured on 2026-09-10, the tree is `time` with `caffeinate` and the binary as
# its two children — so the binary is that holder's sibling, and one level deeper should a future
# caffeinate fork instead of exec.
BIN_NAME=$(basename "$B")
PID=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  PID=$(pgrep -P "$WRAP" -x "$BIN_NAME" 2>/dev/null | head -1)
  [ -n "$PID" ] || PID=$(for c in $(pgrep -P "$WRAP" 2>/dev/null); do pgrep -P "$c" -x "$BIN_NAME" 2>/dev/null; done | head -1)
  [ -n "$PID" ] && break
  kill -0 "$WRAP" 2>/dev/null || break
  sleep 0.5
done
[ -n "$PID" ] || {
  kill_tree "$WRAP"
  wait "$WRAP" 2>/dev/null
  refuse_partial_store "no $BIN_NAME under $WRAP to sample — the peak CPU column would read 0, which compare leaves unjudged" "$F"
  exit $?
}
: >"$L/$NAME.samples"
# The row `top` printed for this pid, the way measure.sh takes it and for the reason stated there:
# the loop's last pass straddles the exit, so `top`'s last line is the column header and not a
# reading. Here it is what makes the empty-file refusal below reachable at all — a binary that dies
# inside two seconds would otherwise leave one header line and be refused on its peak instead.
while kill -0 "$PID" 2>/dev/null; do
  top -l 2 -s 1 -pid "$PID" -stats pid,cpu,mem,th 2>/dev/null |
    awk -v p="$PID" '$1 == p {l=$0} END{if (l != "") print l}' >>"$L/$NAME.samples"
done
wait "$WRAP"; RC=$?
sleep 0.5
# The embed's own stderr shares this file with `time`'s report — that is the point of the stamps —
# and an unanchored `/real/` prints one number per matching line, which would split the summary
# line `judge.py medians` parses. Each field is read off the report's whole shape, one stamp wider
# than measure.sh's, and only the last such line is taken.
ERR=$L/$NAME.err
RSS=$(awk 'NF == 6 && $3 == "maximum" && $4 == "resident" && $5 == "set" && $6 == "size" {r=$2} END{if (r != "") printf "%.2f", r/1073741824}' "$ERR")
REPORT=$(awk 'NF == 7 && $3 == "real" && $5 == "user" && $7 == "sys" {l=$0} END{print l}' "$ERR")
WALL=$(printf '%s\n' "$REPORT" | awk '{print $2}')
USR=$(printf '%s\n' "$REPORT" | awk '{print $4}')
SYS=$(printf '%s\n' "$REPORT" | awk '{print $6}')
# `time` leaves a full report even when the command bailed, so a partial embed — weights gone
# mid-run, the disk full at 60% — would otherwise enter the median of three as an honestly
# measured, much faster whole-store embed. The row carries its status and `judge.py medians`
# refuses a row that measured a failure; the script's own exit is 4, below, because the store the
# run left behind is the same partial one every other failure here leaves.
[ "$RC" = "0" ] || echo "embed: $NAME exited $RC — this row measured a failure" >&2
ROW=$(summary_row "$NAME" "$WALL" "$USR" "$SYS" "$RSS" "$RC" "$L/$NAME.samples") || {
  # What the store holds follows the embed's status and not the sampler's, which is the difference
  # between the two refusal codes. A binary that dies inside two seconds comes through this door
  # rather than the no-PID one — `pgrep` catches it once, and the sampler's first `kill -0` finds it
  # already gone, so the samples file stays empty — and the vectors were removed before it started,
  # so nothing whole is left in their place; refusing at 2 would tell an operator otherwise, and the
  # next `ask` on that store answers lexical-only without saying so.
  [ "$RC" = "0" ] && exit 2
  echo "  the vectors were removed before the embed started and it exited $RC, so $(store_of "$F")/ holds no whole set of them — reset.sh before anything reads this store" >&2
  exit 4
}
printf '%s\n' "$ROW" | tee -a "$L/summary.txt"
# Not a pipeline: bash 3.2 here has no pipefail, and `| tee` hands back tee's status — the
# cadence verdict would be swallowed and this script would exit 0 on an OUTSIDE reading, which is
# the trap readers.sh names on its own quiet gate.
python3 "$HERE/judge.py" cadence "$L/$NAME.err" > "$L/$NAME.cadence" 2>&1; CAD=$?
cat "$L/$NAME.cadence" | tee -a "$L/summary.txt"
# 4 and the warning here too, not the embed's own status: what the store holds follows the embed and
# not the sampler, and a sampled failure — the disk full at 60%, the weights gone mid-run — leaves
# exactly what the no-PID and the no-samples doors leave, the vectors removed and no whole set in
# their place. Exiting `$RC` said 1 or 2, and the header above promises that every code but 4 leaves
# the store untouched or whole; an operator reading 2 that way runs the reader suite on a store with
# no vectors and every `ask` answers lexical-only without saying so. The row is still printed and
# still carries `rc=`, which is what `judge.py medians` refuses it on.
[ "$RC" = "0" ] || {
  echo "  the vectors were removed before the embed started and it exited $RC, so $(store_of "$F")/ holds no whole set of them — reset.sh before anything reads this store" >&2
  exit 4
}
exit "$CAD"
