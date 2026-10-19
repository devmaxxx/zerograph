---
name: trace
description: Shortest chain of calls from one symbol to another
---

Call the zerograph MCP server's `trace` tool for what the user gave (<From> <To> [--depth N])

The first word is `from`, the second `to`; `--depth N` sets `depth` (default 6). The answer is one
step per line, each at its `path:line`; a `Passes` step hands the next symbol over as a value
rather than calling it.

1. Walk the chain by opening each step's `path:line`; describe what carries the call across, in
   order.
2. No path within the depth is an answer, not an error. Say so, then offer a higher `depth`, the
   reverse direction, or an rg for a link the graph cannot see (a callback, an event, a string
   key).
