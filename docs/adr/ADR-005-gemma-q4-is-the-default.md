# ADR-005 · the default embedder reads embeddinggemma-300m's q4 export

**Status:** Accepted, 2026-10-18. Amends ADR-003: the model stays, the export changes.

## Context

ADR-003 made `onnx-community/embeddinggemma-300m-ONNX` the default, and every first build paid
its fp32 export: a 1,235 MB download before the first vector. 0.6.4 added `<hub id>:<variant>`, and
[the readings of its exports](../bench/2026-10-17-gemma-quantized-results.md) put `:q4` beside
fp32 on the bench fixture, unenriched, no rerank:

| | fp32 | `:q4` |
| --- | --- | --- |
| download | 1,235 MB | **197 MB** |
| keyword / paraphrase / code | 40/40 · 20/30 · 12/12 | 40/40 · 22/30 · 12/12 |
| whole store embedded, 11,593 rows | 531.0 s | 537.4 s |
| peak RSS / footprint | 1.47 GB / 0.45 GB | **0.91 GB** / 0.42 GB |

The readings recommended keeping fp32 until the widened 60-case paraphrase arm could be read
again, because 30 cases cannot tell a two-hit gain from a two-hit loss. That arm's cases are still
missing.

## Decision

**`DEFAULT_MODEL` is `onnx-community/embeddinggemma-300m-ONNX:q4`.** The download is the cost a
first build pays before anyone knows whether they need dense recall, and q4 cuts it to a sixth at
fp32's wall time, with no case lost on the suite that exists. Waiting for the widened arm protects
against a loss nothing has shown.

`GEMMA` names the fp32 export. It keeps the catalogue's control row, the default is quoted at its
figures, and the trust check treats the default's spelling as trusted, since the export has no row
of its own. `bench` grades either export against `Floors::Gemma`.

## Consequences

- A store fp32 gemma already wrote, in a repository that names no `embed_model`, stays on fp32:
  moving it would cost a 197 MB download and one whole re-embed for nothing the suite can measure,
  since its user has already paid the download q4 saves. Naming
  `embed_model = "onnx-community/embeddinggemma-300m-ONNX:q4"` moves it. A store the small model
  wrote still moves to the default, now q4, as ADR-003 set out.
- The widened arm, written again for the same 30 targets, reads q4 40/60 against fp32 38/60
  and level on the 30 new cases ([readings](../bench/2026-10-18-widened-arm-q4.md)). It shows
  no loss, so the decision stands.
- The licence is unchanged: q4 is the same model's weights under Gemma's terms.
