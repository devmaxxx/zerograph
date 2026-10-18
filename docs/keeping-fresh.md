# Keeping the graph fresh

### Keeping it fresh

`ask` walks the tree before it answers. Anything edited since the last build is re-extracted in
process and saved, so the graph is never behind the working copy and no `update` has to be
remembered. One line goes to stderr when that happens:

```
refresh: 3 changed, 1 removed
```

A fused query opens the embedding model anyway, so the rows that changed are re-embedded and the
vectors stay in step too. `--no-dense` and the exact-id path open nothing: the lexical graph is
fresh, and the vectors catch up on the next fused query or `update` — after an `update --no-dense`
from a commit hook too, because `vectors.json` records the graph its rows were last synced against.
Measured on the development
corpus (825 files, 7.5k nodes): the no-change check costs ~10 ms, and a one-file edit costs ~20 ms
lexical, ~40 ms with the re-embedding — the new rows are appended to `vectors.f32` and the rows
they replace are left as holes, so nothing rewrites 50 MB to store a row of 1.5 kB. Holes past a
quarter of the live rows are compacted away by the next refresh, which does rewrite both files.
`ZEROGRAPH_TIMING=1` prints the stages.

An upgrade is the one refresh that reads everything. The manifest records which generation of the
extractor's grammar the graph beside it was read by, and where a newer generation is reading, a hash
settles nothing: a file nobody has touched may hold a citation the older grammar never looked for.
So the first `update`, `watch` poll or refreshing `ask` after such an upgrade says on stderr that it
is re-reading the whole tree and why, writes the new generation forward, and leaves every update
after it incremental again. A release that does not change the grammar costs no walk at all.

The other direction is refused. A store a newer release wrote — its manifest names a grammar past
this build's — is never re-read and never re-stamped by an older binary, such as a worktree pinned
to an earlier release: `ask` and the other reads answer from it as it stands, `update` and `build`
refuse, and each says so in one line naming both generations and the release that wrote it.
`status` shows the same. The manifest records the writing release from 0.6.5 on; a store written
before that is named by its grammar alone.

```bash
zerograph ask --stale отмена записи         # answer from the store as it stands, no check
```

`--stale` never pays for that walk and never repairs it: it answers from the store as it stands,
which is the whole of what it promises.

A reader never waits on a refresh longer than `reader_budget` (10 s by default). `ask`, `impact`,
`trace`, `changes` and `explain` estimate the refresh first — about 40 ms a changed file, 5 ms a
vector row, and then the rate the rows actually embed at — and when it does not fit, answer from the
store as it stands, say so in one stderr line, and start one detached `zerograph update` that waits
for the store's writer lock and catches it up for the next reader:

```
index: 576 files behind, refreshing in background
dense: 1683 vectors pending, answered from the stored ones, refreshing in background
```

A reader that finds a refresh already holding the lock starts nothing and answers the same way,
ending its line `a refresh is already running`. `--json` carries what the answer was given without
as a first field, present only when it is behind, so an agent can read those files itself:

```json
{"stale":{"files":["src/billing.ts","src/refund.ts"],"vectors":0},"root":"sym:…", …}
```

The detached refresh writes its output to `.zerograph/background.log`. `zerograph update --detach`
starts the same refresh by hand, and `install-agent` writes `post-merge` and `post-checkout` git
hooks that run it, so a pull or a branch switch starts catching the store up before anyone asks. An
existing hook keeps its own lines and gets the block appended between `# zerograph:begin` and
`# zerograph:end`; `core.hooksPath` is honoured. A foreground `update` or `build` prints where its
embed is every two seconds:

```
embedded 400/1683 (14.2 rows/s, ~90 s left)
```

### Borrowing from sibling worktrees

A new `git worktree` of a repository that already has a built checkout does not embed the rows that
checkout already holds. Each worktree builds its own graph, and a cold build is almost all
embedding, so before a sync embeds a missing row it looks the row's text hash up in the stores of
the other worktrees `git worktree list` names. A sibling lends only when its rows are the same
model, variant and dimension as the ones being written; only the rows no sibling holds are embedded:

```
dense: borrowed 5119 rows from /work/zerograph
```

A sibling is read under its writer lock and left untouched: one that is refreshing, has no lock
file, or holds a torn pair is skipped. A borrowed vector is the one this checkout would have
embedded up to the last digits — batch padding makes two embeds of the same text near-identical,
not bit-identical — so answers do not move. There is no switch: a repository with one checkout, or
siblings on another model, simply embeds everything.

### Background priority

The detached `update` that a reader, a git hook or the MCP `reindex` and `switch_model` tools start
runs at background priority, and so does `watch`. A command someone is waiting on — `ask`, `build`,
`update` without `--detach`, `embed`, `model` — keeps normal priority, and so does `serve`: a
question can be waiting behind its poll, and the poll re-reads only the files that changed.

| platform | what the process gets |
| --- | --- |
| macOS | the `PRIO_DARWIN_BG` band: CPU and disk I/O throttled |
| Linux | nice 19 and the idle I/O class, set before any thread starts so every pool inherits them |
| Windows | `PROCESS_MODE_BACKGROUND_BEGIN` |

The embed under it is still bounded by [`resources`](configuration.md#resources), 40% of the logical
cores by default. A platform that refuses the call costs a line in the run's log and nothing else.
The price is wall time when the machine is busy: the band waits for the cores nothing else wants,
which is the point, and a foreground `update` that finds a background one holding the store waits
for it. `ask` never waits on it: it answers from the store as it stands and says so.

A dense `update` embeds every row the store is owed, whoever left it owed: the rows are matched to
the stored graph by hash, not to what that run re-read, so one over a tree nothing moved still pays
for an `update --no-dense` or a lexical `ask` before it. A hook written as `update --no-dense
--detach` never does, and a fused `ask` misses the rows it leaves until something dense runs — the
MCP `status` tool counts them as `owed`. Leave `--no-dense` out of the hooks: the background band
is what keeps the embed out of the way.

Readers that do not refresh themselves — an editor plugin, an MCP server — can be kept supplied by
a poller instead:

```bash
zerograph watch                  # poll every 30 s, apply what changed, embed it
zerograph watch --every 15       # a tighter cadence
zerograph watch --batch 5        # save once five files are waiting, not on every one
zerograph watch --no-dense       # lexical only, leaves the 1.4 GB model unopened
```

A refresh rewrites the whole graph, so `--batch` is there to spend that once on a burst of edits
rather than once per file. Fewer than `--batch` files waiting are carried to the next poll, at most
three times in a row, so a lone edit lands within four polls whatever the batch says;
`ZEROGRAPH_TIMING=1` reports each deferral.

It prints one line per refresh and exits on Ctrl-C; every store write is a temp file and a rename,
so interrupting it cannot leave half a graph behind. An idle poll is the walk and nothing else —
21 ms of CPU on the development corpus, under a tenth of a percent of a core at the default
cadence.

Git hooks are the free alternative to a poller or a resident process, for a repository whose changes arrive by pull:

```sh
# .git/hooks/post-merge, .git/hooks/post-checkout, .git/hooks/post-commit
#!/bin/sh
exec zerograph --repo "$(git rev-parse --show-toplevel)" update
```

`chmod +x` each of them. `post-checkout` and `post-merge` cover a branch switch and a pull;
`post-commit` covers your own work, which the self-healing `ask` already handles.
