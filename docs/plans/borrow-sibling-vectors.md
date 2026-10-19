# feat/borrow-sibling-vectors

> Superseded by the machine-wide vector cache (#241).

## Tickets
- **#211: Borrow vectors from sibling worktrees instead of re-embedding** (GitHub issue, free-text task, tracker=none)
  - Description: a fresh worktree pays a full cold build, and a cold build is 99.6% embedding
    (1,409 files: walk 0.21 s, parse 1.77 s, embed 671 s).
  - Acceptance criteria: during dense sync, a row missing locally is looked up by its blake3 text
    hash in the vectors of the other worktrees of the same repository that use the same model,
    revision and dimension; only the misses are embedded.
  - Comments: the owner's 2026-10-18 comment re-scopes the issue and supersedes approaches A and B
    (no `seed` command, no store copy, no clonefile, no auto-seed, no `--no-seed`). C, a
    content-addressed cache in the git common dir, stays the follow-up.
  - Open questions: none.

## Files to change
- `src/index/siblings.rs` (new) — find the sibling worktrees and read a sibling's pair under a
  try-lock of its `writer.lock`.
- `src/index/dense.rs` — remember the store's repo root and the claimed width; borrow the misses
  before embedding; count only real embeds.
- `src/index/mod.rs` — declare the module.
- `docs/keeping-fresh.md` — a short section on borrowing.

## Implementation steps
1. `DenseIndex` keeps the repo root it was loaded from and the width `written_by` claimed
   (both `serde(skip)`).
2. `siblings::worktrees(repo)` — `git worktree list --porcelain` against the repo root, self
   excluded, only those whose store holds `vectors.json`.
3. `siblings::read(path)` — open the sibling's existing `writer.lock` read-only (no create, no
   link), try-lock it, load the pair, release. Busy, missing, torn or unreadable ⇒ `None`.
4. `DenseIndex::sync_chunked` — after the misses are known, and only when there are some and a
   model and width were claimed, walk the siblings in order, take every missing hash a matching
   sibling holds as a live passage row, stop when nothing is missing. Borrowed rows are appended
   before the embed loop; the embed loop runs on what is left; the return value and the progress
   total count real embeds only.
5. One stderr line per lending sibling: `dense: borrowed N rows from <path>`.
6. Document in `docs/keeping-fresh.md`.

## Tests
- `src/index/siblings.rs` — a real `git init` + `git worktree add`: discovery lists the sibling
  with vectors and not self; a sibling with no store is not listed.
- `src/index/dense.rs` — two temp worktrees of one repository with the fake embedder:
  - rows a matching sibling holds are borrowed, not embedded, and equal the sibling's vectors;
  - a sibling of another model or width lends nothing;
  - a sibling whose writer lock is held lends nothing;
  - a sibling with a torn pair lends nothing and is no error;
  - nothing inside the sibling's store is created or modified.

## Decisions
- Owner ruling R3 (2026-10-18): no `seed` command, no store copy. Each worktree builds its own
  graph; during dense sync, rows missing locally are looked up by text hash in sibling worktrees'
  vectors before embedding; only misses are embedded.
- L1: siblings = `git worktree list --porcelain` run against the repo root, excluding self, keeping
  those whose `.zerograph/vectors.json` exists and whose model id, revision/variant and dimension
  equal ours. Looked up lazily — only when there are local misses.
- L2: a sibling's pair is read only while holding a try-lock of that sibling's `writer.lock`,
  released right after reading; busy or missing lock file ⇒ skip that sibling. Nothing is ever
  created or written inside a sibling's `.zerograph/`.
- L3: a sibling whose pair loads torn or inconsistent is skipped silently, never an error.
- L4: one line per lending sibling, `borrowed N rows from <sibling path>`; `embedded` counts only
  real embeds.
- L5: no config switch.
- L6: code stays in `dense.rs` plus one new module `src/index/siblings.rs`; `lib.rs`, `ask.rs`,
  `walk.rs`, `store.rs`, `status.rs` are not touched (another batch item edits them).
- L7: documented in `docs/keeping-fresh.md`, including that borrowed vectors are near-identical,
  not bit-identical, to freshly embedded ones.
- Revision: `vectors.json` records no revision; the revision is this binary's pin for the model id
  (`embed::revision`), and the variant is part of the id. Considered adding a `revision` field;
  chose comparing the id (case-insensitive, as `written_by` does) and the width — a recorded field
  would lend nothing from every store written before it, the main checkout's included, and a store
  that outlives a pin change already keeps its rows under the same rule in `written_by`.
- Repo root without touching `store.rs`: taken from `Store::dir()`'s parent at `load`, rather than
  adding an accessor to `store.rs`.
- Borrowing needs a width the caller claimed through `written_by`; a sync with no claim (the unit
  tests' `sync`) never borrows. A cold store has width 0 until the first chunk, so the claimed
  width is what makes a borrow possible before any embed.
- The foreground meter's total in `lib.rs` is `owed()`, which counts borrowable rows too; left as
  is (L6 keeps `lib.rs` out), so a mostly-borrowed run's meter may end below its total. The
  `dense: embedded N rows` line is the count.
- Match by text hash alone, ignoring the node id: the vector depends only on the text, so a row
  whose node id differs between the checkouts while its text does not is lent all the same.
- Review: `embed` and `watch` save without the writer lock, so the lock alone does not keep a
  half-saved pair out. Considered taking the lock in `lib.rs` (out of bounds, L6) and a shared
  generation field in both files; chose an mtime guard in `siblings::read` — a save renames the
  rows before the metadata, so rows newer than their metadata, or a pair that moved during the
  read, lends nothing.
- Review: the model is checked from the metadata before a sibling's rows are read, and a store
  directory that is a link is not read.
- Review, not taken: a first embedded chunk wider than the claimed width would drop the borrowed
  rows with the rest. Every caller claims the width its embedder just measured, so the case does
  not arise; the next sync would re-owe the rows if it did.
- Review, not taken: the `ask` reader's budget gate weighs `owed()`, which counts borrowable rows,
  so a reader on a fresh worktree may refuse a sync a borrow would have made cheap. `ask.rs` is
  out of bounds (L6); `update` and the background refresh borrow, which is the path a new
  worktree takes. Follow-up.
- Review, not taken: a `--repo` naming a subdirectory of a worktree would list that worktree's
  root as a sibling. It is only read, under its lock, so nothing breaks.
