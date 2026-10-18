# fix/store-root-and-grammar

## Tickets
- **#213: Resolve the repository root like git does, and prune nested checkouts** (`bug` · open · milestone 0.6.5)
  - Description: `--repo` defaults to `.` and never walks up; a read creates `.zerograph/` because the lock calls `ensure_dir`; run from a subdirectory, a second partial store appears there. The walker prunes only `.git` and the store, so nested worktrees (`.claude/worktrees/*`) are kept out only when gitignored.
  - Acceptance criteria: with no `--repo`, the root is the git top level (fallback: nearest ancestor with a store, else cwd); a directory below the root holding a `.git` entry is not walked; a read never creates the store directory.
  - Open questions: none — settled in Decisions.
- **#215: Refuse to downgrade a store a newer release wrote** (`bug` · open · milestone 0.6.5)
  - Description: `stale_grammar` compares with `!=`, so an older binary re-reads a newer store in full and stamps it with the older grammar. Nothing refuses or names a store a newer release wrote.
  - Acceptance criteria: an older binary answers read-only from a newer store and says so in one line naming both grammar versions; `update` refuses to rewrite it with the same line; the manifest records the writer's release and `status` shows it.
  - Open questions: none — settled in Decisions.

## Files to change
- `src/lib.rs` — `--repo` becomes optional; `repo_root` resolves git top level → ancestor with a store → cwd; `run_update` refuses a newer store; `record_stamps` keeps the release and writes nothing on a newer store; the watcher's poll writes nothing on a newer store; the GRAMMAR+1 test moves to an older generation.
- `src/walk.rs` — `Manifest.release` (`#[serde(default)]`, set to `CARGO_PKG_VERSION` by `from_entries`); `stale_grammar` means older only, `newer_grammar` added with `newer_store_line`; `filter_entry` prunes a directory below the root holding `.git`.
- `src/ask.rs` — `graph_for_ask` creates nothing when no store stands, and answers read-only with the one line from a newer store; `Read` carries that line as `note`.
- `src/store.rs` — `Store::stands`, `Store::load_manifest`.
- `src/status.rs` — `Tree::OtherGrammar` and new `Tree::NewerGrammar` carry the store's grammar and release.
- `agent/claude-code/types/index.d.ts`, `agent/claude-code/hooks/parse.ts` (+ test) — the `newer_grammar` state.
- `docs/commands.md`, `docs/keeping-fresh.md`, `docs/configuration.md` — root resolution, nested checkouts and submodules, the newer-store contract.

## Implementation steps
1. Manifest `release`; `stale_grammar` → older only; `newer_grammar`; one-line message.
2. Writers refuse a newer store: `apply_diff` guard, `run_update` bails, `record_stamps` no-op, watcher quiet.
3. `graph_for_ask`: no store standing → empty read, nothing created, nothing spawned; newer store → read-only with the line.
4. Status variants with release; TS type and hint.
5. Root resolution in `run`; walk prunes nested checkouts.
6. Docs; tests.

## Tests
- `src/walk.rs` — a newer manifest is not stale and is newer; a manifest without `release` loads; a nested checkout below the root is not walked.
- `src/lib.rs` — an update refuses a store a newer grammar wrote and leaves its bytes; an ask on a newer store answers read-only with the line and writes nothing; an ask with no store creates no directory; root resolution from a subdirectory of a git checkout, from a subdirectory of a non-git tree with a store above, and with neither.
- `src/status.rs` — a newer store reads `NewerGrammar` with its release.
- `agent/claude-code/hooks/parse.test.ts` — the hint for `newer_grammar`.

## Decisions
- R1 (owner, 2026-10-18): #215 — when manifest.grammar > GRAMMAR, an older binary answers read-only from the store (no write, no re-stamp) and prints one line naming both grammar versions; `update` also refuses to rewrite a newer store (same message). This reverses the documented behaviour at src/walk.rs:251-253 and the GRAMMAR+1 test near src/lib.rs:1516-1572 — update that comment and test to the new contract.
- L1 #215 B: record the writer's release (CARGO_PKG_VERSION) in manifest.json with `#[serde(default)]` (no migration), show it in `status` (src/status.rs, Tree::OtherGrammar ~113).
- L2 #213 A: with no `--repo`, resolve root via `git rev-parse --show-toplevel` (worktree gets its own root), fallback nearest ancestor containing `.zerograph/`, else cwd. Code: src/lib.rs:55-57 default ".", canonicalize ~980.
- L3 #213 B: in walk.rs filter_entry (~179-183) prune any directory (below root) containing a `.git` entry; submodules drop out of the index too — state that in docs.
- L4 #213 C: reads (ask.rs graph_for_ask ~115, store.rs try_lock_writer/lock_file/ensure_dir ~443-468, 296-302) no longer create `.zerograph/`.
- Recon evidence: scout reports confirm all cited lines on origin/main with small drift.
- `build` over a newer store: considered letting it rewrite (explicit start-from-nothing) vs refusing like `update`; chose refusing — a build is the same downgrade, and deleting the store is the deliberate escape.
- Where a read with no store stands: considered spawning a detached build vs answering from nothing; chose answering from nothing with a "no index — `zerograph build` builds it" line, since a detached build would create the directory L4 forbids and the hook already tells the user to build.
- The "no read creates" guard sits in `graph_for_ask` (the reader's one lock) rather than in `try_lock_writer`, whose other callers (`update`, MCP `reindex`) are writers that must create the store.
- Fallback ancestor accepts `.repograph/` too, since a store not yet moved to the new name is the same store.
- `embed` and `model` rewrite vectors only, not the graph or manifest, so they are left outside the refusal.
- Review taken: `build` reads the newer-store check leniently so a manifest that does not parse is still rebuilt; a resident context adopting a newer store stops its per-answer vector sync; `embed` and a model switch refuse a newer store after all (supersedes the earlier choice to leave them out); `ask --stale` with no store says so; `status` names a newer store before walking.
- Review not taken: the "a refresh is already running" wording on a read-only store's owed vectors (cosmetic; a `Writer::ReadOnly` variant is a wider change than this fix); the load-before-lock window where a newer `update` lands between the two (pre-existing order, narrow); `update` writing `writer.lock` and `background.log` into a newer store before it refuses (store data untouched); a cwd inside `.git/` resolving to `.git` (rare, and `--repo` overrides).
