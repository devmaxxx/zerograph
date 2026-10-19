#!/bin/bash
# reset.sh — the one writable beauty-crm worktree put back to a known state before a writer arm.
#
# One directory holds every writer on this branch, so arms are sequential and each starts from
# whatever the last one left unless something puts the fixture's state back. This does: tracked
# edits reverted and untracked files removed (the store excepted — it is rewritten below), the
# pinned fixture's store copied in whole, and one --no-dense update to settle the stamps: the
# copied manifest carries the fixture's mtimes, which match nothing here,
# so that first walk hashes every file, finds every hash unchanged, records this tree's stamps,
# and every later walk is stat-only. `zerograph-main` takes the no-op path on an unchanged tree
# and derives nothing, which is why the settle reads `changed 0` and is not a re-read.
#
# Which embedder the arm runs under is declared in the environment and refused when absent, so no
# writer here reaches for weights this machine does not hold; embedder.sh says why the declaration
# is not a file in the tree. The worktree it leaves has nothing untracked in it at all, which is
# what a `changes` row needs to map the one path it touched.
#
# Refuses with exit 2 and nothing touched unless WT is the locked, detached worktree at PIN and
# is not FIX — so a mistyped path cannot silently become the target. Exit 1 when the settle walk
# changed anything or its counts differ from the fixture's graph.json: the tree is not the one
# the store was built from, and nothing measured on it would be comparable.
#
# usage: ZEROGRAPH_EMBED_MODEL=intfloat/multilingual-e5-small reset.sh
#        (FIX, WT, PIN and BIN from the environment too; the defaults are this machine's)
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Before the directories are resolved, so a session set up without the declaration is refused
# before anything is touched rather than after the store has been replaced.
. "$HERE/embedder.sh" || exit 2
embedder_declared || exit 2
FIX=${FIX:-$HOME/bench/beauty-crm-502e8a6d}
WT=${WT:-$HOME/bench/beauty-crm-test}
PIN=${PIN:-502e8a6d}
BIN=${BIN:-$HOME/bench/levers-2026-09-10/bin/repograph-main}

refuse() { echo "reset: refusing — $1" >&2; exit 2; }

fix=$(cd "$FIX" 2>/dev/null && pwd -P) || refuse "FIX=$FIX is not a directory"
wt=$(cd "$WT" 2>/dev/null && pwd -P) || refuse "WT=$WT is not a directory"
[ "$wt" != "$fix" ] || refuse "WT and FIX are the same directory ($wt)"
# Current binaries read `.zerograph` only, so that name wins whenever anything stands there. A
# fixture built by a pinned pre-rename binary holds `.repograph`, the only name that binary reads.
st=.zerograph
[ -e "$fix/$st" ] || st=.repograph
other=.repograph
[ "$st" = .repograph ] && other=.zerograph
[ -f "$fix/$st/graph.json" ] || refuse "$fix/$st/graph.json is missing — not a store to restore from"
[ -x "$BIN" ] || refuse "BIN=$BIN is not executable"
block=$(git -C "$wt" worktree list --porcelain 2>/dev/null | awk -v w="$wt" 'BEGIN{RS=""; FS="\n"} $1 == "worktree " w {print; exit}')
[ -n "$block" ] || refuse "$wt is not a registered git worktree"
printf '%s\n' "$block" | grep -q "^HEAD $PIN" || refuse "$wt is not at $PIN (HEAD $(printf '%s\n' "$block" | awk '/^HEAD/ {print substr($2, 1, 8)}'))"
printf '%s\n' "$block" | grep -q '^detached$' || refuse "$wt is on a branch, not detached at $PIN"
printf '%s\n' "$block" | grep -q '^locked' || refuse "$wt is not locked — lock it, or this is the wrong directory"

# `reset --hard` rather than `checkout -- .`: a staged edit survives the latter.
git -C "$wt" reset -q --hard HEAD || exit 1
# The store under the name the fixture does not hold goes: a binary reading the other name would
# answer from it, so the restore below would not be the store that is read. `clean` cannot do it — a
# moved store carries its own `.gitignore` of `*`, which `clean` honours.
rm -rf "$wt/${other:?}" || exit 1
git -C "$wt" clean -fdq -e "$st" || exit 1
# The store is the one path `clean` is told to keep, and the one the filter below excuses: it is
# rewritten from the fixture two lines down, so removing it here would only cost an rsync. In this
# corpus it is ignored (`.gitignore`) and `status` never names it, but the pair is there for a
# corpus that does not ignore it, where `status` prints `?? $st/` and an unfiltered check
# would refuse every reset with "the tree did not come clean". Nothing else is spared, which is how
# a `zerograph.toml` (or a pre-rename `repograph.toml`) an earlier reset left behind goes: what a reading declares is in the
# environment now, so the tree keeps no file of ours for a `changes` row to map.
left=$(git -C "$wt" status --porcelain | grep -vxF "?? $st/")
[ -z "$left" ] || { echo "reset: the tree did not come clean:" >&2; echo "$left" >&2; exit 1; }
rsync -a --delete "$fix/$st/" "$wt/$st/" || exit 1
embedder_line
echo "reset: $wt at $PIN, tree clean, store restored from $fix:"
ls -l "$wt/$st" | awk 'NR > 1 {printf "  %10s  %s\n", $5, $9}'
out=$("$BIN" --repo "$wt" --no-dense update) || exit 1
echo "reset: settle  $out"
case "$out" in "changed 0 removed 0 "*) ;; *) echo "reset: the settle walk changed something — this tree is not the fixture's: $out" >&2; exit 1;; esac
want=$(python3 -c 'import json, sys; g = json.load(open(sys.argv[1])); print(len(g["nodes"]), len(g["edges"]))' "$fix/$st/graph.json")
got=$(echo "$out" | awk '/^changed/ {print $6, $8}')
[ "$got" = "$want" ] || { echo "reset: nodes/edges $got after the settle, fixture holds $want" >&2; exit 1; }
