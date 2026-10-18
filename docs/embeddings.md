# Embeddings

Dense retrieval embeds by default with `onnx-community/embeddinggemma-300m-ONNX:q4`, the 4-bit
export of embeddinggemma-300m (768-d, ONNX, ≈200 MB on disk, downloaded on first use) run through `ort` directly: the tokenizer and the session open concurrently at optimisation
level 1,
which halves model-open time against the library default. The files are a one-time Hugging Face
download cached under `FASTEMBED_CACHE_DIR` if that is set, else `~/.cache/zerograph/fastembed`
(`%USERPROFILE%\.cache\zerograph\fastembed` on Windows; the layout is the hub client's, so a
cache populated by an earlier release is reused as is — where the client links each file into its
snapshot, or on a Windows account without symlink rights moves it there). Every
command that touches the dense stage — `build`, `update`, `embed`, `watch`, `ask`,
`bench`, `dump`, `serve` — reuses the cache; there are no further network calls once it is
populated. `--no-dense` skips the download and the embedding stage everywhere.

The catalogue's models are fetched at the hub commit their numbers were measured on, not at
whatever the model's `main` holds today: a model id is a branch its owner can push new weights to,
and `ort` would parse them on the next cold cache. A cache filled by an earlier release at that
commit is adopted without a download. A model trusted by name with `zerograph model <hub id>` has no
measured commit and follows `main`.

