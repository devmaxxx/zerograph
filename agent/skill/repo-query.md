---
name: repo-query
description: Use when you need to find where something lives, what calls it, or what a change would break in a repository that carries a zerograph index — before grepping for a concept or reading files to orient yourself.
---

# Asking the graph

The index answers by meaning at no model tokens. It is not a replacement for grep: grep is right
for a literal, the graph is right for a concept, and the two questions look different.

## The loop

1. **Anchor.** If you know an identifier — a symbol, a document id — ask that: `{{command}} ask
   withTenant`, `{{command}} ask FR-PAY-22`. The exact match answers in about 60 ms and 30 tokens.
2. **Words.** Otherwise ask in the words a person would use: `{{command}} ask отмена записи`,
   `{{command}} ask cancellation policy`. Four words beat one; seven do not beat four.
3. **Widen once.** A thin answer takes `--seeds 8` or a second phrasing, not a different tool.
4. **Read only what it printed.** Every line carries `path:line`. Open those; do not re-derive them
   by reading directories.
5. **Spend a model only on a miss.** `ask --rerank` re-picks the seeds with a model from a 200-deep
   pool: ~4 s and about $0.03 a question, and it reads paraphrase 29/30 where the free path reads
   15/30. It is for the question that came back wrong, not for the first question.
6. **Cite or say "not found".** Every claim about the code names a `path:line` an answer printed
   and you opened. If no answer holds it — after a rephrase, and a rerank — say the repository
   does not show it. Never fill the gap from a neighbour's name.

## Which command answers which question

| The question | The command | What it costs |
| --- | --- | --- |
| where is this concept written | `ask <words>` | ~80–250 tokens of output, no model |
| what is this id, and what is next to it | `ask <ID>` or `explain <id>` | ~30 tokens |
| who calls this symbol | `impact <Symbol> --depth 1` | one screen; `--depth 3` for a blast radius |
| what does my diff touch | `changes --base main` | a risk word and the callers reached |
| is there a path from A to B | `trace <A> <B>` | a chain, or that there is none |
| which families do the docs define | `families` | a table |
| a literal string, an env var, a flag | `rg` | the graph does not index literals |

Every one takes `--json`, and the objects are documented in the README's *Answering a program
instead of a person*.

## Freshness is not your problem

`ask`, `impact`, `trace` and `changes` bring the index in line with the working tree before they
answer — an edited file is re-extracted in milliseconds. Pass `--stale` when you want the answer
now and can live one edit behind. Nothing here needs a build step in your loop; `{{command}} build` is
for a repository that has no index at all.

## Verify before you believe

The graph is extracted, not inferred, but an answer can still be about the wrong thing. Two habits
cost nothing: check that a seed's `path:line` really contains what you were told, and prefer
`impact` over your memory of who calls something — the graph sees a caller that imported through a
barrel, and you will not.

## What costs money, and when

One stage is opt-in and everything else is free: `ask --rerank` spends a model per question, as
above. Which model it runs is a config key — `rerank_model` — set on the machine, not in the
repository.
