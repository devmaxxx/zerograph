---
description: zerograph's index state, refresh, serve and the machine's vector cache
allowed-tools: mcp__plugin_zerograph_zerograph__status mcp__plugin_zerograph_zerograph__cache_status
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__status` and `mcp__plugin_zerograph_zerograph__cache_status`,
both with no arguments, in one turn.

Summarise the two reports in a few lines: index size and files behind the tree, the model and any
vectors owed, whether a refresh is running and how far it has got, whether `serve` answers, and the
cache's size against its cap. Then say what, if anything, wants doing:

- no store, or files behind or vectors owed with no refresh running: `/zerograph:refresh`;
- warnings: quote them as printed;
- the cache at or over its cap: the user can run `zerograph cache prune` in a terminal.

Everything in order: say so in one line.