The model is a property of the store. `embed_model` names what `build`, `update`, `embed`
and `watch` write vectors with; `vectors.json` records it, and `ask`, `bench` and `dump` open the
recorded one — so a store keeps answering with the model that wrote it whatever the configuration
says today, and a new default never silently reinterprets an index nobody re-embedded. The
recorded name is a clone's text like any other, so it passes the same gate as a model
`zerograph.toml` names (below): one that is neither catalogued nor trusted on this machine is not
opened: readers answer lexical-only and say why, `dump` and a dense `bench` fail, and no reader
re-embeds the store under another model. A store written before the field existed is
the small model's. `zerograph model <hub id>` switches it —
[below](#choosing-the-model) — and rows another model wrote are dropped and the file rewritten, on
width as well as on name. Naming a model in `zerograph.toml` by hand is enough: the first `ask`
after it says so on stderr, answers from the stored vectors, and starts a background `update` that
downloads the model and re-embeds. That holds for the catalogue's models; any other hub id is a
model a cloned repository would have this machine download and load, so the file's word is not
enough for it: `zerograph model <hub id>` trusts it on this machine (in `trusted-models` beside the
machine config), and until then the default stands and stderr says why.
`ZEROGRAPH_EMBED_MODEL=<hub id>` outranks both for one command, which is how a copy of a store is
measured under a second model — query that copy with `ask --stale`, or with `bench` and `dump`,
which read the store as it stands; a refreshing `ask` would claim the index for the overriding
model. It is trap 7 of the [runbook](bench/runbook.md).

`--no-dense` turns the dense stage off everywhere; on the default model that costs two hits of
eighty-two ([the arithmetic](history.md#what-turning-the-dense-stage-off-costs)).

`ask` opens the model only when a fused query needs it, and that open is most of what a fused
answer costs: ~0.30 s and ~1.7 GB on the default — the model, not
the graph. An exact-id lookup answers in ~50 ms and ~50 MB, a `--no-dense` question in ~0.1 s, since
neither opens the model or reads the vectors. What is left is paid once per process, which is what
[`serve`](serve.md) is for. `ZEROGRAPH_TIMING=1` prints the stages.

Five other embedding-side levers were measured on the fourteen-case set, before the generated
questions were in the index, and none moved recall — a larger base model, BGE-M3, a quantized
MiniLM, a 512-token passage cut, a second vector per node
([the readings](history.md#five-embedding-side-levers-all-rejected)). The passage cut stays at
256 tokens and a node keeps one vector; the two models in that list were read again with the
questions in the index, in the table below.

If the model cannot be opened (no cache, no network) the two kinds of caller degrade differently on
purpose: `ask` and `update` fall back to lexical-only, say so on stderr and exit 0, while `bench` in
dense mode fails outright — its only output is an exit code, and a silent fallback graded against
the weaker no-dense floor would report green without having measured what it claims to.

Embedding times, and what a rebuild reuses rather than pays for twice, are in
[the measurements](history.md#the-first-embedding-pass-and-what-a-rebuild-reuses).

### Choosing the model

Nine models were embedded over the same corpus and read on the same 82 cases — one run each, the
whole store re-embedded per model. Keyword and code read 40/40 and 12/12 for every model but the
smallest, which drops one keyword case, so paraphrase is the column the embedder moves. Three of
the nine are offered here; [the readings](bench/2026-09-22-embedders-results.md) hold the rest.

| hub id | dim | paraphrase | embed | vectors | licence |
| --- | --- | --- | --- | --- | --- |
| `onnx-community/embeddinggemma-300m-ONNX` (default as `:q4`) | 768 | **21/30** | 1.0× | 103 MB | Gemma |
| `intfloat/multilingual-e5-small` | 384 | 15/30 | 0.2× | 51 MB | MIT |
| `Snowflake/snowflake-arctic-embed-l-v2.0` | 1024 | **21/30** | 2.7× | 137 MB | Apache-2.0 |

The default is the cheapest of the four models that read 21/30, read through its `:q4` export and
quoted at fp32's figures ([ADR-005](adr/ADR-005-gemma-q4-is-the-default.md)); the small model it
replaced reads 15/30. A reader answers with the model that wrote the store, but a writer embeds with the
configured one: a store the small model wrote, in a repository that names no `embed_model`, moves
to the default at its next `update` — one 197 MB download and one whole re-embed, in the background
band, and so does a store gemma's fp32 export wrote. Pin `embed_model = "intfloat/multilingual-e5-small"`
to stay on the small model, or `embed_model = "onnx-community/embeddinggemma-300m-ONNX"` to stay on fp32.
`Snowflake/snowflake-arctic-embed-l-v2.0` is the upgrade, and the choice under an OSI licence: a
widened arm of 60 paraphrase cases separated the four, and it took 39/60 against the default's
35/60, for 2.7× the embed and 137 MB of vectors where the default writes 103 MB. The conditions, the other seven candidates, the flips and
the caveats are in [the readings](bench/2026-09-22-embedders-results.md).

`zerograph model` prints that table with the store's own model marked, and
`zerograph model <hub id>` changes it:

```console
$ zerograph model Snowflake/snowflake-arctic-embed-l-v2.0
model: intfloat/multilingual-e5-small → Snowflake/snowflake-arctic-embed-l-v2.0
model: 2.1G of files in the hub cache, fetched once
model: 2.7× the default's embed — 1944 s for the bench fixture's 33,525 rows
model: 137 MB of vectors for that corpus, against the other model's 51 MB on it
model: 384-d → 1024-d, so every row is re-embedded and the whole index rewritten, not extended
model: opened in 5.2s, 1024-d vectors
model: embed_model = "Snowflake/snowflake-arctic-embed-l-v2.0" in /repo/zerograph.toml
embedded 33525/33525 (14.5 rows/s, ~0 s left)
dense: embedded 33525 rows in 2317.4s
```

The model is opened before anything is written, so an id with no ONNX export fails with the hub's
own error and leaves the repository exactly as it was. `zerograph.toml` is then rewritten in place —
only that one value, comments and everything else kept — and the store re-embedded, which
`--no-embed` stops if you would rather pay for it later.

A hub id followed by `:<variant>` reads another export from the same repository and commit:
`onnx-community/embeddinggemma-300m-ONNX:q4` opens `onnx/model_q4.onnx` in place of
`onnx/model.onnx`, with the model's own prefixes and pooling. Variants follow onnx-community's file
names, so `quantized`, `fp16`, `q4` and `q4f16` are the usual ones. A variant writes different
vectors, so the store records it as a model of its own, and switching to or from it re-embeds every
row once. Only the default's `:q4` is trusted as is; any other variant is not catalogued, so
`zerograph model <hub id>:<variant>` trusts it on the machine before `zerograph.toml` may name it.
Measured on gemma, every export recalls what fp32 does and none embeds faster on Apple Silicon.
`:q4` is a 197 MB download against 1.2 GB, with 0.91 GB of RSS against 1.47 GB, at the same wall
time, which is why it is the default
([the readings](bench/2026-10-17-gemma-quantized-results.md)).

### Licence

The default's weights are published under the [Gemma Terms of Use](https://ai.google.dev/gemma/terms),
not an OSI licence: they carry a prohibited-use policy that Google may update, and anyone who
redistributes the weights passes those terms on. zerograph does not redistribute them — the files
are fetched from the Hugging Face hub onto the machine that runs it, so whoever runs zerograph is
the one taking the terms. The default stays on gemma because it reads arctic's recall on the
recorded arm for under two fifths of its embed, at 1.3 GB peak RSS ([ADR-003](adr/ADR-003-gemma-is-the-default.md)).

A project whose policy admits only OSI licences pins another model in `zerograph.toml`:
`Snowflake/snowflake-arctic-embed-l-v2.0` (Apache-2.0) for the same or better recall, or
`intfloat/multilingual-e5-small` (MIT) for the cheapest embed.
