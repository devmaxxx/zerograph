# The lexical held-out drop since v0.5.3: store provenance, not a commit

**Date:** 2026-10-18 · **Fixture:** `~/bench/beauty-crm-502e8a6d`, read only; every store below is
built in its own `cp -c -R` clone · **Arm:** lexical only (`--no-dense`), no enrich, no rerank

The 0.6.0 text-files reading (T5) had a text-off control that lost 6 lexical held-out answers against
B0, 113 → 107 of 400, 0 gained, exact McNemar p = 0.0312, and dense lost 5. The question was which
commit between `v0.5.3` and `main` cost them.

## Answer

None. Every binary from `v0.5.3` to `main`, each on a store it built itself from the same tree,
reads the same lexical held-out recall within noise. The 6 answers were lost by the store, not the
code: B0 was `v0.5.3` dumping a store whose walk predated `9a1199c` (#64, dot-paths and JavaScript
enter the corpus), which shipped inside `v0.5.3`. The control ran `update` first, and that update
read the 286 files #64 added. A before that never ran them was paired with an after that did.

The drop is accepted: it is the corpus #64 meant to add, not a regression. The 0.6.0 results already
drew the rule that follows from it — a before/after with `update` on one side needs it on the other —
and this reading confirms the mechanism it named.

## How it was read

The held-out set is 400 `synthetic` questions written by `v0.5.3`'s `bench/heldout.py build`
(seed 20260905; the script left the tree with enrich in #161) from the fixture's `questions.json`
(`e0d6267d`, 2147 enriched nodes, 13.1 questions each). Set sha1 `c84a634f`. Each binary built its
store with `build --no-dense` in a fresh clone, `questions.json` was copied in where the binary
reads one, and `dump --no-dense` read the set; `heldout.py compare` paired the dumps. Binaries were
built from `git archive` exports with their own `CARGO_TARGET_DIR`. Dumps and the set are kept in
`~/bench/lexical-held-out-2026-10-18/`.

The test for "reproduced" is the one that raised the issue: a lexical loss significant at 0.05 by
paired exact McNemar.

## Readings

Each binary on its own fresh store:

| before → after | with questions | passages only |
| --- | --- | --- |
| `v0.5.3` (`000ec89`) → `22d7b0a` (`main` before #161, 0.6.0) | 238 → 239 · 8 gained, 7 lost · p 1.00 | 181 → 179 · 7 gained, 9 lost · p 0.80 |
| `22d7b0a` → `main` (`2189fb9`, 0.6.4) | — (`main` reads no questions) | 179 → 179 · 2 gained, 2 lost · p 1.00 |
| `v0.5.3` → `main` | 238 → 179 · 15 gained, 74 lost · p < 0.0001 | 181 → 179 · 7 gained, 9 lost · p 0.80 |

The one significant row is #161, which removed the questions list on purpose. This set is made of
generated questions scored against their siblings in that list, so on `main` it measures the
passages alone, and the passages column is the comparison that still means something there.

The provenance, read with the `v0.5.3` binary on both sides:

| store | files | nodes | with questions | passages only |
| --- | --- | --- | --- | --- |
| built by `ad5618f` (the parent of #64) | 908 | 8,475 | 238 | 181 |
| the same store after `v0.5.3 update` | 1,194 | 9,120 | 238 · 0 gained, 0 lost | 181 · 1 gained, 1 lost |
| built by `v0.5.3` | 1,194 | 9,120 | 238 | 181 |

The update reads `changed 286 removed 0 nodes 9120`: the same 286 files and the same 9,120 nodes
the 0.6.0 bisection logged, which identifies B0's store as one built before #64. The 286 are files
the store had never read, none changed: 228 `.md`, 44 `.mjs`, 11 `.js`, 2 `.tsx` and 1 `.ts`,
237 of them under a dot-path such as `.claude/` or `.agents/`. B0's store held 8,316 nodes rather
than 8,475, so it was built earlier still; the 286 are the same.

## Why the 6 do not reappear

On today's questions the 286 files cost no lexical answer. The B0 store and the questions it was
read with were lost with `~/bench` on 2026-10-07. The fixture's `questions.json` was rebuilt on
2026-10-08 from a different enrich run, and on it the same set reads 0.595 lexical recall, not
0.283. With about twelve sibling questions per target left in the index, the questions list finds
most targets on its own, and the extra markdown passages move no answer out of the top five. The drop cannot be re-read at its original
size. Its cause can, and the cause is #64's corpus rather than any later commit, so there is
nothing to bisect.
