# The three `.claude/` overlays

`run.sh` swaps one of these into the disposable worktree as its `.claude/` and runs the twelve
tasks against it. The worktree is a `git worktree` of beauty-crm at `502e8a6d` with the pinned
fixture's `.zerograph/` copied beside it; the fixture itself is read once and never written.

| overlay | what it is | instructions the agent carries |
| --- | --- | --- |
| `A` | no zerograph at all — the fixture's `CLAUDE.md` with its zerograph sections cut, no binary on `PATH` | ~15.8 kB |
| `B` | today's surface, verbatim from the fixture: `CLAUDE.md`, `settings.json`, the notice/precompact/compact hooks, the `repo-query` skill | **29,543 B** |
| `C` | the planned surface, produced by `zerograph install-agent --claude` and never hand-edited: the stanza between its markers, the skill, the scout, the four-event hook | **1,295 B** |

`C-rem` is not a directory. It is `C` with `ZEROGRAPH_HOOK_INTERCEPT=0` in the environment — the
reminder without the Bash interceptor — because the two configurations differ by a switch and a
copied directory would let them drift apart.

The two instruction sizes are the whole hypothesis: B says everything in prose at the top of every
session; C says the little that cannot be discovered and lets the hook and the skill say the rest
when it is needed. What that trade actually costs is what the runs measure — tokens per hit, not
tokens.

**A is not run by default.** The B and C rows answer the question this branch asks; A is the
control for a different one (what an agent does with no graph at all) and costs a full run to read.

None of the three is checked in. `build.sh` makes all of them — A and B from the consuming
project's own `.claude/`, which belongs to that project rather than to this repository, and C from
whatever `install-agent` writes today:

```bash
cargo build --release && bench/agent/overlays/build.sh     # FIXTURE=… to point at another checkout
```

So a run always measures the surface this branch installs, and a stale overlay cannot quietly
become the thing being measured.
