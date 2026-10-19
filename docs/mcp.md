# MCP server

```bash
zerograph mcp                    # a stdio MCP server (protocol 2025-06-18) over the current repository
claude mcp add zerograph -- zerograph --repo /path/to/checkout mcp   # what `install-agent --claude` runs, per checkout
```

The [Claude Code plugin](install.md#claude-code-plugin) starts this server itself, once per
session, as `mcp__plugin_zerograph_zerograph__*`, and its slash commands call these tools. With the
plugin installed, `install-agent --claude`'s registration below is a second copy of the same
server: remove it with `claude mcp remove zerograph -s local`
([why](install.md#claude-code-plugin)).

`install-agent --claude` registers the server through the `claude` CLI, at local scope, once for
every checkout of the clone that `git worktree list` names, each with its own `--repo`. Local scope
is keyed by the project's path, so a session opened in a worktree sees only what was registered for
that worktree; a worktree added later gets its server from the next `install-agent` run. A
registration that already says the same is left alone, and the command to run by hand is printed
where `claude` is not on `PATH`. One that says something else is replaced only in the local scope: a
differing `zerograph` in the user scope or a tracked `.mcp.json` is left as its owner wrote it, the
local one is added beside it and wins in that project, and the run says so in one note.

The same run prunes `~/.claude.json` (or `.claude.json` in `CLAUDE_CONFIG_DIR`): a `zerograph` server
it registered for a checkout that is no longer on disk — a removed worktree — is taken out.
`claude mcp remove` cannot do this, since it finds
the project by a working directory that is gone. Nothing else in the file is changed, though its keys
are written back in sorted order.

Why not one project-scope `.mcp.json` with no `--repo`: Claude Code's
[MCP documentation](https://code.claude.com/docs/en/mcp) does not say which working directory a
stdio server starts in, so a server that found its checkout from its working directory would be
relying on something undocumented.

The protocol is newline-delimited JSON-RPC on stdout and nothing else; diagnostics go to stderr. A
tool that fails returns an `isError` result and the server carries on.

### Other agents

```bash
zerograph install-agent --cursor --copilot --gemini --opencode   # any subset, with --claude or --codex too
```

Each flag writes the agent's project MCP file with a `zerograph` server in it and the stanza into
the instructions file that agent reads always. A file already there is merged: its other servers and
settings stay.

| flag | MCP file | instructions |
| --- | --- | --- |
| `--cursor` | `.cursor/mcp.json` | `.cursor/rules/zerograph.mdc`, `alwaysApply: true` |
| `--copilot` | `.vscode/mcp.json` (VS Code) | `.github/copilot-instructions.md` |
| `--gemini` | `.gemini/settings.json` | `GEMINI.md` |
| `--opencode` | `opencode.json` | `AGENTS.md` |

These files are committed, so they carry no absolute path. Cursor and VS Code expand
`${workspaceFolder}` into `--repo`; Gemini CLI and opencode start the server in the directory they
were started in, so start them at the repository root. An `mcp.json` with comments in it is not
rewritten: the entry to paste is printed instead. Windsurf reads MCP servers only from a
machine-wide file, so it is not covered; `zerograph mcp` is the command to give it.

| tool | does |
| --- | --- |
| `ask` | `question` (plus `seeds`, `bodies`, `rerank`, `rerank_local`, `depth`, `stale`) to the answer `ask` prints, with the low-confidence line when the retrievers disagree. `rerank` spends tokens on the agent's word, so one server process runs at most `mcp_reranks` of them (25 by default, set in the machine config) and answers the rest from the fused order, saying so |
| `explain`, `impact`, `trace`, `changes` | the commands of those names, with their flags; an answer given while files are behind the tree says so |
| `status` | nodes and edges, files behind the tree, the store's model against the configured one, vector rows owed, and whether a background refresh is running with its last progress line from `.zerograph/background.log` and what the run said besides, and whether a `serve` answers with its last log lines. The same report as `zerograph status` |
| `reindex` | starts a detached `build` where no store stands, else a detached `update`, and returns at once; says so when one is already running. The child takes the writer lock as the command line does, so readers keep answering from the store as it stands |
| `cache_status` | the machine's vector cache: where it is, its cap, and the rows and bytes it holds per model. The same report as `zerograph cache status`; it writes nothing, and a cache not made yet reads as empty |
| `switch_model` | `model <hub id>` in the background. Only catalogued models and ones trusted on this machine with `zerograph model <id>`; any other id is refused, because the caller is an agent and a model is code this machine downloads |

`status` reports progress while the writer lock is held and nothing once the refresh is over. Each
read tool loads the store per call, or asks a running `serve` for `ask`.
