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
gets to have. `install-agent --codex` therefore installs the repository-scoped half — the stanza and
the skill — copies the hook script into `.codex/hooks/` so there is something to point at, and
prints the JSON to add to `~/.codex/hooks.json`, for the reader to paste once if they want it. The
path it prints is the main worktree's copy (the first entry of `git worktree list --porcelain`),
written there when the install runs in a linked worktree and the main one has none, so removing a
worktree does not leave every Codex session pointing at a missing file.

Max, 2026-09-07: Codex gets the brief, the skill and the risk line, and **not** the Bash
interceptor — its verdict is being read on Claude Code and is not carried over blind. The printed
JSON reflects that: three events, no `PreToolUse`.

## Skills: `.codex/skills/<name>/SKILL.md`

Confirmed by running it: a scratch repository carrying `.codex/skills/probe-codex/SKILL.md` and
`.agents/skills/probe-agents/SKILL.md` had **both** listed under *Available skills* in
`codex debug prompt-input`'s output, with the roots table naming each directory. So
`.codex/skills/zerograph/SKILL.md` is read, which is where the installer puts it; `.agents/skills`
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
