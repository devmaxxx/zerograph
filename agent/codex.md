# What Codex actually reads

Read from the installed CLI rather than from memory of a docs page — first on 2026-09-09 against
0.147.0, and re-read on 2026-09-10 against **0.154.0**, which moved one of the three. Each fact
carries how it was established, because the first install written against an assumption put two
files where nothing would ever read them.

Two of the three are settled by running the CLI: `codex debug prompt-input` renders the exact
message list a session would be given, and costs no model call. That is the check to re-run when a
Codex release moves any of this.

## Instructions: the repository's root `AGENTS.md`

The binary's own instruction template carries an *AGENTS.md spec* section:

> Repos often contain AGENTS.md files. These files can appear anywhere within the repository. …
> The scope of an AGENTS.md file is the entire directory tree rooted at the folder that contains
> it. … The contents of the AGENTS.md file at the root of the repo and any directories from the CWD
> up to the root are included with the developer message and don't need to be re-read.

Confirmed by running it: a scratch repository whose root `AGENTS.md` held a unique marker, and
`codex debug prompt-input` in that directory printed the marker under a `--- project-doc ---`
separator. On 0.147.0 that separator was inside a **developer** message headed
`# AGENTS.md instructions for <cwd>`; on 0.154.0 it is inside the **user** message, at the end of
an `<INSTRUCTIONS>` block. What matters to the stanza is unchanged — the root file is included in
every turn rather than fetched on demand — but the role is not something to write into a test.

So the stanza goes in `<repo>/AGENTS.md`, created when the repository has none. There is no
`.codex/AGENTS.md` in that spec, and an earlier draft of this installer wrote one — a file Codex
would never have opened. The same section is why the stanza is short: a root `AGENTS.md` is
included in **every** turn's instructions, not fetched on demand.

## Hooks: a machine-level file, not a repository one

The config schema names hooks once, as `"hooks": "./hooks.json"` — a path relative to `CODEX_HOME`,
which is `~/.codex` unless the environment says otherwise. The live file on this machine
(`~/.codex/hooks.json`, written by GitNexus) confirms the shape, and it is the same shape Claude
Code uses:

```json
{ "hooks": { "PreToolUse": [ { "matcher": "Grep|Glob|Bash",
  "hooks": [ { "type": "command", "command": "node '/abs/path/hook.mjs'", "timeout": 10 } ] } ] } }
```

All four events this hook dispatches on are present in the binary's strings: `SessionStart`,
`SubagentStart`, `PreToolUse`, `PostToolUse`.

A **repository-level** hooks file is not read at all. Settled on 0.154.0: a scratch git repository
carrying the same valid `SessionStart` block at five candidate paths — `.codex/hooks.json`,
`.codex/hooks/hooks.json`, `hooks.json`, `.agents/hooks.json`, `.codex/config.hooks.json` — pointed
at a script that appends a line to a file, run through `codex exec` with the project marked
`trust_level = "trusted"` **and** `--dangerously-bypass-hook-trust`, left that file untouched while
the session's own two `SessionStart` hooks ran. Those two come from installed plugins, which is the
other source: `[hooks.state]` in `~/.codex/config.toml` keys entries as
`<plugin>@<owner>:hooks/hooks.json:<event>:<i>:<j>` beside `/Users/<user>/.codex/hooks.json:<event>:<i>:<j>`.

So the sources are `$CODEX_HOME/hooks.json` and installed plugins, and nothing a repository ships.

**Hooks are trusted by hash, not just declared.** Each entry has a `trusted_hash = "sha256:…"` in
that `[hooks.state]` table; `codex exec --dangerously-bypass-hook-trust` exists precisely to run an
untrusted one for a single invocation. A person pasting the printed block into
`~/.codex/hooks.json` therefore has a second step — accepting it once when Codex asks — and
editing the hook script later invalidates the hash, which is a feature and not a fault.

**Nothing here writes that file.** It is machine-level: an entry added from inside one repository
would fire in every other repository on the machine, which is not a side effect a `--repo` command
gets to have. Releases before 0.7.0 copied the hook script into `.codex/hooks/` and printed the
JSON to paste; since 0.7.0 the hooks ship in the plugin (below), and `install-agent --codex` takes
a pasted zerograph entry back out of `$CODEX_HOME/hooks.json`, since the plugin's would fire
beside it.

