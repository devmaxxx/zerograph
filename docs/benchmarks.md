# Benchmarks

## Bench

`zerograph bench [--cases file]` runs the recorded 82 cases (40 keyword + 30 paraphrase + 12 code)
against a built graph and fails the process if any floor is missed. The recorded `bench/cases.jsonl`
is compiled into the binary, so a release build benches from any directory; `--cases` substitutes
any other file — one of the recorded 40/30/12 shape is graded against the floors below, any other
shape is measured and reported with `gated=false`, the way `bench --cases bench/dev-cases.jsonl` is
used throughout [the runbook](bench/runbook.md).

Twelve of the cases expect a path, and a path is a fact about one checkout, so the recorded suite
names the corpus it was written against: `bench/cases.pin` holds `beauty-crm 502e8a6d`, and a run
prints it above the cases (`suite: built-in bench/cases.jsonl, recorded against beauty-crm
502e8a6d`). The floors below are counts on that tree, and they grade that tree only: a checkout
at any other commit is measured, printed and not graded (`suite: checkout a3bf96ff is not the pin,
so this run is measured and not graded`, `gated=false`, exit 0), because beauty-crm `a3bf96ff`
reads paraphrase 12/30 against the pin's 15 after 203 documents changed, and that is the corpus
growing, not zerograph failing. A tree whose commit git cannot read is graded as before. An anchor the graph does not hold stops the
run and the refusal quotes the pin, because editing the case to a newer path is what silently
moves the suite off the tree its floors were counted on; `bench/missing-anchors.py <checkout>`
lists every path anchor a tree is missing, without a build and without CI. The one exception is a
`code` case whose symbol moved: its question is the symbol's name, so when exactly one file defines
that name now, the anchor follows it and the run says so (`suite: "staleClaims" moved:
tools/tasks/src/cli.ts -> packages/task-sync/src/cli.ts`). An id anchor is graded on the node it
names, never on another node declared in the same document: `ADR-024` counts when the ADR's own
node is reached, not when `entity:StaffMember.role_id`, which the ADR defines, is.

The dense floors are keyed by the store's embedder, since a floor measured on one model says nothing
about another: `bench` names the model on its summary line (`dense=true  model=gemma`), rows under
the default model (`embeddinggemma-300m`, printed `model=gemma`) are graded against its own numbers,
small-model rows (or rows under no name) against the small model's, and rows under any other model
are measured and never graded. The lexical arm has no embedder in it and keeps one floor whatever
the rows are. Until 0.6.0 the floors were two sets, one for a store `enrich` had written questions
into and one for a store without; `enrich` is gone and the raw set is the only one. Beneath it, on a line
of its own, `anchors  <kind> <reached>/<wanted> …` says how much of each answer was reached, not only
whether it was: a case that keeps its verdict and loses two of its three anchors moves that line and
nothing else. `bench --repeat N` runs the suite N times, judges every run on the floors, and prints
a median beneath them.

The exit status says which of three things happened, so a harness never has to read the sentence on
stderr: **0** — the suite answered and every floor was met; **3** — the suite answered and a floor
was missed, which is a verdict on the answers and a reading of the reader; **1** — nothing was
measured, which is an empty graph, a case file that does not parse, a built-in case set of the wrong
shape, or a dense width mismatch. The verdict is **3** and not 2 because 2 is written above the
command and says nothing about a suite: `clap` exits 2 on a usage error, and `npm`'s launcher exits
2 when no platform binary is installed. A `--repeat` run takes the worst of its runs: every run has
to meet the floors, not the median of them.

| | floor |
| --- | --- |
| keyword | 40/40 with embeddings, 39/40 with `--no-dense` |
| paraphrase, gemma rows (the default) | ≥18/30 |
| paraphrase, small-model rows | ≥9/30 |
| paraphrase, `--no-dense` (no embedder) | ≥7/30 |
| code | 12/12 |
| p90 | ≤250 tokens in every arm without `--rerank`; a reranked answer is whichever seeds the model picked (248 and 259 on two haiku runs of one store), so a reranked run is graded on its counts alone |

The gemma row was first measured on a store with no questions at 19/30 on 2026-10-08, and its floor
sits one case under that reading, the margin the enriched gemma row carried.

Where each of those floors came from, why keyword is 39 and not 40 in the lexical arms, how the p90
is counted, and the 400-question held-out set retrieval changes were cleared against before the 82 cases
are consulted, are in [the measurements](history.md#the-bench-floors-case-by-case).

## Measured

The bench fixture is 908 files and about 8.3k nodes. At 0.6.3, under the default embedder, the store
reads **keyword 40/40, paraphrase 20/30, code 12/12 at 222 p90 tokens** (19/30 at 244 at 0.6.0). At 0.6.0, under the
small model that preceded the default, it read 40/40, 9/30, 12/12 at 237 with embeddings and 39/40, 7/30, 12/12 at
238 with `--no-dense`, per case identical to the binary before the removal on the same store. Zero model tokens
throughout. What `enrich`'s questions bought before 0.6.0 removed them — one paraphrase case under
the default embedder, five under the small one — is in [the measurements](history.md).

The graph itself costs nothing to build: 7,525 nodes and 27,412 edges on the corpus of 2026-09-02,
10.2 MB on disk, ~19 ms to load, zero model tokens. The full snapshot, the prior art it replaced and
what that cost are in [the measurements](history.md#the-corpus-as-it-was-measured-and-the-prior-art).
