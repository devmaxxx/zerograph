---
name: status
description: zerograph's index state, refresh, serve and the machine's vector cache
---

Call the zerograph MCP server's `status` tool and the zerograph MCP server's `cache_status` tool,
both with no arguments, in one turn.

Summarise the two reports in a few lines: index size and files behind the tree, the model and any
vectors owed, whether a refresh is running and how far it has got, whether `serve` answers, and the
cache's size against its cap. Then say what, if anything, wants doing:

- no store, or files behind or vectors owed with no refresh running: `$zerograph:refresh`;
- warnings: quote them as printed;
- the cache at or over its cap: the user can run `zerograph cache prune` in a terminal.

Everything in order: say so in one line.
