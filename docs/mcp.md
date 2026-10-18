# MCP server

```bash
zerograph mcp                    # a stdio MCP server (protocol 2025-06-18) over the current repository
claude mcp add zerograph -- zerograph mcp   # what `install-agent --claude` runs, once
```

`install-agent --claude` registers the server through the `claude` CLI, leaves one that is already
registered alone, and prints the command to run by hand where `claude` is not on `PATH`. The
protocol is newline-delimited JSON-RPC on stdout and nothing else; diagnostics go to stderr. A tool
that fails returns an `isError` result and the server carries on.

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
| `reindex` | starts a detached `update` and returns at once; says so when one is already running |
| `switch_model` | `model <hub id>` in the background. Only catalogued models and ones trusted on this machine with `zerograph model <id>`; any other id is refused, because the caller is an agent and a model is code this machine downloads |

`status` reports progress while the writer lock is held and nothing once the refresh is over. Each
read tool loads the store per call, or asks a running `serve` for `ask`.
