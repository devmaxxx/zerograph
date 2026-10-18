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
So the first `update`, `serve` poll or refreshing `ask` after such an upgrade says on stderr that it
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
starts the same refresh by hand. A foreground `update` or `build` prints where its embed is every two
seconds:

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
file, or holds a torn pair is skipped. Its `vectors.json` is read first, and its rows only when
that names one of the rows wanted; a sibling that held none is remembered against its rows' stamp
for the life of the process, so the rows of a fresh edit, which no sibling has, cost no further
read on each answer after it. A borrowed vector is the one this checkout would have
embedded up to the last digits — batch padding makes two embeds of the same text near-identical,
not bit-identical — so answers do not move. There is no switch: a repository with one checkout, or
siblings on another model, simply embeds everything.

### The session hook

`install-agent` writes no git hooks. Every read already refreshes, and `serve` catches up when it
starts and then polls, so a git hook only bought a warm first read, at the price of a full embed on
every `git worktree add` (hooks live in the common git directory, so the block fired in every
worktree) and of a block a hook manager rewrote away. A re-run of `install-agent` takes out the
`# zerograph:begin` … `# zerograph:end` block an earlier release wrote into `post-merge` and
`post-checkout`, leaves the rest of each hook as it is, and deletes a hook that held nothing else.

The agent's session hook does that job per checkout instead. At SessionStart:

- **No index in this checkout** — a new worktree, typically — and the checkout asked for one (it
  holds the hook `install-agent` writes, or a `zerograph.toml`, or its main worktree does): it starts `update --detach` at the
  checkout's root and says in one line that a build is running. A checkout that asked for nothing
  stays silent, which matters for Codex, whose one hook runs in every repository.
- **An index** — it starts the resident `serve`, which catches the store up as it starts; with
  `ZEROGRAPH_HOOK_SERVE=0` it starts `update --detach` instead, which writes nothing when nothing
  moved.

The hook runs the binary `ZEROGRAPH_BIN` names, else the `zerograph` on `PATH`, and never one
inside the checkout: a cloned repository that commits a `zerograph.toml` and an executable
`node_modules/.bin/zerograph` would otherwise have that file started as soon as a session opened
in it. A project-local install (`pnpm add -D zerograph`) puts no `zerograph` on `PATH`, so set
`ZEROGRAPH_BIN` to its absolute path in the environment the agent starts from, or install
globally; without either the hook stays silent.

The hook's own reads wait at most 5 s, so each one runs with `ZEROGRAPH_READER_BUDGET=2` (or a
smaller value the environment or the repository's `zerograph.toml` already sets): a refresh that would not finish inside that is left to a detached
`update` rather than killed at the timeout with its work lost.

Each worktree keeps its own store, its own `vectors.f32` included. Copying a store as an APFS clone
(`cp -c`) saves less than it seems: `graph.json`, `graph.bin`, the manifest and the vectors'
metadata are rewritten whole through a temporary file and a rename, so the copy's first refresh
gives them blocks of their own. `vectors.f32` grows in place and keeps its shared blocks only until
it is rewritten whole — after a compaction, a model switch, or another process rewriting it. There
is no delta format. The vectors a new worktree needs come instead from a sibling worktree's store,
borrowed when its index is built rather than embedded again.

### Background priority

The detached `update` that a reader, the session hook or the MCP `reindex` and `switch_model` tools start
runs at background priority. A command someone is waiting on — `ask`, `build`,
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

Every writer holds the store's `writer.lock` while it writes: `build`, `update`, `embed`,
`model <id>`, a refreshing `ask` and each `serve` poll. That lock is how `status` knows a refresh is
running and how two writers stay out of each other's files. `build`, `update`, `embed` and
`model <id>` wait for a refresh that holds it, saying so on stderr; a `serve` poll that finds it
held skips that cycle and reads back what the holder wrote on the next one.

A dense `update` embeds every row the store is owed, whoever left it owed: the rows are matched to
the stored graph by hash, not to what that run re-read, so one over a tree nothing moved still pays
for an `update --no-dense` or a lexical `ask` before it. A hook written as `update --no-dense
--detach` never does, and a fused `ask` misses the rows it leaves until something dense runs — the
MCP `status` tool counts them as `owed`. Leave `--no-dense` out of the hooks: the background band
is what keeps the embed out of the way.

Readers that do not refresh themselves are covered already: every MCP tool refreshes before it
answers, and a resident [`serve`](serve.md) polls the tree between questions.

Git hooks written by hand are the free alternative to a resident process, for a
repository whose changes arrive by pull. They run in every worktree of the clone, a new one included:

```sh
# .git/hooks/post-merge, .git/hooks/post-checkout, .git/hooks/post-commit
#!/bin/sh
exec zerograph --repo "$(git rev-parse --show-toplevel)" update
```

`chmod +x` each of them. `post-checkout` and `post-merge` cover a branch switch and a pull;
`post-commit` covers your own work, which the self-healing `ask` already handles.
