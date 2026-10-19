#!/usr/bin/env bash
# One configuration, one model, all tasks: the agent runs in the disposable worktree with the
# overlay as its .claude/, and every byte it streams is kept, because the tokens, the tool calls
# and the answer are all read back from that stream afterwards.
set -euo pipefail
CONFIG=$1; MODEL=$2; REPEAT=${3:-1}
M=${M:-/Users/max/bench/agent-surface-2026-09-07}; WT="$M/wt"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# C-rem is C with the interceptor off: the overlay directory is C's, the switch is the environment.
OVERLAY_NAME=$CONFIG
if [ "$CONFIG" = "C-rem" ]; then OVERLAY_NAME=C; export ZEROGRAPH_HOOK_INTERCEPT=0; fi
OVERLAY="$HERE/overlays/$OVERLAY_NAME"; [ -d "$OVERLAY" ] || { echo "no overlay $OVERLAY_NAME" >&2; exit 2; }
# A has no binary on PATH; every other configuration has the one this branch built.
case "$CONFIG" in A) BINPATH="" ;; *) BINPATH="$M/bin:" ;; esac
STAMP=$(date -u +%Y%m%dT%H%M%SZ); OUT="$M/runs/$STAMP-$CONFIG-$MODEL"; mkdir -p "$OUT"
# One TSV line per task — id, prompt, setup — read back with tabs as the only separator; prompts
# carry backticks and Cyrillic and no tabs.
python3 -c '
import json,sys
for l in open(sys.argv[1]):
    if l.strip():
        t=json.loads(l); print(t["id"], t["prompt"], " && ".join(t.get("setup",[])) or ":", sep="\t")
' "$HERE/tasks.jsonl" > "$OUT/tasks.tsv"
# The overlay is laid down *after* the reset, not before the loop: beauty-crm tracks its own
# `.claude/` — 45 files, its CLAUDE.md byte-identical to overlay B's — so `git checkout -- .`
# restores it and `git clean` removes whatever the overlay added. The 2026-09-09 runs put the
# overlay down once at the top and reset before every task, which silently ran configuration B
# twelve times whatever the argument said. The check below fails the run rather than measuring
# the wrong thing again.
for r in $(seq "$REPEAT"); do
  while IFS=$'\t' read -r id prompt setup; do
    git -C "$WT" checkout -- . >/dev/null 2>&1 || true
    git -C "$WT" clean -fdq -e .zerograph
    rm -rf "$WT/.claude"; cp -R "$OVERLAY" "$WT/.claude"
    cmp -s "$WT/.claude/CLAUDE.md" "$OVERLAY/CLAUDE.md" \
      || { echo "the overlay did not survive the reset: $WT/.claude is not $OVERLAY" >&2; exit 3; }
    (cd "$WT" && eval "$setup")
    (cd "$WT" && PATH="$BINPATH$PATH" claude -p "$prompt" --model "$MODEL" \
        --output-format stream-json --verbose --no-session-persistence \
        --setting-sources project --allowedTools "Bash,Read,Grep,Glob" \
        --disallowedTools "Edit,Write,MultiEdit,NotebookEdit" \
        --max-budget-usd "${CAP:-0.30}" < /dev/null > "$OUT/$id.r$r.jsonl" 2> "$OUT/$id.r$r.err") \
      || echo "$id r$r: exit $?" >> "$OUT/failures.txt"
  done < "$OUT/tasks.tsv"
done
git -C "$WT" checkout -- . >/dev/null 2>&1 || true
git -C "$WT" clean -fdq -e .zerograph
python3 "$HERE/score.py" "$OUT" --tasks "$HERE/tasks.jsonl" --config "$CONFIG" --model "$MODEL" > "$OUT/summary.json"
python3 -c "import json;s=json.load(open('$OUT/summary.json'));print(s['config'],s['model'],'hits',s['hits'],'/',s['tasks'],'tokens',s['tokens'],'cost',round(s['cost'],4),'asked',s['asked'],'grepped',s['grepped'])"
echo "$OUT"
