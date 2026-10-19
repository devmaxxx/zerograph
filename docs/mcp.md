# MCP server

```bash
zerograph mcp                    # a stdio MCP server (protocol 2025-06-18) over the current repository
zerograph --repo /path/to/checkout mcp
```

The [Claude Code plugin](install.md#claude-code-plugin) starts this server itself, once per
session, as `mcp__plugin_zerograph_zerograph__*`, and its slash commands call these tools. The
[Codex plugin](install.md#codex-plugin) starts it the same way for a Codex session.
`install-agent --claude` and `--codex` install those plugins and register no server of their own.

Both start it through the plugin's `scripts/mcp.mjs`, which finds the checkout around the session's
project directory (`CLAUDE_PROJECT_DIR` under Claude Code, the working directory Codex starts it
in) and runs `zerograph --repo <checkout> mcp`, so a session opened in a package of a monorepo
serves the whole repository.

A session keeps the server it started with, and a plugin cannot restart it, so after `npm i -g
zerograph` or `pnpm add -g zerograph` the old binary goes on serving, and writing a store the new
one owns. `mcp.mjs` asks the binary its version at start and, on each tool result, looks at the
file it resolved to; when that file has moved and now reports another release, every tool result
from then on begins with one line, `zerograph was updated to X (this session runs Y) — restart the
session or run /reload-plugins`. Nothing is stopped or restarted. The hook cannot see which release
a session's server runs, so the line comes only from the server.

Releases before 0.7.0 registered the server through `claude mcp add` at local scope, once per
worktree. A re-run of `install-agent --claude` removes those registrations from `~/.claude.json`
(or `.claude.json` in `CLAUDE_CONFIG_DIR`) for the checkout and its worktrees, and for a checkout
no longer on disk — a removed worktree, which `claude mcp remove` cannot reach, since it finds the
project by a working directory that is gone. A differing `zerograph` in the user scope or a tracked
`.mcp.json` is left as its owner wrote it. Nothing else in the file is changed, though its keys are
written back in sorted order.

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
