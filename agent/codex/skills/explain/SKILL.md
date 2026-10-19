---
name: explain
description: One node and every edge attached to it — an id, a symbol or a label
---

Call the zerograph MCP server's `explain` tool with `node` set to what the user gave (<ID | Symbol | label>)

It tries the argument as an exact id, then a symbol name, then a label, and prints the node's kind
and `path:line`, then each edge in or out with the place it is written.

1. Open the node at its `path:line` and say what it is in a sentence.
2. Group the edges by kind and direction (what it cites, what cites it, what it calls) and open the
   ones the user's question is about; do not list every edge back.
3. Not found: suggest `$zerograph:ask` with the same words, which also matches by meaning.
