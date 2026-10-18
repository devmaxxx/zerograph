# fix/agent-install-worktrees

Stacked on `fix/store-root-and-grammar` (PR #224): its git-toplevel root resolution is what lets a
server or a refresh started without `--repo` find the checkout it runs in.

## Tickets
- **#212: Remove the git-hook block; bootstrap a worktree from the session hook** (open)
  - Description: the `post-checkout`/`post-merge` block `install-agent` writes lives in the common
    `.git/hooks`, so it fires in every worktree and starts a full embed on every `git worktree add`;
    under a hook manager it is rewritten away.
  - Comments: re-scoped 2026-10-18 by the owner — remove the block instead of guarding it, remove one
    an earlier release wrote, let SessionStart start `update --detach` when the index is missing or
    behind, update `docs/keeping-fresh.md`. This overrides the description's approaches B and C.
- **#214: Register the Claude MCP server per checkout, not with an absolute --repo** (open)
  - Description: the server is registered at local scope keyed by the clone's path with
    `--repo <clone>`, so a session in a worktree gets no server or the clone's tree.
- **#218: Keep the hook's in-process refresh inside its 5 s timeout** (open)
  - Description: the hook kills a call after 5 s, but a reader refreshes in process for up to
    `reader_budget` (10 s), so PostToolUse `changes` can be killed mid-refresh and lose the work.
- **#219: Codex hook path breaks when the worktree it was installed from is removed** (open)
  - Description: `install-agent --codex` prints a hook path inside the checkout it ran in, and
    Codex's hooks are per user.

## Files to change
- `src/install_agent.rs` — git-hook installer gone; `retire_git_hooks` removes an earlier block;
  `worktrees`/`main_worktree` read `git worktree list --porcelain`; `prune_stale_mcp` drops entries
  for checkouts that no longer exist; `codex_hook` puts the Codex hook in the main worktree.
- `src/lib.rs` — `install-agent` registers the MCP server in every worktree, prunes, retires the git
  block, and prints the Codex hook path from the main worktree.
- `agent/hook.mjs` — SessionStart bootstraps a missing index and catches up a behind one; every
  binary call carries a refresh budget below the hook's timeout.
- `agent/hook.test.mjs` — tests for both.
- `docs/keeping-fresh.md`, `docs/mcp.md`, `docs/install.md`, `README.md`, `agent/codex.md`.

## Implementation steps
1. #212: delete the git-hook installer, add the retirement of an old block, the SessionStart
   bootstrap, the docs.
2. #218: `ZEROGRAPH_READER_BUDGET` from the hook.
3. #214: register per worktree, prune entries whose path is gone, docs.
4. #219: the Codex hook in the main worktree.

## Tests
- `src/install_agent.rs` — an old block is removed and the rest of the hook kept; a hook that held
  nothing else is deleted; worktree listing; stale MCP entries pruned, live and foreign ones kept;
  the Codex hook lands in the main worktree and an existing one there is left alone.
- `agent/hook.test.mjs` — SessionStart in an opted-in checkout with no index starts a detached
  `update`; one with no opt-in stays silent; with serve off, a built index gets `update --detach`;
  every call carries a budget under the timeout, and a smaller one already set is kept.

## Decisions
Owner rulings (not re-opened):
- R4 (owner, 2026-10-18), #212: delete the git-hook installer and its block and tests; a re-run
  removes a block an earlier release wrote, keeps the rest of the hook file, and deletes the file
  only when nothing else is left; SessionStart starts `update --detach` when the index is missing or
  behind; update `docs/keeping-fresh.md`.
- R2 (owner, 2026-10-18), #214: project scope with no `--repo` if Claude Code's docs confirm a
  project-scope stdio server starts with cwd = the project directory; otherwise per-worktree local
  registration with pruning of entries whose path no longer exists. Either way retire the old
  path-keyed entry and update `docs/mcp.md`.
- L1, #218: a budget below the hook's 5 s timeout, by the smallest mechanism the hook can pass.
- L2, #219: resolve the Codex hook path from the main worktree, or install it next to the binary.

Taken in the flow:
- **MCP path (R2): fallback, per-worktree local registration.** Source:
  https://code.claude.com/docs/en/mcp, read in full on 2026-10-18. It documents no working
  directory for a stdio server; it says instead that "Claude Code sets `CLAUDE_PROJECT_DIR` in the
  spawned server's environment to the project root, so your server can resolve project-relative
  paths without depending on the working directory." Not confirmed ⇒ the ruling's fallback.
  (`CLAUDE_PROJECT_DIR` read by `zerograph mcp` is a possible later move to project scope; not taken
  here, since the ruling names the cwd as the condition.)
- **Which worktrees are registered:** considered the checkout `install-agent` runs in only, or every
  worktree `git worktree list` names; chose every existing non-bare worktree — local scope is keyed by
  path, so registration per worktree is what makes a session in each one get its own server.
- **"Retire the old path-keyed entry" in the fallback:** the entry keyed by a live checkout is the
  mechanism now, so what retires is an entry (`zerograph`, or `repograph` from before the rename)
  keyed by a path that no longer exists; `retire_legacy_mcp` still removes `repograph` from each
  live checkout.
- **How pruning edits `~/.claude.json`:** considered `claude mcp remove` (needs its cwd to be the
  gone path) and a direct edit; chose the direct edit, removing only our server from a project whose
  path is missing and whose entry runs `… --repo <path> mcp`, written to a temp file and renamed
  over, permissions kept, and only when something is pruned. `serde_json` here has no
  `preserve_order`, so the rewritten file's keys come out sorted; no build-config change was allowed
  to avoid that. `CLAUDE_CONFIG_DIR` is honoured as the directory holding `.claude.json` (inferred,
  not in the MCP page).
- **Removing the old git block:** both the `--git-path hooks` directory (honours `core.hooksPath`)
  and the common `hooks/` directory are checked, since an earlier release wrote to the first and a
  hook manager may have moved it since.
- **SessionStart bootstrap gate:** considered every checkout, or only one that opted in; chose a
  git checkout that holds `.claude/hooks/zerograph-hook.mjs`, `.codex/hooks/zerograph-hook.mjs` or
  `zerograph.toml`. The Codex hook is machine-wide, and a build started in every repository a session
  opens would be the cost #212 removes, moved.
- **Bootstrap dense or `--no-dense`:** chose dense, as `docs/keeping-fresh.md` already rules for
  hooks: the background band keeps the embed out of the way, and a `--no-dense` build leaves the
  vectors owed until something dense runs.
- **"Behind" without a walk in the hook:** with the resident `serve` on (the default) it catches up
  at start, so nothing more is spawned; with `ZEROGRAPH_HOOK_SERVE=0` SessionStart starts
  `update --detach`, which decides for itself whether anything moved, at background priority.
  A synchronous `status` walk inside a 10 s hook to decide the same thing was rejected.
- **L1 mechanism:** `ZEROGRAPH_READER_BUDGET` already exists and is the last config layer; the hook
  sets it to 2 s on every call (or keeps a smaller value already set). 2 s of refresh leaves ~3 s of
  the 5 s for process start, the store load, the walk and the answer. No new flag.
- **L2 choice: the main worktree.** One `git worktree list --porcelain`, beside code that already
  shells out to git. Next to the binary was rejected: under `pnpm exec` the binary is in a
  worktree's `node_modules`, which goes with the worktree, and an npm global or cargo bin dir is
  replaced on upgrade or not writable. The hook and `rule.txt` are written into the main worktree's
  `.codex/hooks/` only when no hook is there yet; one already there is left as it is. A bare main
  repository, or git missing, falls back to the checkout the install ran in.
- **Build shape:** the four steps were built in order in this item's own worktree by the item agent
  rather than by one implementer subagent each — the batch lead had already delegated the track, and
  a nested fleet would add test runs on a machine other tracks share. Review is one read-only agent
  over the whole diff, as the batch run asked.
