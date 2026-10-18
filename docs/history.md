# The measurements this tool was built on

Everything here was in `README.md` until 2026-09-10, when the README was cut to what a reader — or
an agent — needs in order to *use* zerograph. None of it is deleted, because every number in it
paid for a decision that is still in the code: which model is the default, which list joins the
fusion, which floors `bench` grades against, why `--rerank` is opt-in and ungraded.

The narrative order is the order the README carried, and each section keeps its own links to the
results documents under `docs/bench/` and the amendments under `docs/adr/`.

## The embedder weighed, and the two open figures

Measured on the fixture,
`intfloat/multilingual-e5-large` reads paraphrase **22/30** against the default's 15/30 with
keyword 40/40 and code 12/12 unchanged, and held-out 103 → 119 of 400 (+19 −3, p = 0.0009). What
that recall costs is the rest of this section: an `ask` in 0.8 s against 0.30 s (the model opens in
676 ms against 220), 1.9 GB resident against 1.7, a 2.1 GB download against 470 MB, and 1,930 s to
embed the fixture's 33,525 rows against 266 s at the default `resources = "balanced"`
([2026-09-09](bench/2026-09-09-normal-band-only-results.md)): a first build is half an hour
under the large model where the default's is four minutes. The large model is one
line and one `zerograph embed` away, and a store already on it keeps answering by it whatever this
file says afterwards; [ADR-002](adr/ADR-002-two-defaults-multiplied.md) weighs the two and
says why the cheaper one is the default.

The two open figures are not the same measurement twice. The small model's fell from 418 ms to
220 when the cache lookup stopped asking the hub for a weights file it has never had and waiting
out the 404; the large model's 676 ms is untouched by that, because its weights genuinely do live
beside its graph and the lookup was always a cache hit. The saving is the default's alone, and the
large model pays what it always paid.

## What turning the dense stage off costs

Turning the dense stage off altogether is the step below that, and what it costs depends on which
model it replaces. The lexical lists do not know what is configured, so `--no-dense` reads keyword
39/40, paraphrase 14/30, code 12/12 on the fixture's enriched store either way. Against the
default's 40/40, 15/30, 12/12 that is two hits of eighty-two, for a 470 MB download and ~0.2 s an
`ask` saved; against `e5-large`'s 40/40, 22/30, 12/12 it is nine, for 2.1 GB and ~0.7 s. The dense
stage earns its keep in proportion to the model behind it: on the default it is worth one
paraphrase and one keyword, which is why the model and the `--no-dense` switch are one decision
rather than two.

