---
description: What the current diff touches and who reaches it — run before a commit or a PR
argument-hint: "[base] [--depth N]"
allowed-tools: mcp__plugin_zerograph_zerograph__changes
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__changes` for: $ARGUMENTS

A first word that is not a flag is `base`, the revision to diff against (`main` reads the whole
branch); none leaves it at the uncommitted diff. `--depth N` sets `depth` (1 is the direct callers
alone; the default is 2). The answer is a risk word, the symbols the diff touches and the callers
that reach them.

1. Lead with the risk word and the touched symbols.
2. For each affected caller outside the diff, open its `path:line` and say whether the change can
   break it: a new parameter, a changed return, a renamed export. Those are the review's and the
   test plan's real targets.
3. Name the affected callers no test covers, if any stand out. A caller list is the graph's view; a
   dynamic call has no edge, so Grep the changed names before calling a change contained.