Max, 2026-09-07: Codex gets the brief, the skill and the risk line, and **not** the Bash
interceptor — its verdict is being read on Claude Code and is not carried over blind. The plugin's
hooks reflect that: three events, no `PreToolUse`.

## The plugin: `agent/codex/`

Established on 0.154.0 with a scratch `CODEX_HOME` and a local marketplace, no model call:

- A plugin is a directory with `.codex-plugin/plugin.json`, which names `skills`, `mcpServers` and
  `hooks` by relative path. A repository's `.agents/plugins/marketplace.json` lists it, and Codex
  prefers that file to `.claude-plugin/marketplace.json` in the same repository.
  `codex plugin marketplace add <source> [--ref <tag>]` and `codex plugin add zerograph@zerograph`
  are both idempotent, exit 0 on a repeat, and copy the plugin to
  `$CODEX_HOME/plugins/cache/zerograph/zerograph/<version>/`.
- Hook commands run through a shell with `PLUGIN_ROOT` (and `CLAUDE_PLUGIN_ROOT`) set, in the
  session's project directory, with the payload's `cwd` naming it too. A `PostToolUse` matcher of
  `apply_patch|Edit|Write` is honoured; `apply_patch` is Codex's edit tool.
- An MCP server's `args` are passed as written — no `${PLUGIN_ROOT}` is substituted — and its
  environment is scrubbed down to Codex's defaults (`HOME`, `LANG`, `LC_ALL`, `LOGNAME`, `PATH`,
  `SHELL`, `TERM`, `TMPDIR`, `USER` on macOS) and what `env_vars` names. With no `cwd` it starts in
  the project. `command` takes only a bare name or a `./` path, and `cwd` only a `./` path — which
  resolves to the plugin root, losing the project — so `${PLUGIN_ROOT}` reaches the server nowhere.
  `.mcp.json` therefore runs `node -e` with a bootstrap that imports `scripts/mcp.mjs` from the
  newest version directory under `$CODEX_HOME/plugins/cache/zerograph/zerograph/`.
- `codex plugin marketplace add` with a different `--ref` than the configured one fails ("already
  added from a different source"); `marketplace remove` then `add` repins it and keeps the plugin
  enabled, and `codex plugin add` replaces the old version directory with the new one.
- Hook handlers take a `commandWindows` override, used on Windows instead of `command`.
- A skill's `agents/openai.yaml` with `policy.allow_implicit_invocation: false` keeps it out of the
  model's own choices, so the commands run only when named, as `$zerograph:<name>`.

The scripts are copies of `agent/claude-code/scripts/` — a plugin is installed as one directory and
cannot import from a sibling, and a symlink out of it would be a text file in a Windows checkout
without `core.symlinks` — and `plugin.test.mjs` fails when they differ;
`scripts/sync-plugin-scripts.sh` copies them over.

## Skills: `.codex/skills/<name>/SKILL.md`

Confirmed by running it: a scratch repository carrying `.codex/skills/probe-codex/SKILL.md` and
`.agents/skills/probe-agents/SKILL.md` had **both** listed under *Available skills* in
`codex debug prompt-input`'s output, with the roots table naming each directory. So
`.codex/skills/zerograph/SKILL.md` is read, which is where releases before 0.7.0 put the skill and
where `install-agent --codex` now removes it from, the plugin's skills replacing it; `.agents/skills`
is the other repository-scoped root, and is left alone because a repository that uses it has its own
conventions there.

Only the *description* line of each skill is in the prompt — the body is fetched on demand — which
is why the skill's front matter says when to use it and the body says how.

## What has not been established

Whether Codex delivers `additionalContext` to the model, on `SessionStart` or on `SubagentStart`.
The event names are in the binary and `SessionStart` hooks demonstrably *run* — the CLI prints
`hook: SessionStart` and a completion line for each — but proving what reaches the model needs a
hook Codex will load, and the only two sources are the machine's own `hooks.json` and an installed
plugin. Editing either to run a probe is a change to every repository on the machine, which is the
one thing this installer refuses to do on its own; so the delivery contract stays unproven, and the
printed JSON stays something a person opts into rather than something this command performs.

An earlier attempt to sidestep that by copying `auth.json` into a scratch `CODEX_HOME` spent the
machine's ChatGPT refresh token. Never again: probe with the real `CODEX_HOME` and a scratch
*repository*, which is what settled the hooks question above and cost no model call — the hook
lines are printed before the model is reached.
