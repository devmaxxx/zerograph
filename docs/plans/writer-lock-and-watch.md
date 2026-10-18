# fix/writer-lock-and-watch

## Tickets

- **#217: Take the writer lock in every writer: serve's watcher, embed and model** (bug)
  - Description: `writer.lock` is how readers and `status` know a refresh is running and how two
    writers stay apart. The `serve`/`watch` Watcher applies a diff and saves the graph unlocked;
    `embed` and `model <id>` write vectors unlocked. A detached `update` can write the same files
    meanwhile, and `status` reports no refresh.
  - Acceptance criteria: `embed` and `model <id>` take the lock the way `update` does (try, then
    wait); the Watcher try-locks each cycle and skips the cycle while a refresh holds it.
- **#221: Remove the watch command** (breaking)
  - Description: `watch` runs the same Watcher as `serve` without answering requests; MCP tools
    refresh on every read, so it is one more process per worktree and one more unlocked writer.
    The `/.fastembed_cache` line in `.gitignore` is a leftover: the model cache is
    `~/.cache/zerograph/fastembed`.
  - Acceptance criteria: the command, its docs and the ignore line are gone; the release notes say
    removing it breaks scripts that call it.
- **Follow-up from #225 (L3)**: the read-path budget gate (`src/ask.rs`) and the progress meter
  (`pay_owed` in `src/lib.rs`) count rows a sibling worktree can lend as owed, so a read can spawn
  a detached update needlessly and the meter's total overstates the work.

## Files to change

- `src/lib.rs` — `writer_lock` helper used by `update`/`build`, `embed`, `model <id>`; the
  Watcher's `poll` try-locks and skips a held cycle; `Cmd::Watch` and `run_watch` removed; the
  meter's total counted after borrowing.
- `src/index/dense.rs` — `borrow_owed`: the sibling borrow run before a count, and not run twice
  by the sync after it.
- `src/ask.rs` — the locked reader sync borrows before weighing what is owed against its budget.
- `src/index/siblings.rs` — comment no longer says `embed` and `watch` save unlocked.
- `src/priority.rs`, `src/serve.rs` — references to `watch`.
- `.gitignore` — drop `/.fastembed_cache`.
- `README.md`, `docs/*.md` (live docs) — drop `watch`.
- `docs/releases/0.6.5.md` — Breaking and Fixed lines.

## Implementation steps

1. Merge `origin/main` (for #225's `src/index/siblings.rs`), conflicts kept on both sides.
2. L1: lock helper; `embed` and `model <id>` take it; the Watcher try-locks per poll.
3. L2: remove `watch`, its docs and the ignore line; release notes.
4. L3: `DenseIndex::borrow_owed`; called before `owed` in the reader's locked sync and in
   `pay_owed`.

## Tests

- `src/lib.rs` — a poll while another holder has the writer lock refreshes nothing and leaves the
  edit pending; the next poll after the lock is dropped refreshes it.
- `src/index/dense.rs` — after `borrow_owed`, `owed` counts only rows no sibling holds, and the
  following sync embeds exactly those.

## Decisions

- L1 (#217, owner): `embed` and `model <id>` take the writer lock the way `update` does — try,
  then wait. The serve Watcher try-locks each cycle and skips it while a refresh holds it.
- L2 (#221, owner): delete `watch`, its docs and `/.fastembed_cache` in `.gitignore`; breaking,
  said under "Breaking" in the PR and in the next release notes.
- L3 (follow-up from #225, owner): count only rows neither local nor borrowable, by borrowing
  before the count. The base lacked #225, so `origin/main` was merged in first; the merge had two
  conflicts (`docs/keeping-fresh.md`, `src/lib.rs`) and was clean to resolve, so L3 is done.
- Watcher lock placement: considered locking only around `apply_diff` vs. the whole poll; chose a
  try-lock at the start of the poll, before the reload and the walk, held to the poll's end — a
  cycle that cannot lock does not read a store mid-rewrite either, and the next poll reads it back.
- `model <id>` locks before the model is fetched and opened, as `update` locks before its walk:
  the switch rewrites every row, and `status` should show it running from the start.
- `borrow_owed` is applied only where the reader holds the lock; a reader that found it held
  (`Barred`) reports its count unchanged, since the holder is the one that will borrow.
- Historical bench results and dated plans that mention `watch` are left as written.
- Release notes: 0.6.4 is tagged, so the lines go into a new `docs/releases/0.6.5.md`.
