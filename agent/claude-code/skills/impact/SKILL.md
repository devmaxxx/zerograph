---
description: Blast radius of a symbol before you change it — callers by depth, importing files and a risk line
argument-hint: <Symbol> [--depth N] [--down]
allowed-tools: mcp__plugin_zerograph_zerograph__impact
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__impact` for: $ARGUMENTS

The first word is `symbol`. `--depth N` sets `depth` (1 is the "will break" list alone; the
default is 3), and `--down` sets `down: true`, which walks what the symbol calls instead of who
reaches it.

1. Lead with the risk line and the depth-1 callers: those are the call sites a signature change
   breaks. Open each at its `path:line` before saying how it uses the symbol.
2. A note naming other candidates means the name was ambiguous; say which one was taken.
3. An empty or short list is not proof that nothing uses it: a call through a callback, a chained
   expression or a destructured method has no edge. Confirm with a Grep for the name (files with
   matches) before calling anything dead or safe to delete.
