# The machine-wide vector cache against borrowing from siblings

**Date:** 2026-10-19 · **Corpus:** the zerograph repository at 4264436, one `git clone --no-hardlinks`
per checkout · **Model:** `intfloat/multilingual-e5-small` (384), as its `zerograph.toml` names it ·
**Binaries:** release builds of main 4264436 (borrowing) and `feat/vector-cache` ·
**Machine:** macOS, APFS, one run each, `ZEROGRAPH_NO_SERVE=1` for the builds

The cache sat in a directory of its own (`ZEROGRAPH_VECTOR_CACHE`), empty before the cold build.
The cold build is the only build that embeds every row; every later checkout reads the cache.

## First builds

| checkout | main 4264436 (borrowing) | vector cache | rows cached / embedded |
| --- | --- | --- | --- |
| cold, empty cache | 56.2 s (issue baseline) · 50.0 s (this run) | 41.9 s | 0 / 5319 |
| new worktree, same commit | 1.49 s | **1.52 s** (3.0% of cold) | 5319 / 0 |
| worktree at HEAD~30 | 7.0 s (3994 borrowed / 566 embedded) | 6.33 s | 3979 / 590 |
| independent clone | 56 s | **1.52 s** (3.0% of cold) | 5319 / 0 |

The percentages are taken against the main 4264436 (borrowing) cold build of this run (50.0 s).
Against the cache build's own cold (41.9 s) they come to 3.6%. The two cold builds embed the same 5319 rows, so the
gap between them is run-to-run variance, not the cache. The issue's target was under 10% of cold
for both the new worktree and the clone, and both meet it. Borrowing could never help the clone,
because a clone has no siblings.

At HEAD~30 the cache supplies 3979 rows and borrowing supplied 3994. The borrowing figure is the
issue's baseline, taken in an earlier run and not re-run here, so the 15-row gap was not looked
into.

After the four builds, the cache holds 5670 distinct rows in 9.1 MiB (15 MiB on disk, counting the
4096-shard allocation per model).

## Serve: edit → ask

Corpus A has two sibling worktrees, the same setup as the issue's "2 siblings" baseline. Each round
runs 15 asks after a one-line edit, then 15 without. The binaries alternate, base then cache, for
three rounds, which gives 45 asks per cell.

| ms | main 4264436 (borrowing) p50 | main 4264436 (borrowing) p90 | cache p50 | cache p90 |
| --- | --- | --- | --- | --- |
| edit → ask | 111 | 121 | 110 | 120 |
| ask, no edit | 54 | 56 | 53 | 55 |

p90 does not regress. An earlier cut of the branch weighed the whole cache after every write and
read 256 shards of 16 MiB each at the default cap. It came out at 136 ms p90 on edit → ask. The
weighing now runs only when a shard crosses a 64 KiB boundary, and the shard count is 4096. A prune
started by a write runs in a detached background process, never on the reader's path.

## Answers

`compare.py` asked the same ten questions of the cold-embedded clone and of the cache-fed clone:
the top-5 was identical 10/10, and the largest score difference was 0.

## Method

`run.sh` builds the timings. `serve.sh <bin> <repo> <label>` runs the serve rounds. `compare.py`
compares the top-5 across two checkouts. All three scripts are in the session scratchpad and were
not committed. The corpora went to `~/.Trash` afterwards.
