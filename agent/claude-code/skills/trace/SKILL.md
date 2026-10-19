---
description: Shortest chain of calls from one symbol to another
argument-hint: <From> <To> [--depth N]
allowed-tools: mcp__plugin_zerograph_zerograph__trace
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__trace` for: $ARGUMENTS

The first word is `from`, the second `to`; `--depth N` sets `depth` (default 6). The answer is one
step per line, each at its `path:line`; a `Passes` step hands the next symbol over as a value
rather than calling it.

1. Walk the chain by opening each step's `path:line`; describe what carries the call across, in
   order.
2. No path within the depth is an answer, not an error. Say so, then offer a higher `depth`, the
   reverse direction, or a Grep for a link the graph cannot see (a callback, an event, a string
   key).
