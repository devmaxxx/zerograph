# The widened paraphrase arm, re-authored: gemma fp32 against `:q4`

**Date:** 2026-10-18 · **Fixture:** `~/bench/beauty-crm-502e8a6d`, one `cp -c` clone per export,
vectors removed and re-embedded · **Stack:** unenriched, no rerank, `balanced` resources

The 30 cases the 2026-09-22 arm added were lost with its kit, so ADR-005 shipped `:q4` with the
widened reading still owed. This run writes 30 new questions for the same 30 target ids and reads
both exports on them.

## The cases

[`bench/cases-wide-112.jsonl`](../../bench/cases-wide-112.jsonl) is the 82 recorded cases plus the
30 new ones, now in the repository so they cannot be lost again.
[`bench/paraphrase-gate.py`](../../bench/paraphrase-gate.py) admitted them: Snowball stems for
Russian and English, at most 0.34 of a question's content stems shared with its target's label and
body, and the anchor must resolve. 11 of the first drafts failed and were reworded. The third rule,
that no second node answers, was a judgement made while writing, not a check.

These texts are not the 2026-09-22 ones, so this arm's numbers do not compare with that arm's
`paraphrase60` rows. The history names it `paraphrase60b`.

## Readings

| | fp32 | `:q4` |
| --- | --- | --- |
| keyword / code | 40/40 · 12/12 | 40/40 · 12/12 |
| paraphrase, all 60 | 38/60 | **40/60** |
| the recorded 30 | 20/30 | 22/30 |
| the new 30 | 18/30 | 18/30 |
| p90 tokens | 228 | 228 |
| embed, 11,593 rows | 604.6 s¹ | 539.6 s |
| peak RSS / footprint | 1.43 GB / 0.46 GB | **0.88 GB** / 0.42 GB |

¹ A `cargo test` build ran on the same machine during fp32's embed, so its wall time is high;
the 2026-10-17 readings put the two exports level at 531.0 s and 537.4 s.

q4 gains `FR-CAL-100`, `FR-DM-66` and `FR-STAFF-45` and loses `FR-MKT-67`. On the new 30 the two
exports tie, 18 each, with the same cases hit.

## What this says

The widened arm does not show q4 below fp32: it reads two hits above on 60 cases, all of that
margin from the recorded 30, and level on the 30 nobody had tuned against. ADR-005 said a loss
beyond noise would reverse the default. There is no loss, so `:q4` stays the default.

## Reproduce

```bash
cp -c -R ~/bench/beauty-crm-502e8a6d <clone>
rm -f <clone>/.zerograph/vectors.json <clone>/.zerograph/vectors.f32
ZEROGRAPH_EMBED_MODEL=<id> ZEROGRAPH_RESOURCES=balanced ZEROGRAPH_NO_SERVE=1 zerograph --repo <clone> embed
ZEROGRAPH_EMBED_MODEL=<id> ZEROGRAPH_NO_SERVE=1 zerograph --repo <clone> bench --cases bench/cases-wide-112.jsonl
```

`<id>` is `onnx-community/embeddinggemma-300m-ONNX` or the same with `:q4`.
