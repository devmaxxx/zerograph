# The default model's quantised exports, read against fp32

Machine: Apple M3 Pro, 12 logical cores (6 performance + 6 efficiency), 36 GB, macOS 25.6.
Corpus: a clone (`cp -c`) of the pinned fixture `~/bench/beauty-crm-502e8a6d` at corpus commit
`502e8a6d`, so the fixture's own store was never written: 11,593 dense rows at 768-d. Tool:
`feat/embed-tuning`, `cargo build --release`, with the `<hub id>:<variant>` spelling this branch
adds. Model: `onnx-community/embeddinggemma-300m-ONNX` at the pinned commit `5090578d`, whose
`onnx/` holds fp32 `model.onnx` and five more exports beside it.
Kit and transcripts: a session scratchpad under `/private/tmp`, which is not kept. It held `run.sh`
and, per arm, `embed-<variant>.{txt,err}` and `bench-<variant>.txt`. The figures below are copied
from those files.

Method. For each arm, `vectors.json` and `vectors.f32` were removed and
`ZEROGRAPH_EMBED_MODEL=<id> ZEROGRAPH_RESOURCES=balanced ZEROGRAPH_NO_SERVE=1 zerograph embed` ran
under `/usr/bin/time -l`, so every row was embedded at the same level: four threads, the 40% rule.
Then `bench` ran with the same override on the 82-case suite (40 keyword, 30 paraphrase, 12 code),
no rerank. **embed** is that command's whole wall clock, model open included. **RSS** is `maximum
resident set size`; **footprint** is `peak memory footprint`, the anonymous memory the system
counts under pressure. A writer reads the weights memory-mapped, so RSS carries the mapped file and
footprint does not. Every model file was downloaded beforehand, so no arm's wall includes a
download. Every row is one run.

> **The machine was not quiet.** One-minute load averages ran 6 to 8 throughout, from work this
> session did not own. fp32's 531 s here sits beside the 521 s the
> same four threads read on 2026-10-09, so the noise is a few percent, not tens.

## The table

| arm | export | download | keyword | paraphrase | flips vs fp32 | code | p90 tok | embed | RSS | footprint |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| fp32 (control) | `model.onnx` | 1,235 MB | 40/40 | 20/30 | — | 12/12 | 222 | 531.0 s | 1.47 GB | 0.45 GB |
| `:quantized` (int8) | `model_quantized.onnx` | 309 MB | 40/40 | 22/30 | +2 −0 | 12/12 | 222 | 542.0 s | 1.99 GB | 1.41 GB |
| `:q4` | `model_q4.onnx` | 197 MB | 40/40 | 22/30 | +2 −0 | 12/12 | 220 | 537.4 s | **0.91 GB** | 0.42 GB |
| `:no_gather_q4` | `model_no_gather_q4.onnx` | 195 MB | 40/40 | 21/30 | +2 −1 | 12/12 | 223 | 2101.9 s | 3.89 GB | 3.48 GB |
| `:fp16` | `model_fp16.onnx` | 617 MB | 40/40 | 20/30 | +0 −0 | 12/12 | 222 | 558.5 s | 1.61 GB | 0.89 GB |
| `:q4f16` | `model_q4f16.onnx` | 175 MB | 40/40 | 22/30 | +2 −0 | 12/12 | 220 | 635.1 s | 1.01 GB | 0.53 GB |

*download* is the export and its `_data` file, from the hub's file listing at the pinned commit; the
tokenizer and configs are shared and not counted. *flips* are the paraphrase cases the arm hits that
fp32 misses, and the reverse. Every gain is the same pair or a pair from the same three cases:
`FR-STAFF-45` in all four of the q4 and int8 arms, then `INV-16` (int8) or `FR-DM-66` (the q4 family);
`no_gather_q4` also loses `FR-TOOL-22`.

**The widened 60-case paraphrase arm was not run.** Its 30 added cases lived in
`~/bench/paraphrase-arm-2026-09-22/cases-wide-112.jsonl`, which no longer exists on this machine.
`bench/history/runs.jsonl` keeps their ids, and the case texts are not in this repository. The arm
can be read again only after those 30 cases are recovered or written again under the same gate.

## What the readings say

**Every export loads and runs under ort's CPU provider.** All six open in under 1.2 s and give
768-d vectors. The two fp16 exports still return fp32 `sentence_embedding`, so the embedder reads
them with no change.

**None is faster.** On this CPU the int8 and q4 graphs embed in the same time as fp32: 531, 537 and
542 s, within the noise. fp16 is 5% slower and q4f16 20% slower. `no_gather_q4` is **4× slower**,
2,102 s, and takes 3.48 GB of anonymous memory. Why was not profiled. The export differs from `q4`
in how it stores the 262k-token embedding table, which points at that table, but this run does not
show it. No export cut the embed's wall time.

**What a quantised export does buy is size.** q4 is a 197 MB download against 1,235 MB, and its
peak RSS is 0.91 GB against 1.47 GB, because the mapped weights it pages in are a sixth as large.
Its footprint is fp32's (0.42 GB against 0.45 GB). The int8 export goes the other way: 1.41 GB of
footprint, three times fp32's. The cause was not profiled.

**Recall does not fall, and the rise is noise.** Four arms read 22/30 against fp32's 20/30, and the
gain is `FR-STAFF-45` plus one other case each time. Two flips on one run of 30 cases are far
inside the four hits that [the embedders' readings](2026-09-22-embedders-results.md) call noise.
Keyword and code are saturated at 40/40 and 12/12 in every arm, as they are for every model measured.
What the table supports is that 4-bit weights do not cost this suite a hit. It does not support
that they add one.

## Recommendation

**The default stays fp32.** No export makes an embed cheaper in time, and time is what a first
build costs.

**`:q4` is the export to offer** to a machine short on disk or memory. It is a sixth of the
download and three fifths of the RSS, at fp32's wall time and with no paraphrase lost here. It
becomes a candidate for the default once the widened paraphrase arm can be read again: 30 cases
cannot tell a two-hit gain from a two-hit loss, and that arm is what separated the models last time.
`:quantized`, `:fp16`, `:q4f16` and `:no_gather_q4` each give something up and buy nothing back
on this CPU.

To use one: `embed_model = "onnx-community/embeddinggemma-300m-ONNX:q4"` after `zerograph model
onnx-community/embeddinggemma-300m-ONNX:q4` has trusted it on the machine. A variant is a model of
its own to the store, so switching it re-embeds every row once, like any other change of model.

## Caveats

1. One run per arm on a machine with other load. The wall times are comparable with each other
   to a few percent, as the fp32 row against 2026-10-09 shows. They are not this machine idle.
2. Apple Silicon only. An x86 CPU with VNNI or AVX-512 runs int8 `MatMulInteger` and q4
   `MatMulNBits` on paths this machine does not have, and could read differently on wall time.
3. Not enriched, no rerank, the 30-case paraphrase arm. The widened arm is the one that counts for
   the default, and it was not available.