The floors in [Bench](benchmarks.md#bench) are keyed by the model the store's rows were written with, since
0.5.0. Only the two dense arms depend on the embedder at all, and `e5-large` has floors of its own
there too, measured on a copy of the fixture re-embedded under it and read twice per arm:
`keyword 40/40  paraphrase 22/30  code 12/12  p90 224` with `enrich`'s questions and
`40/40  17/30  12/12  p90 227` without. A store whose rows were written by any other model is
measured and never graded — the summary line says `model=<name>` and `gated=false`. The numbers and
how they were read are in [the 0.5.0 gap results](bench/2026-09-05-0.5.0-gaps-results.md).

## Five embedding-side levers, all rejected

Five embedding-side levers were measured on the same corpus and cases — on the fourteen-case set,
and before `enrich`'s generated questions were in the index — and none moved recall past 6/14: the
larger `MultilingualE5Base` (768-d, ≈1.1 GB, 2.4× the download) scores 6/14 with a different hit
set; `BGEM3` (1024-d, ≈2.1 GB) scores 5/14 at eleven times the embedding time (1,454 s against
132 s); the quantized `ParaphraseMLMiniLML12V2Q` scores 2/14 and drops keyword to 21/24; raising
the passage cut from 256 to 512 tokens scores 5/14 at double the embedding time; a second vector
per node for the label alone, max-scored against the passage vector, scores 6/14 at 1.85× the
embedding time. What none of them had was size: re-measured with the questions in the index, which
is the paragraph above, `e5-large` reads paraphrase 22/30 against 15/30 — as the one line that
buys it and not as the default, for the reasons in
[ADR-002](adr/ADR-002-two-defaults-multiplied.md). The passage cut stays at 256 tokens and the
node keeps one vector.

## The first embedding pass, and what a rebuild reuses

On `beauty-crm`'s 6,700 non-`File` nodes, the first embedding pass takes ~103 s on an M3 Pro — rows are
batched by length, so a ten-token label no longer pads out to a 256-token batch (191 s before that,
same vectors to six decimals); a second `update` with nothing changed embeds 0 — only nodes whose
passage hash changed are re-embedded. `build` drops the graph and the manifest and nothing else: the
vectors are reused by content hash and the questions cost tokens, so neither is paid for twice.

## `enrich --code`: the fifth list, and why it stays out of the plain fusion

`enrich --code` extends the pass to code: symbols with a doc comment or a body of their own and
files with a head comment — 3,475 nodes on the corpus, 290 batches — through a prompt of its own
that asks four Russian and four English questions per node and forbids repeating the identifier: a
developer's question is «где проверяется, что запрос принадлежит нужному бизнесу», not
`TenantContextInterceptor`. Entries carry a `c<n>` key in the prompt because the model, asked to
copy a `sym:apps/api/src/…::AvailabilityService` id, copies its label instead — half the batches
came back without a usable line before the key. The code questions are an index of their own,
searched for the `--rerank` pool and nowhere else: the plain `ask` fusion is byte for byte the
fusion of a store without them, a symbol's passage row stays its declaring line, and a file is a
passage nowhere (its head comment in the passage index moved the BM25 statistics against paraphrase,
15/30 → 13/30 on 2026-09-05) and is present through its questions alone. Why they are kept out of
the plain fusion is measured, in three steps: inside the documents' questions index they lifted that
index's average length until the gate admitted it over the passage that held the answer (keyword
39/40 → 38/40 without embeddings); as a list of their own admitted last on the same 0.85 gate they
took two document answers off that same arm's recorded suite, the arm with embeddings unchanged; and
capped at a single seat they read `where` 0/9 → 2/9 on the developer suite and still lost six of 400
held-out questions in each arm, gaining none (p = 0.031). That seat was measured once more under
the coverage admission at a constant of the code list's own — 0.902, the crossover of 800 held-out
questions, half the fixture's document set and half drawn from a code-enriched copy's 3,463 code
entries — and refused again for a different reason: it reads `where` 0/9 → 2/9 in both arms and
takes the code held-out set from 54/400 to 122/400 and 40/400 to 115/400 (p = 0.0000), and it now
clears the held-out clause that stopped it before (five document questions lost per arm, none
gained, p = 0.0625), but it costs one recorded paraphrase case in the lexical arm — the code seed
takes a slot and pushes off the fifth seed the answer was reached from. A store without code
questions is therefore the old index byte for byte; `coverage` still counts documents, so the
floors grade the same store the same way, and the summary line reports `code_questions=` beside it.
What the questions buy, and what a seat for them costs, is measured in
[the developer-questions results](bench/2026-09-05-dev-cases-results.md) and, under the
coverage form, in
[the residue, the seat and the register](bench/2026-09-06-residue-seat-register-results.md).

## Three ways to spend the generated questions, and the one that ships

Two ways of spending the documents' generated questions were measured and rejected: as extra dense
rows pooled with the passages they bury targets (a passage at rank 2 fell to 87 behind other nodes'
questions), and mixed into a node's own BM25 text they cost a keyword hit. What ships for those
questions is the third — a BM25 list of their own, which the plain `ask` fuses alongside the passage
list — when that list has earned its turn. Each list is asked what fraction of the question its
best document actually reached: `best / attainable`, where `attainable` is the idf of every term
the query asked for, priced in that index — a term the index never saw charged at the idf BM25
gives `df = 0`, so a word a list cannot answer lowers its coverage instead of leaving its
denominator. Those fractions are dimensionless, so the two lists compare in one unit whatever
their raw scores are worth, and the questions list joins the fusion when its coverage reaches
0.761 of the passage list's. That constant is the crossover of 400 held-out generated questions in
the `--no-dense` arm. It was re-derived on that same set when the denominator began charging every
term and returned 0.761 again, so the number did not move, but it belongs to the form that ships —
[the residue, the seat and the register](bench/2026-09-06-residue-seat-register-results.md)
and [ADR-001, Amendment 9](adr/ADR-001-paraphrase-recall-was-a-prediction.md). Until that
change an absent term left the sum, and a list's coverage rose with every query term its index
lacked; charging it moved no count in any of the four recorded arms and no case's verdict in
either suite. Until 0.5.0 the admission compared the two raw bests
at a ratio of 0.85, which moved with enrichment coverage and questions per node and was a constant
of one store rather than of BM25 — gaps G8 and G12, closed by
[the coverage admission results](bench/2026-09-06-coverage-admission-results.md) and recorded
in [ADR-001, Amendment 8](adr/ADR-001-paraphrase-recall-was-a-prediction.md). The change reads
paraphrase 15/30 against the ratio's 14/30 in the lexical arm, reproduces the dense arm's four
counts, raises both developer totals and gains one held-out question with embeddings and three
without, neither significantly. Before any admission at all,
an equal turn cost the `--no-dense` arm two exact keyword seeds, 39/40 raw
against 37/40 enriched; with it that arm reads 39/40 either way, paraphrase 7/30 raw against 14/30
enriched, and the held-out set moved by 5 gained and 7 lost, exact McNemar p = 0.77. The arm with
embeddings was 40/40 throughout. (An older reading on the 14-case set — 6/14 paraphrase with the
questions and without — is what this section used to cite for the claim that the plain `ask` ignores
them; it does not.) The questions also carry targets into a deeper candidate pool for `--rerank`:
with them, all six reachable paraphrase misses of that older set sat within the top 100 fused
candidates; without them, two did not.

