---
description: Ask the zerograph index a question, an id or a symbol, then read the top hits
argument-hint: <question | ID | Symbol>
allowed-tools: mcp__plugin_zerograph_zerograph__ask
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__ask` with `question` set to: $ARGUMENTS

Pass the words as typed. Set `seeds: 8` only when the user wrote `--seeds 8` or asked for a wider
search, and `bodies: true` only when they asked for full requirement text. Never set `rerank`: it
spends model tokens, and that is the user's call.

Each top-level line of the answer is a seed, `path:line` first; a `└` line is a neighbour one hop
from the seed above it.

1. Open the top two or three seeds at their `path:line` with Read, a window around the line, not
   the whole file. Skip a seed whose headline plainly answers something else.
2. Answer from what you opened, citing `path:line`. Nothing printed holds it: say so, and offer one
   rephrase or a wider search, not a directory walk.
3. A literal string, an env var or a flag is a text search's question, not the graph's: use Grep.
