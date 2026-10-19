---
name: use-the-graph
description: Use when you need who calls a symbol, what a change would break, what an id like FR-PAY-22 is, or where a concept lives in a repository with a zerograph index — before grepping or reading files to orient yourself.
---

# zerograph before grep

The plugin's MCP server answers from the repository's zerograph index: code symbols, document ids
and the edges between them, at no model tokens. Its tools are named after the commands below.

| The question | Tool | Not |
| --- | --- | --- |
| what is `FR-PAY-22`, `BE-M10/T05`, `asGrosze` | `ask` with the id or symbol | rg for the id |
| where is this concept written | `ask` with four words | reading directories |
| who calls `X` | `impact` with `symbol: X`, `depth: 1` | rg for the name |
| what breaks if I change `Y` | `impact` with `symbol: Y` | guessing from memory |
| how does `A` reach `B` | `trace` with `from: A`, `to: B` | following imports by hand |
| what does my diff touch | `changes` (`base: main` for the branch) | re-reading the diff |
| a literal string, an env var, a flag | rg | the graph: it does not index literals |

- Every answer line starts with `path:line`. Open those; do not re-derive them.
- "Nothing uses this" from `impact` is the graph's view: a callback, a chained call or a destructured
  method has no edge. Pair it with an rg for the name before deleting anything.
- The readers bring the index in line with the tree before they answer; no build step belongs in
  your loop. `status` says whether the index is behind and whether a refresh runs.
- A thin `ask`: rephrase once or set `seeds: 8`. Still nothing: say the repository does not show it.

The user can run the same through `$zerograph:ask`, `$zerograph:impact`, `$zerograph:trace`,
`$zerograph:changes`, `$zerograph:explain`, `$zerograph:status` and `$zerograph:refresh`.