## What the reranker is shown, and every model read in that seat

What the model is shown decides more than which model it is. Shown titles only, haiku, sonnet and
opus all read 10–11/14 whatever the depth, and a deeper pool made haiku worse; and because a
title is not evidence, the fused top two had to stay pinned ahead of the model's picks or it
dropped a keyword hit. Shown 120 characters of text, sonnet at depth 200 reads 13/14 with the two
pins and 14/14 without them — the pins were the retrievers' guess taking two of the model's five
slots. Haiku with the same prompt reads 11/14; opus 14/14 on paraphrase but 23/24 on keyword, in
two runs of two. Measured on the 41 cases then recorded (`bench --rerank`, one full run each unless
stated; input tokens are the answering model's own, median over the 38 questions) — all but the
last row, which is the 82 cases recorded since. That row's prompt is **metered in bytes** by `bench`
itself, over the thirty paraphrase prompts of
[the rerank diagnostics](bench/2026-09-09-rerank-diagnostics.md): the transport this row was
run through returns the picked ids and no usage block, so the byte figure is a count and the token
figure derived from it is a floor:

|                                                | paraphrase | keyword | code | p90 tokens | model tokens per question | latency per question |
| ---------------------------------------------- | ---------- | ------- | ---- | ---------- | ------------------------- | -------------------- |
| `ask`                                          | 7/14       | 24/24   | 3/3  | 216        | 0                         | ~0.30 s              |
| `--rerank`, haiku, depth 100, titles           | 10/14      | 24/24   | 3/3  | 222        | ≈4,600                    | ~3.5 s               |
| `--rerank`, haiku, depth 100                   | 11/14      | 24/24   | 3/3  | 222        | ≈9,500                    | ~4 s                 |
| `--rerank`, sonnet, depth 100                  | 13/14      | 24/24   | 3/3  | 222        | ≈10,900                   | ~4 s                 |
| `--rerank`, sonnet, depth 200 (default), 3 runs| 14/14      | 24/24   | 3/3  | 221–226    | ≈19,200                   | ~4.3 s               |
| `--rerank`, sonnet, depth 200, 82 cases, 2 runs (2026-09-09) | 29/30 | 40/40 | 12/12 | 228–231 | 58,314 B median, 60,843 B p90 — metered over 30 prompts; ≥14.6k tokens | ~4.3 s |

The last row is the 82-case set the floors are read on, not the 41 the rows above it use, so its
counts are the ones to compare against `ask`'s 15/30, 40/40, 12/12 on the same store. Against that
baseline the flag gains fourteen paraphrase cases and loses none, and both runs picked identically,
down to the one chronic miss: `FR-MKT-35`, the case the reranker has never answered. The p90
straddles the 230-token ceiling — 228 in the first run, 231 in the second, green and red on tokens
alone with every count unmoved — one more reason the arm is measured and left unfloored, rather
than an argument against it. The tokens per question are ≈14.4k, taken as bytes over four from one
captured 57.5 KB prompt: an estimate on mostly Cyrillic text, and a floor on what it costs.

Of the two later changes to what the flag builds, one is now read and the other still is not: a
symbol's 120-character snippet is its doc comment rather than its declaring line, which is what the
82-case row above measures; a store carrying `enrich --code` questions puts them into the pool as a
fifth list, which that row does not exercise — the fixture store carries no code questions, so the
fifth list was empty in both runs. The code-question pool ships unread because `--rerank` is opt-in
and on no floor.

## Why `--rerank` is measured and never floored

The `bench` floors apply to the zero-token query path in each of the two states a store can be
in — with `enrich`'s generated questions and without them, each on its own numbers, see
[Bench](benchmarks.md#bench); `--rerank` is measured, not
floored, because a model's pick can vary by one hit between identical runs — which is also why the
default is the configuration that read 14/14 three times, not the one that read it once. A
per-question query rewrite by the model was measured too — 20/24 keyword, 6/14 paraphrase,
~3,100 tokens — and rejected: the added synonyms dilute exact matches and find no new targets.

## The bench floors, case by case

**Keyword is 39, not 40, in both lexical-only arms.** `FR-PH-43` sits at passage rank 23 and no
lexical path reaches it, enriched or raw. The enriched arm read **37/40** until 2026-09-05, losing
`FR-WH-53` and `W-206` as well, because the generated-questions list took an equal turn in the
fusion on questions it had nothing to say about; the gate described under [Spending tokens on purpose](rerank.md)
put it level with the raw store. The floor moved to 39 only once it was level — at 37 it stayed 40,
because 37 was a cost enrichment itself imposed and a floor that blesses one is not a floor. The
measurement is in [G7](bench/next-version-gaps.md).

A raw store is a supported way to run the tool, not a broken one: it answers every code case and,
with embeddings, every keyword case, and `bench` passes on it. What `enrich` buys is the higher
paraphrase bar. A store below the 99% mark — `--limit`, an interrupt, a batch the model never
answered — is graded raw, since the enriched numbers describe a finished pass; the node counts on
the summary line say how far the pass got. The raw floors themselves have no headroom: unlike the
enriched paraphrase floor of 14, which has a point of slack and weeks of runs behind it, 9 and 7
are two runs on one machine on one day sitting flush on the noisiest split, and they are the first
thing to relax if they flap.

The p90 is counted as rendered UTF-8 bytes / 4 — a conservative proxy, since it counts a Cyrillic
answer at roughly double what an equivalent chars/4 reading would give a Latin one. An answer
seeded from a requirement therefore costs more than one seeded from a symbol or a task node, and
the arms differ by a few tokens at p90 according to where their seeds fall; the four sit between
220 and 226, so one ceiling covers them all.

The paraphrase cases are the noisy half, and the set was grown to narrow them: Wilson 95% on 14/30
is 0.30–0.64, against 0.27–0.73 when the same gate rested on 14 cases. It is still a wide interval,
so retrieval changes are judged on a second set:
`zerograph dump --queries qs.jsonl --out lists.json` writes,
for every question in a `{"q", "expect", "kind"}` JSONL, the four retriever lists 300 deep (dense
and BM25, over passages and over the generated questions) with their scores, the query vector,
the exact ids and the answer `ask` would give — and, for a question that is itself a stored
generated question, leaves that row out of both question indexes while it is asked. Four hundred
such held-out questions, one per node, give recall@5 a ±5-point interval and a paired exact
McNemar test against the shipped rule; that is the bar a fusion or expansion change has to clear
before the 82 real cases are consulted as the smoke test they are.

Measured, on the shipped binary against `beauty-crm` with its generated questions in the store,
two runs of each arm identical case by case: `keyword 40/40  paraphrase 15/30  code 12/12  p90 220
tok` with embeddings and `keyword 39/40  paraphrase 14/30  code 12/12  p90 215 tok` with
`--no-dense`, both arms green. A store that
`enrich` has never touched — the same store with its questions files removed — reads `keyword
40/40  paraphrase 9/30  code 12/12  p90 221 tok` with embeddings and `keyword 39/40  paraphrase
7/30  code 12/12  p90 226 tok` with `--no-dense`; the raw floors in the table above are those
numbers, measured rather than assumed. Code is floored at the whole 12 and keyword at the whole 40
with embeddings, which is why they are counts rather than fractions; only paraphrase is a fraction
wherever it is graded.

Three of the first fourteen paraphrase cases were rewritten on the way. One asked about withdrawing
consent through a messenger, while the entry it names (`FR-VIS-76`) is about who may leave a
review — «отзыв» meant a review there, not a withdrawal — so no retriever could have answered it.
Two more were under-specified rather than wrong: «export for tax reporting» names the corpus's
DAC7 tax-reporting cluster better than its target, the accountant's export (`FR-PAY-104`), and
«the product's inviolable requirements» fits the individual invariants as well as their registry
(`FR-VIS-01`); a model shown both sets chose between them at random. Each new question still
shares no word with its target line. Asking for one of the 40 keyword cases' ids verbatim returns
its head line first every time, at 68 tokens median — an exact match fills the answer alone instead
of being topped up with fused neighbours, which had cost 174 tokens for the same lookups.

The design note that shaped this architecture predicted paraphrase recall would reach ≥12/14 once
dense retrieval was fused in. It measured at 5/14, 6/14 after the fusion change and 7/14 once three ill-posed cases were
rewritten — a prediction that
did not survive contact with measurement, not a bug; see
[`docs/adr/ADR-001-paraphrase-recall-was-a-prediction.md`](adr/ADR-001-paraphrase-recall-was-a-prediction.md)
for what was ruled out and what wasn't. The floors above are that measurement, and on the 38
questions both tools were ever run against the tool still beats the incumbent on every axis anyone
has measured: 7/14 and 24/24 at 203 median tokens against graphify's 0/14 and 11/24 at 1,027-1,555
tokens, built for 14.6 million tokens instead of zero.

**`import-legacy`'s coverage note.** Folding in a graphify graph costs recall and cost at query time,
not just disk: importing `beauty-crm`'s graphify graph adds 8,577 concept nodes and 23,117 edges and
moves the recorded 82 cases from `40/15/12` to `38/16/12` — two keyword hits traded for one
paraphrase gained, below the binary's own floors
([the three-graph results](bench/2026-09-03-three-graphs-results.md)). That is why `bench`
above is always measured against a legacy-free store, and why `import-legacy` stays a separate,
opt-in step rather than folding into `build`. Two graphify nodes that resolve to the same
requirement collapse onto one node, and the edge between them is dropped rather than kept as a
self-loop; the import prints the count for whatever graph it is run against.

## The corpus as it was measured, and the prior art

A snapshot from 2026-09-02 (commit `b589ca2`), when the development corpus was a TypeScript
monorepo with a Russian-language PRD, 825 indexed files out of 3,599 tracked:

|                                 |                                                                                                                              |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Nodes                           | 7,525 — 3,640 `Symbol`, 1,880 `Requirement`, 1,001 `Task`, 825 `File`, 72 `Entity`, 69 `Milestone`, 20 `Invariant`, 18 `Adr` |
| Edges                           | 27,412                                                                                                                       |
| Graph on disk                   | 10.2 MB JSON                                                                                                                 |
| Graph load                      | ~19 ms                                                                                                                       |
| Lexical index build             | ~120 ms, paid once per context since 0.5.0 rather than once per question — a later sitting bounds that build, the question index beside it, their scoring and the fusion at ~49 ms on the bench corpus at 8.3k nodes ([the perf results](bench/2026-09-06-perf-results.md)), and a later one still reads that socket answer at 6.8 ms with the indexes kept ([the 0.5.0 gap results](bench/2026-09-05-0.5.0-gaps-results.md)); different sittings, not a before and after |
| Tokens spent building the graph and its vectors | 0                                                                                                             |

The corpus has since grown; the bench fixture the numbers below are measured on is 908 files and
about 8.3k nodes
([the 0.5.0 gap results](bench/2026-09-05-0.5.0-gaps-results.md),
[the perf results](bench/2026-09-06-perf-results.md)).

Retrieval on the recorded 82 cases against that current corpus, both arms run twice with identical results:
keyword 40/40, paraphrase 15/30, code 12/12 at 220 p90 tokens with embeddings; 39/40, 14/30, 12/12
at 215 p90 with `--no-dense`, both arms green. Those are the numbers with `enrich`'s generated
questions in the store — the one thing above that was paid for, roughly $2.5 of Haiku, once. The
lexical-only arm read **37/40** until 2026-09-05, when the gate described under
[Spending tokens on purpose](rerank.md) put it level with the raw store; the
floor it is held to is 39, which [Bench](benchmarks.md#bench) explains. The same corpus indexed and queried at
zero tokens throughout reads 40/40, 9/30, 12/12 at 221 p90 and 39/40, 7/30, 12/12 at 226;
[Bench](benchmarks.md#bench) floors each state on its own numbers.

The prior art on the same corpus was an LLM-extracted graph that cost **14.6 million input tokens
over 13 runs** — see the table at the top of this document for how it and zerograph compare on the
38 questions of the day. Cost is not the only reason to replace it, but it is the easiest one to
state.

See [Bench](benchmarks.md#bench) for the retrieval-quality floors these numbers are held to, and the ADR for the
one figure that didn't hold up on first measurement.

## What a rebuild costs the machine

### Resources

Everything above is a reader, and a reader costs a fraction of a second and the model it opened.
The one command that can take a machine over is a writer that has to embed the store whole —
`build`, `update`, `enrich`, `embed` or `watch` on a store whose rows belong to another model.
Measured on the bench fixture's 33,525 rows under `intfloat/multilingual-e5-large`, before and
after the work in [the resource-usage results](bench/2026-09-07-resource-usage-results.md):

| | wall | user | max RSS | peak CPU | threads |
| --- | --- | --- | --- | --- | --- |
| before | 2,582 s (43 min) | 13,342 s | 2.96 GB | 444% | 18, 6 running |
| after | 1,930 s (32 min) | 7,641 s | 2.15 GB | 293% | 8, 4 running |

Both rows were taken at `786b994` on 2026-09-07 in the normal band, which is the only band there
is now, so they stand as written. That is the large model because it is the worst case the tool has
and a repository opts into it: the same 33,525 rows under the default model are
**266 s and 1.45 GB**, re-measured on 2026-09-09
([the levels results](bench/2026-09-09-normal-band-only-results.md)) — the 214 s this file
used to quote here was an *uncapped* row standing in for a four-thread default, and understated it
by about a quarter.

Those are the rebuild's own numbers, and they are the only ones there are: the writers run in the
same scheduling band as everything else, on every platform, and there is no setting that names one.
There was, until 2026-09-09 — `priority = "background"` put a rebuild in the band the machine keeps
for work nobody is waiting on, and it bought a quiet keyboard genuinely well (a compile beside it
slowed 1.7% instead of 15.7%). It was removed because the price was four times the wall, paid by
everyone who ever rebuilt, to buy something only a person sitting at a loaded machine collects.
[The unnoticeable results](bench/2026-09-07-unnoticeable-results.md) are what it measured while
it existed; none of those numbers is carried forward here, because they are the band's.

Since 0.6.4 a band is back, for work nobody waits on and nothing else: a detached `update` and
`watch` ([keeping fresh](keeping-fresh.md#background-priority)); `serve` stays in the normal band,
because a question can be waiting behind its poll. The 2026-09-09
objection was to the price landing on everyone who rebuilt; a foreground writer still runs in the
normal band, and the wall a detached catch-up loses is time no command was waiting through. It
followed a repository whose hooks ran `update --no-dense` for fear of the embed loading the machine,
and whose store owed 18,787 rows that a paraphrase never found.

What a rebuild costs the person beside it is now bounded by two things only, the token budget and
one word:

| `resources` | threads here | wall | peak CPU (mean) | threads / running |
| --- | --- | --- | --- | --- |
| `"full"` | 6 | 168.8 s | 382% (335%) | 12 / 6 |
| `"balanced"` *(the default)* | 4 | 265.7 s | 275% (218%) | 8 / 4 |
| `"low"` | 2 | 358.7 s | 140% (127%) | 4 / 2 |

The fixture's 33,525 rows under the default model, on a twelve-core Apple Silicon machine
([2026-09-09](bench/2026-09-09-normal-band-only-results.md); `balanced` is the median of three
runs, and the machine was carrying other work throughout, which is recorded there). The rule is
fractions of the logical cores with one thread as the floor: `full` a half, `balanced` a third,
`low` a sixth. On four cores or fewer `balanced` and `low` meet at one thread and only `full` still
names a different amount.

That table is the 2026-09-09 rule, and `balanced` has moved since. On 2026-10-09 it became 30% of
the logical cores rather than a third — three threads here, not four — because four busy threads
are 394% at peak, over the 360% that is 30% of this machine, and nothing else lowers that: no
thread runs that the level does not count, and turning ORT's spin-wait off changes nothing. The
price is about 30% more wall on an embed. The re-measured table, per phase, is in
[Resources](configuration.md#resources) and
[the 2026-10-09 results](bench/2026-10-09-bound-build-results.md).

On 2026-10-17 the owner moved it to 40%, which is four threads here again: the wall the 30% rule
cost was judged worth more than the 94 points of CPU it kept back. A box of twelve logical cores
lands on the same count the third gave, so the four-thread rows of 2026-10-09 are its measurement.

`resources` is the only resource lever there is. `threads = N` was the escape hatch until
2026-09-09 and is gone with `priority`; **`full` is now both the most of the machine you can ask
for and the fastest setting the tool has**, which was not true while a hand-written count existed.
`full` resolves to half the logical cores rather than to no cap at all, and that is a measured
choice rather than a literal one: leaving both pools to size themselves gives ORT its six
performance cores and rayon all twelve — 18 threads, 6 running, 178.1 s — against 12 threads,
6 running and 168.8 s when rayon is capped at the same six, for the same 814 user seconds either
way. Twelve tokenizer threads queueing for six cores cost more than they add.

Set it in `zerograph.toml`, or once for the machine in `~/.config/zerograph/config.toml`, or
`ZEROGRAPH_RESOURCES=full` for one run. Readers take the level too, but it is not what it is for:
every reader but `ask --rerank-local` is under a second.

A writer also stops making its own copy of the model's weights: it reads them from the
memory-mapped file instead, which takes the anonymous memory a rebuild holds from 1.63 GB to
0.50 GB — the part the system counts when it decides what to compress, swap or kill. On a machine
with memory to spare that costs nothing; on one that is already short it costs 6–26% of the wall,
because pages the system is free to reclaim are pages it reclaims and the run reads them again.
Readers are unchanged and keep their own copies: they answer one query and leave.

The level follows the machine: `balanced`'s 40% of the logical cores is one thread on a two- to
four-core box, and on Linux `available_parallelism` honours a container's `--cpus` quota, so a
devcontainer gets 40% of what it was given rather than 40% of the host. A build server with
nobody at the keyboard wants `resources = "full"`, one line in
`~/.config/zerograph/config.toml`; a laptop you are working on wants `"low"`.

On a machine with 8 GB the model is the lever and the level is not: a rebuild under
`embed_model = "intfloat/multilingual-e5-large"` touches about 1.6 GB of weights whatever the level
says, where the default model's are 0.45 GB and the whole store is 266 s.
There is no low-memory flag, because which vectors are on disk is a property of the repository and
not of the laptop that happens to be building them.

Two things bound it, and the first of them now has a name. `resources` caps the ONNX session's
intra-op pool and rayon's global pool, which is what `tokenizers` fans a batch out over; left alone
that is 40% of the logical cores, because the runtime otherwise takes every performance core
and holds it for the length of the run. And a batch closes on a padded-token budget rather than on
a count of texts, so the largest shape the runtime ever allocates an arena for is bounded whatever
the corpus's longest passages happen to be — a count of sixty-four bounds nothing, since sixty-four
256-token passages are 16,384 tokens and sixty-four labels are 1,280.

Moving the level is a straight trade, cores against wall time, with no free side: the three rows in
the table above are 6, 4 and 2 threads for 169, 266 and 359 seconds. A long embed saves after every
1,024 rows and says where it is, so an interrupted rebuild resumes from its last checkpoint instead
of starting again:

```
dense: 16384/33525 rows, 18.7 rows/s, ~15 min left
```

The cheapest way out is the default and costs nothing to keep: `intfloat/multilingual-e5-small`
writes the whole store in 266 s and 1.45 GB where `e5-large` takes 1,930 s and 2.15 GB, and
`--no-dense` on the writer opens no model at all. `serve` and `watch` hold the model on purpose —
1.4 GB resident on the default model, about 1.8 GB on `e5-large` — and are idle between refreshes;
`ask --rerank-local` opens a second 2.1 GB session beside the embedder and is the one reader that
reaches 3+ GB.


## The head-to-head against the graph it replaces

Measured on the same corpus and the same 38 keyword/paraphrase questions against `graphify`, the
LLM-extracted graph it replaces. That head-to-head is the case set as it stood then; the recorded
set has since grown to 82 cases, which the [Bench](benchmarks.md#bench) floors are measured on:

|                           | graphify (the incumbent) | zerograph           |
| ------------------------- | ------------------------ | ------------------- |
| paraphrase questions      | 0/14                     | 7/14                |
| keyword questions         | 11/24                    | 24/24               |
| tokens per answer         | 1027-1555                | 197 median, 216 p90 |
| tokens to build the graph | 14,597,195               | 0                   |

Every number above came from running both tools; none is a target. See [Bench](benchmarks.md#bench) for the full
floor set and how it was recorded, and
[`docs/adr/ADR-001-paraphrase-recall-was-a-prediction.md`](adr/ADR-001-paraphrase-recall-was-a-prediction.md)
for the one number in this project's history that travelled from a design note into a plan as though
it had been measured, and hadn't been.


## What each stage asks of a model, and every answer measured

Three of those keys name a model and one names a directory, and they are four different jobs
rather than one preference. What each stage asks of a model, and what the answer was measured to
be:

As measured before 0.6.0, which removed `enrich` and made `haiku` the reranker's default on each
candidate's matching sentence ([ADR-004](adr/ADR-004-haiku-reranks-by-default.md)); the current keys
are in [Configuration](configuration.md).

| key | stage | what the model does there | what it has to be good at | measured | pick |
| --- | --- | --- | --- | --- | --- |
| `embed_model` | the dense index — `build`, `update`, `enrich`, `embed`, `watch` write with it, `ask` reads with what the store records | embeds every passage and every query. A sentence embedder named by its Hugging Face id, downloaded once and opened in-process through `ort`: not a command and not an LLM, so no Claude, GPT or local chat model can sit here, and an API embedder would need a transport this key does not have | putting a question and the sentence that answers it near each other, in Russian and English at once, over a 256-token passage | the default `intfloat/multilingual-e5-small` reads paraphrase 15/30 on the enriched fixture, ~0.30 s an `ask`, 470 MB on disk; `intfloat/multilingual-e5-large` reads 22/30 for ~0.8 s, 2.1 GB, and a first build measured in hours rather than minutes once the background band multiplies the embed | the small one, unless paraphrase recall is the job — [ADR-002](adr/ADR-002-two-defaults-multiplied.md) weighs the two |
| `enrich_command` + `enrich_model` | `zerograph enrich` | writes twelve everyday questions and a line of synonyms for each requirement-like node, as `id<TAB>question` lines, Russian and English together | being cheap over thousands of nodes, holding a strict line format across a batch, and asking in a reader's words rather than the document's | haiku reads paraphrase 15/30 and 5/9 of the developer suite's `rule` answers; a stronger model reads 17/30 and 2/9 — the register the questions are written in beats the model that writes them ([the G14 diagnostic](bench/2026-09-06-g14-second-diagnostic.md)). 1,971 eligible nodes cost ~$2.5 and 16 minutes at 8-way parallelism; the corpus is 1,996 nodes now | the cheapest model that keeps the format — `haiku` |
| `rerank_command` + `rerank_model` | `ask --rerank` | picks up to five ids out of a 200-deep pool it is shown as `id<TAB>title — 120 characters` | reading a mostly Cyrillic prompt of near-duplicate candidates — **median 58,314 bytes, p90 60,843**, metered over thirty prompts — and answering with ids and nothing else | sonnet on the 82 recorded cases, two runs on 2026-09-09: paraphrase 29/30 both times, keyword 40/40, code 12/12, p90 228 and 231 tokens, ~4.3 s a question. The prompt is measured in **bytes**, because `claude -p --output-format text` returns the picked ids and no usage block; bytes ÷ 4 is ≈14.6k tokens and is a floor on this corpus, where a Cyrillic character is two bytes — so ≈$0.03 a question at Sonnet 5's $2 per million is a floor too. On the older 41-case pool haiku read 11/14 and opus 14/14 paraphrase but 23/24 keyword | `sonnet`: opus buys nothing and costs a keyword hit, haiku loses three paraphrases |
| `reranker_dir` | `ask --rerank-local` | scores the same pool with a local cross-encoder instead of a model command, at zero tokens | the same pick, without a network or an account | measured and rejected as a floor candidate on 2026-09-04: 17.9 seconds a question against a bar of one, keyword 39/40 | not this, unless tokens are impossible |


## Id families: why they are derived and not configured

A default would have been the alternative, and a default is the answer for a repository about which
nothing is known — 49 families read off `beauty-crm` were never that answer for anybody else's
corpus. A list computed once and pinned beside the graph was the other, and it would have been a
second place saying what the documents already say, out of step the first day somebody wrote a
family down without recomputing it.


## What the rule read on this project's own corpus

On this project's own development corpus the rule reads 54 id families and 5 milestone families
where the list named 49 and 7. Nineteen of them the list never had — twelve `OP-<AREA>`
open-question families, and `HT`, `I`, `P`, `T`, `C`, `W0B` — each defined by a heading like
`### OP-AI-01 · Может ли салон…` that nobody had thought to configure; thirteen the list had are
cited and never defined, `OQ`, `IDEA` and `PREP` among them, and are now text. The rebuild added
159 nodes and dropped 2,238 edges that pointed at ids no document declares. Of the 82 recorded
bench cases exactly one moved: a paraphrase the lexical arm now reaches, 16/30 against 15/30.


## The resident answer, measured

Measured on the bench corpus (908 files, 8.3k nodes, enriched), median of eleven, socket and
in-process runs interleaved in one sitting, except where noted:

| | resident | one process |
| --- | --- | --- |
| fused question, dense | 66 ms | 326 ms |
| lexical, indexes rebuilt per question | 54 ms | 106 ms |
| lexical, indexes built once and kept (0.5.0) | **6.8 ms**\* | not re-measured |

\* A different sitting, median of 33 (three blocks of eleven, not the row above's eleven) against a
pre-change base, two fix commits before the binary that shipped. The shipped binary's own socket
median is 6.5 ms.

The first question after a start still pays the model open — 0.26 s, against 0.07 s for the ones
after it. What is left is a process start (5 ms), the socket round trip and the answer itself. The
BM25 indexes are no longer part of that: until 0.5.0 `ask` rebuilt them from the graph on every
question, which was almost all of the remaining 49 ms of the lexical arm's 54 and the one expensive
thing a resident process did not keep. A resident context now builds them on its first fusing answer
and keeps them until the context takes up a moved store, and that arm reads 55.0 ms against 6.8 ms —
medians of 33 over base and head binaries alternating in one sitting, base spread 0.9 ms, which is
the 30 ms this was aimed at. That is its own sitting on the same copy rather than a before-and-after
of the table above, and the one-process column was not re-measured. The stage tables, the levers
behind these numbers and the evidence that the bytes do not move are in
[the perf results](bench/2026-09-06-perf-results.md) and
[the 0.5.0 gap results](bench/2026-09-05-0.5.0-gaps-results.md).


## The socket path, and the same bytes either way

The socket path is limited to 108 bytes on Linux and Windows and 104 on macOS; the bytes are UTF-8,
so a Cyrillic user name costs two a letter, and
`C:\Users\Максим\OneDrive - <company>\Documents\projects\beauty-crm\.zerograph\serve.sock` is about
100 of the 107 a path may use. A repository deep enough to exceed it cannot start `serve`, and `ask` answers in its own process as it would with no
server at all. On a store neither `enrich` nor `embed` has moved under it, and a server of this
build serving the arm asked for, the answer is the same bytes either way; that is checked on all
142 recorded and developer bench questions, in every pairing of the server's arm with the
client's.

