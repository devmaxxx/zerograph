# fix/serve-socket-and-status

Stacked on `fix/agent-install-worktrees` (#226, which stacks on #224). One track, three steps,
built in order.

## Tickets

- **#216: Worktree papercuts: hook notice, clone savings, status vs resident serve** (bug · open)
  - Description: (1) `hook.mjs` tells a fresh worktree "a couple of minutes" and says nothing about
    seeding; (2) `write_atomic` rewrites `graph.json`/`graph.bin` whole, so a cloned store stops
    sharing blocks at its first refresh — document it or write a delta; (3) `status` printed
    `serve: not running` while `ask` was answered by the resident process.
  - Comments (2026-10-18, owner): (1) the notice should name borrowing vectors from sibling
    worktrees (#211 re-scoped away from `seed --from`); (2) still applies, document it; (3) likely
    cause is the Claude Code mod's fallback check (`agent/claude-code/hooks/register.ts:48`), which
    looks only at `<dir>/serve.sock` while a checkout path over 100 bytes puts the socket in
    `$TMPDIR/zerograph-<uid>/`; a single socket rule is #220.
- **#220: One socket path rule, independent of the checkout path length and TMPDIR** (open)
  - Always `~/.cache/zerograph/sock/<fnv of the canonical checkout path>.sock`; keep the `$TMPDIR`
    form only when that path is too long. `status` prints the socket path it checked.
- **#223: Read the Claude Code mod's status from the log and lock, not status --json** (open)
  - The mod polls `zerograph status --json` every 10 s per session and `status` walks the whole
    tree each time. Read `background.log`, `writer.lock` and the socket directly, as `fromLogs`
    does; run `status --json` only on demand or at a long interval.

## Files to change

- `src/serve.rs` — one socket rule: `~/.cache/zerograph/sock/<fnv>.sock`, `$TMPDIR` only past the
  `sun_path` limit; the socket directory made private on both paths.
- `src/status.rs` — `Serve.socket`: the path checked, in the text report and in `--json`.
- `src/lib.rs` — the `serve` doc comment names the new location.
- `tests/serve.rs`, `tests/status.rs`, `tests/users_day.rs` — find the socket where the binary says
  it is, not at `.zerograph/serve.sock`.
- `agent/claude-code/hooks/register.ts`, `agent/claude-code/hooks/parse.ts`,
  `agent/claude-code/types/index.d.ts`, `agent/claude-code/hooks/parse.test.ts` — live state from
  the log and the socket every 10 s, `status --json` at start, every 5 min, after a run ends, and on
  `/zerograph`; the socket path taken from `status --json`.
- `agent/hook.mjs` — `BUILD_NOTICE` names borrowing from sibling worktrees.
- `docs/serve.md`, `docs/install.md`, `docs/keeping-fresh.md` — the socket rule and the upgrade
  note, the mod's cadence, per-worktree vectors and APFS clones.

## Implementation steps

1. **Socket rule (#220, #216 item 3).** `socket_path(repo)` = `<home>/.cache/zerograph/sock/<fnv of
   canonical repo>.sock`; when that exceeds the 100-byte margin under `sun_path` (or there is no
   home directory), `$TMPDIR/zerograph-<uid>/zerograph-<fnv>.sock` as today. `prepare_socket_dir`
   creates the parent (and its parents) and refuses a socket directory that is not private to the
   user on unix; on Windows it creates the directory. `status` gains `serve.socket` and prints it.
   Integration tests read the socket path from `status --json` instead of assuming
   `.zerograph/serve.sock`. Docs: `docs/serve.md` names the rule and says a server started by 0.6.4
   or earlier is not found and a new one starts beside it, leaving at its own idle timeout.
2. **Mod reads files (#223, #216 item 3).** Every 10 s the mod reads `background.log` (run state,
   via `readRun`) and checks the socket path the last `status --json` named, overlaying those on the
   last full report. `status --json` runs at session start, every 5 minutes, once after a run ends
   (log written since the last full report and now quiet), and after `/zerograph`. With no
   reported socket (a zerograph without `status`), the fallback keeps `<store>/serve.sock`.
   `docs/install.md` says so.
3. **Notice and docs (#216 items 1, 2).** `BUILD_NOTICE` says a build takes a few minutes on a small
   repository and longer on a large one, and that a worktree beside a built one borrows its
   vectors instead of embedding them again. `docs/keeping-fresh.md` says each worktree keeps its
   own `vectors.f32` and an APFS clone of a store stops sharing blocks at its first rewrite.

## Tests

- `src/serve.rs` unit — the socket is under `~/.cache/zerograph/sock` for a short checkout path;
  a path whose socket would not fit falls back to the private temp directory; one checkout always
  maps to one name, two checkouts to two; a socket directory open to others is refused.
- `src/status.rs` unit / `tests/status.rs` — `serve.socket` is the path `serve` binds, and the text
  report prints it.
- `agent/claude-code/hooks/parse.test.ts` — the overlay replaces refresh, last run and serve from
  the live read and keeps the rest; a full check is due at 5 minutes and after a run ends, not
  before.

## Decisions

Lead decisions given with the batch (not re-opened):

- **L1, #216 item 1:** `BUILD_NOTICE` names borrowing vectors from sibling worktrees, not `seed`.
  #211 shipped as PR #225 (a cold worktree borrows from siblings' stores). No "40 minutes" — no
  bench doc backs a figure. Built on #226's `hook.mjs` (SessionStart bootstraps).
- **L2, #216 item 2:** document it, no delta format. Each worktree keeps its own `vectors.f32`; an
  APFS clone stops sharing blocks at the first rewrite.
- **L3, #216 item 3 and #220:** one socket rule — always
  `~/.cache/zerograph/sock/<fnv of canonical checkout path>.sock`, `$TMPDIR` only past the
  platform's `sun_path` limit. `status` prints the socket path it checked; the mod's fallback uses
  the same rule; the binary exposes the path in `status --json` so the mod does not duplicate the
  hash. A server started by an older release (old location) is not found and a new one starts;
  stated in the docs.
- **L4, #223:** the mod reads `background.log`, `writer.lock` and the socket directly, as
  `fromLogs` does; `status --json` runs only on demand or at a long interval (≥ 5 min).

Choices made during the flow:

- Socket length margin: considered the exact per-platform `sun_path` (104 macOS, 108 Linux) and the
  existing single 100-byte margin; kept 100 — one number for every platform, as before.
- Socket directory privacy: considered checking only the `$TMPDIR` fallback (today's rule) and
  checking whichever directory the socket lands in; chose both — the home-cache directory is made
  `0700` on creation and refused when it is open to others, so a shared or mis-permissioned home
  cannot hand the socket to another user.
- `writer.lock` in the mod: the mod's `$` has no `flock`, so whether the lock is held cannot be read
  from a hook; chose `background.log`'s marker and recency (the `fromLogs` reading) for the run
  state, and the 5-minute `status --json` (which takes the lock's word) to correct it.
- Full report after a run ends: considered waiting for the 5-minute check; chose one extra
  `status --json` when the log was written after the last full report and is now quiet, so "N
  behind" and "to embed" do not linger for minutes after a refresh lands.
- The release-notes line for the old socket location goes in `docs/serve.md`: there is no
  unreleased release-notes file on this base.
- Step 2's overlay replaces `refresh` and `last_run` only when `background.log` changed after the
  full report; otherwise the full report has already taken the lock's word and stands.
- `serve.running` comes from the socket only when the socket is newer than the full report, so a
  dead server's leftover socket does not read as serving forever.
