# Mapped weights for readers, measured and refused

Issue #222 asked whether a resident `serve` should open the embedding model with
`Weights::Mapped` instead of `Weights::Packed`, so that several servers share the model's pages
instead of each holding its own copy. The ship rule was set before measuring: two Mapped servers
must use clearly less memory than two Packed ones, **and** `ask` p50/p90 must stay within noise
(10% or the run-to-run spread, whichever is larger). The memory half holds, modestly; the
latency half fails by a factor of two. No code change ships.

Machine: Apple M3 Pro, 12 logical cores, 36 GB, macOS 25.6. The machine was not quiet: one-minute
load averages of 10 to 12 from work this session did not own, which is why the arms were
interleaved.
Tool: `main` at `17c4dcc`, `cargo build --release`. The Packed arm is that binary unchanged; the
Mapped arm is the same tree with the two `Weights::Packed` arguments in `src/ask.rs` (the cold
open and the warm thread, both reader paths) changed to `Weights::Mapped`, nothing else.
Model: the default, `onnx-community/embeddinggemma-300m-ONNX:q4` at `5090578d` — a 0.5 MB graph
with 197 MB of weights in `model_q4.onnx_data`.
Corpus: this repository at `17c4dcc`, exported with `git archive` into a scratch directory with
`embed_model` set to the default, built once (5,186 dense rows at 768-d, 200 s), then copied
whole to a second directory, so the two servers had identical stores and sockets of their own.
Kit and transcripts: a session scratchpad under `/private/tmp`, not kept; the figures below are
copied from its files.

## Method

Per run, for one binary: start `serve --every 100000 --idle 3600 --idle-model 3600` on store A,
three warm-up questions, then 32 distinct natural-language questions through `ask --stale` (every
one answered by the resident process, every one past the exact-seed path, so every one embeds a
query), timing each client invocation's wall clock. Then memory of A alone, then start a server on
store B, warm it with four questions, and take memory of both. Runs alternated Packed, Mapped,
three times each.

- **RSS** is `ps -o rss`. It counts the file pages a process has mapped, and the OS reclaims or
  compresses resident pages under pressure, so it reads high for Mapped and is unstable on a busy
  machine (Mapped run 1 caught server A at 96 MB RSS with 242 MB of footprint).
- **Footprint** is `footprint -f bytes`: one process's `phys_footprint`, and for two processes the
  `Summary Footprint`, which counts memory shared between them once. This is what memory pressure
  counts.

## Results

| run | arm | `ask` p50 | `ask` p90 | 1 server RSS | 1 server footprint | 2 servers RSS (sum) | 2 servers footprint | file pages shared |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Packed | 32.8 ms | 33.7 ms | 569 MB | 286 MB | 1,144 MB | 578 MB | 16 MB |
| 1 | Mapped | 73.5 ms | 87.2 ms | 530 MB | 223 MB | (591 MB, reclaimed) | 468 MB | 67 MB |
| 2 | Packed | 31.4 ms | 34.2 ms | 570 MB | 292 MB | 1,144 MB | 584 MB | 16 MB |
| 2 | Mapped | 73.7 ms | 81.9 ms | 534 MB | 225 MB | 1,066 MB | 452 MB | 64 MB |
| 3 | Packed | 33.8 ms | 36.9 ms | 582 MB | 297 MB | 1,156 MB | 590 MB | 19 MB |
| 3 | Mapped | 75.7 ms | 83.5 ms | 531 MB | 224 MB | 1,059 MB | 447 MB | 67 MB |

Means: Packed p50 32.7 ms, p90 34.9 ms; Mapped p50 74.3 ms, p90 84.2 ms. Two servers' footprint
584 MB Packed against 456 MB Mapped.

## Reading

**Latency fails the rule.** Packed's run-to-run spread is 31.4–33.8 ms at p50 (7%), so the bar
is 10%. Mapped is +127% at p50 and +141% at p90, in every run. This is the cost the `Weights` doc
comment predicted for a reader: without prepacking, MLAS lays out each weight on every GEMM, and
a one-query forward is nothing but those GEMMs. 40 ms is small next to a model answer, but it is
more than the whole Packed `ask`, and the rule was latency within noise.

**Memory improves, but by far less than the issue assumed.** The issue's 0.9–1.3 GB per server
is the fp32 export. Under the q4 default a warm Packed server's footprint is ~290 MB, of which the
prepacked weights are ~65 MB: Mapped saves 64 MB per server (22%) and leaves 67 MB of clean
`model_q4.onnx_data` pages shared between the two. Two servers go from 584 MB to 456 MB. The rest
of a server's footprint — allocator arenas, the store's vectors, graph and BM25 — is per-store
and does not shrink. The RSS sum barely moves (1,150 → 1,060 MB) because each process's RSS
still counts the shared file pages.

So the trade on the default model is 128 MB saved across two servers for 40 ms on every question.
It is not taken. Readers keep `Weights::Packed`; writers keep `Weights::Mapped`.

## Not measured

- The fp32 export (`embed_model = "onnx-community/embeddinggemma-300m-ONNX"`), where the issue's
  1.2 GB figure comes from. Its weights are six times q4's, so the saving per server would be
  larger, and so would the per-GEMM layout cost; with q4 now the default, fp32 is opt-in. If it
  is revisited, measure it with this method, and consider Mapped only for fp32 readers rather
  than for every model.
- More than two servers. The saving is linear in server count (64 MB each), the latency cost is
  not affected by it.
