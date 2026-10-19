# Install

```bash
pnpm add -D zerograph              # npm: prebuilt binary for macOS arm64, Linux x64 and Windows x64
npm install -g zerograph           # or globally; pnpm add -g zerograph works the same
cargo install --path .             # from source; Rust 1.98, pinned in rust-toolchain.toml
```

The plugins `install-agent --claude` and `--codex` install run the `zerograph` on `PATH` and never
one inside the checkout, so a global install is the simplest route. With only a project-local
install (`pnpm add -D`), name that binary's absolute path: in the Claude Code plugin's `binary`
option (`/config`), or in `ZEROGRAPH_BIN`, written out
(`export ZEROGRAPH_BIN=/home/me/work/app/node_modules/.bin/zerograph`). A relative path is ignored,
and `$PWD` in a shell profile names whichever checkout a shell opens in, so a cloned repository
would supply the binary again. See [the session hook](keeping-fresh.md#the-session-hook).

npm is the recommended route on every platform: it carries a provenance attestation, and on
Windows it never meets SmartScreen (see [Windows notes](#windows-notes)).

The npm package carries all three binaries and a launcher that runs the one for this platform,
so a lockfile written on one platform installs on the others, nothing runs at install time, and
nothing is fetched beyond the package itself (a 56 MB download, about 300 MB installed). It is on the public npm registry and
needs no `.npmrc`. Tagged releases (`v*`) build all three binaries as GitHub release assets (a tarball
for the unix ones, a zip holding `zerograph.exe` for Windows) and cut the npm package from those
same files (`.github/workflows/release.yml`, publishing through npm's trusted publishing, so each
version carries a provenance attestation; `scripts/npm-pack.sh` packs the same tarball by hand).

## Verifying a download

Each release binary is built by `.github/workflows/release.yml` on a GitHub runner, and that
workflow signs a build provenance attestation for both the archive and the binary inside it, through
Sigstore, under its own identity. With the GitHub CLI, any copy can be checked against it — the
archive from the release page, or the binary an npm install unpacked:

```bash
gh attestation verify zerograph-x86_64-pc-windows-msvc.zip -R devmaxxx/zerograph
gh attestation verify "$(npm root -g)/zerograph/bin/linux-x64/zerograph" -R devmaxxx/zerograph
```

A pass names the repository, the workflow and the commit the file was built from. Every release
also carries `SHA256SUMS` for its archives, for a check that needs nothing but a hash tool:

```bash
sha256sum -c SHA256SUMS --ignore-missing                                   # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing                               # macOS
(Get-FileHash zerograph-x86_64-pc-windows-msvc.zip).Hash                   # Windows, compare by eye
```

The npm package itself is published through trusted publishing with npm provenance: its page on
npmjs.com links the workflow run that built it, and `npm audit signatures` checks it locally.

## Upgrading

Re-run `zerograph install-agent` with the flags you used before. For the editors it replaces its
own instructions stanza rather than adding a second one. For Claude Code and Codex it installs the
plugin, then takes out what releases before 0.7.0 wrote, which the plugin now does:

- `--claude`: the `zerograph` server registered for the checkout and its worktrees in
  `~/.claude.json`, the hook entries in `.claude/settings.json` (the file goes when nothing else is
  left in it), `.claude/hooks/zerograph-hook.mjs` and its `rule.txt`, the `zerograph` and
  `repo-query` skills, the `repo-scout` subagent, and the zerograph stanza in `CLAUDE.md`.
- `--codex`: the zerograph hook entries in `~/.codex/hooks.json` (or under `$CODEX_HOME`),
  `.codex/hooks/zerograph-hook.mjs`, the `.codex/skills/zerograph/` and `repo-query` skills, and
  the stanza in `AGENTS.md` unless `--opencode` still reads it there.

A hook script goes only after the entries that run it are gone. A hook script, skill or subagent
somebody edited, a link, or a directory with files of its own is left and named. A file git tracks
is left too, and listed in one line: it is the team's, and serves every teammate who has no plugin
yet. Once all of them have it, `zerograph install-agent --claude --retire-repo` (or `--codex
--retire-repo`) takes those out as well, for a commit of their own. Until then the plugin's hook
stays silent on every event the committed entries in `.claude/settings.json` still answer — in
Codex, the entries left in `$CODEX_HOME/hooks.json` — so a session hears one brief, not two. Both take the `post-merge`/`post-checkout` block an earlier release wrote out of the git
hooks, deleting a hook that held nothing else. The install runs only once the plugin installed: when
the `claude` or `codex` CLI is missing or a step fails, it prints the commands to run by hand and
leaves the older install in place.

### A newer release

Once a day an interactive run asks npm for zerograph's latest release
(`https://registry.npmjs.org/zerograph/latest`) on a thread of its own that no command waits for,
and keeps the answer in `~/.cache/zerograph/update.json`, one entry per installed release. A run
that ends before npm answers is retried within the hour. When the cached answer is newer than the
binary, the run ends with a box on stderr:

```
╭────────────────────────────────────╮
│                                    │
│   Update available 0.7.0 → 0.7.1   │
│    Run npm i -g zerograph@0.7.1    │
│   or pnpm add -g zerograph@0.7.1   │
│                                    │
╰────────────────────────────────────╯
```

It names npm and pnpm whichever way the binary was installed; a Cargo or archive install updates
the way it was installed. `mcp`, `serve`, `status` and any `--json` run neither ask nor print, nor
does a run whose stderr is not a terminal or one with `CI` set. `zerograph status` and the MCP
`status` tool report the cached release as an `update:` line (`update` in `status --json`),
without asking npm.

The Claude Code plugin reads the same file and asks nobody. At session start, when the entry for
the binary it runs names a newer release, it shows

```
▲ zerograph 0.7.1 available (you have 0.7.0)
  npm i -g zerograph@0.7.1 · pnpm add -g zerograph@0.7.1
```

and the hint under the prompt ends in `↑0.7.1`. `ZEROGRAPH_NO_UPDATE_CHECK` or
`NO_UPDATE_NOTIFIER`, set to anything, turns all of it off — see
[Update notices](configuration.md#update-notices).

<a id="claude-code-status-mod"></a>

## Claude Code plugin

`agent/claude-code/` is a Claude Code plugin. Its mod puts the index's state at the end of the line
under the prompt: `◇ zerograph · in sync`, a bar and the time left while a refresh runs, the files
behind, vectors still to embed, a warning count and whether `serve` answers. `/zerograph` prints
the full `zerograph status` report. Install it at a Claude Code prompt:

```
/plugin install zerograph --marketplace devmaxxx/zerograph
```

It also starts zerograph's [MCP server](mcp.md) for the session, always: the plugin declares it in
`.claude-plugin/plugin.json`, and Claude Code connects it whenever the plugin is enabled. Its slash
commands call that server's tools, never a shell, and tell the model how to read what came back:

| command | MCP tools | then |
| --- | --- | --- |
| `/zerograph:ask <question>` | `ask` | opens the top seeds at their `path:line` and answers from them |
| `/zerograph:impact <Symbol> [--depth N] [--down]` | `impact` | leads with the risk line and the depth-1 callers; checks a "nothing uses this" with a text search |
| `/zerograph:trace <From> <To>` | `trace` | walks the chain step by step; no path is an answer |
| `/zerograph:changes [base]` | `changes` | the callers outside the diff a change can break, before a commit or a PR |
| `/zerograph:explain <id>` | `explain` | the node and the edges the question is about |
| `/zerograph:status` | `status`, `cache_status` | what, if anything, wants doing |
| `/zerograph:refresh` | `reindex`, `status` | a first `build` where there is no store, else an `update`, in the background |

Each command pre-approves only the tools it calls, as `mcp__plugin_zerograph_zerograph__<tool>`. A
skill, `use-the-graph`, tells the model to reach for those tools rather than grep when the question
is an exact id, who calls a symbol, or what a change would break.

The server is `zerograph --repo <checkout> mcp`, started by `scripts/mcp.mjs` under `node`: the
`node` that `npm i -g zerograph` needed, which an install from a release archive or `cargo` has to
put on `PATH` itself. The repository is the checkout around `CLAUDE_PROJECT_DIR`, so a session
opened in a package of a monorepo indexes the whole repository; outside git it is the nearest
directory above holding a `.zerograph/`, else the project directory. Claude Code starts that `node`
by name from its own `PATH`, which the plugin cannot pin; a checkout whose directory is on that
`PATH`, or on Windows the directory the server starts in, could put its own `node` there.

**With `install-agent --claude`.** It installs this plugin through the `claude` CLI (`claude plugin
marketplace add devmaxxx/zerograph`, `claude plugin marketplace update zerograph`, `claude plugin
install zerograph@zerograph`, then `claude plugin update zerograph@zerograph`, so a re-run after
upgrading the binary moves the plugin to the same release) and removes
the server, hook, skill and stanza an older `install-agent --claude` wrote (see
[Upgrading](#upgrading)), so the model never sees the tools twice, as `mcp__zerograph__*` and as
`mcp__plugin_zerograph_zerograph__*`.

The plugin's hooks are the [session hook](keeping-fresh.md#the-session-hook): at session start the
index brief, for each subagent the rule to ask the graph first, before a concept grep the graph's
answer, and after an edit to a TypeScript file the
callers outside it that the edit can break. A checkout hears from them only once it has a
`zerograph.toml` or an index of its own, or is a worktree of a main checkout that has one.

It asks the `zerograph` on `PATH` by its absolute path, and never a binary inside the project: a
cloned repository that ships `node_modules/.bin/zerograph` or `target/*/zerograph` does not get it
run, whether `PATH` names that directory relatively or absolutely. For a project-local install,
name its absolute path in the `binary` option in `/config`; a relative path is ignored, and a bare
name such as `zerograph` means the `PATH` lookup. On Windows a `zerograph.exe` on `PATH` is asked
before npm's `zerograph.cmd` shim, which the mod runs through `cmd`. A zerograph without `status`
shows nothing on the line, and `/zerograph` falls back to the tail of `background.log`. The MCP
server's binary is found by the same rules, and a binary option that is not an absolute path, or no
zerograph on `PATH` outside the checkout, fails its start with the reason in the server's log
(`/mcp` shows it). So does a `zerograph.cmd` shim when its path or the checkout's holds a character
`cmd` would act on (`& | < > ^ " % !`): name the `zerograph.exe` the npm package ships in the
`binary` option instead. The server runs with `PATH` cut to the entries outside the checkout.

`zerograph status --json` walks the whole tree to count the files behind, so the mod runs it at
session start, every five minutes, once after a refresh ends and after `/zerograph`. Between those
it reads the live state every ten seconds from the store's `background.log` (whether a refresh runs
and how far it has got) and from the `serve` socket, at the path `status --json` names as
`serve.socket`; a zerograph whose report has no `serve.socket` is looked for at
`.zerograph/serve.sock`. A socket file is all the mod can see between full reports, so a server
killed without removing its socket reads as answering until the next full report asks it.

The marketplace pins the mod to a release tag and the mod carries that release's version, so it
moves with zerograph: `claude plugin update zerograph` then `/reload-plugins` takes the mod of the
latest release, and nothing merged to `main` reaches an install before its tag. A version bump
changes `Cargo.toml`, `agent/claude-code/.claude-plugin/plugin.json`,
`agent/codex/.codex-plugin/plugin.json` and the `ref` in `.claude-plugin/marketplace.json`
together, and the `--ref` in the Codex command below; CI fails when the first four disagree. A fix
to `agent/claude-code/scripts/` reaches the Codex plugin's copies through
`scripts/sync-plugin-scripts.sh`, and CI fails until it does.

## Codex plugin

`agent/codex/` is the same integration as a Codex plugin, listed by `.agents/plugins/marketplace.json`:

```bash
codex plugin marketplace add devmaxxx/zerograph --ref v0.7.1
codex plugin add zerograph@zerograph
```

`zerograph install-agent --codex` runs those two and the clean-up in [Upgrading](#upgrading). Codex
refuses to re-add a marketplace under another ref, so after a binary upgrade the run removes the
`zerograph` marketplace and adds it at the new tag; the plugin stays enabled. The
plugin starts the MCP server (`scripts/mcp.mjs`, the Claude Code plugin's, found under
`$CODEX_HOME/plugins/cache/zerograph/zerograph/<version>/`), runs the session hook at session start,
for each subagent and after an `apply_patch`, and carries the commands as skills: `$zerograph:ask`,
`$zerograph:impact`, `$zerograph:trace`, `$zerograph:changes`, `$zerograph:explain`,
`$zerograph:status` and `$zerograph:refresh` run only when named, and `use-the-graph` is the one the
model picks up by itself. Codex asks once before it trusts a plugin's hooks. Codex hands the server
its own short list (`PATH`, `HOME`, `LANG`, `TMPDIR` and a few more) plus what `.mcp.json` names:
`CODEX_HOME`, the `ZEROGRAPH_*` settings, the model cache and Hugging Face variables, the
`HTTP(S)_PROXY`/`NO_PROXY`/`ALL_PROXY` pair in both cases, the certificate paths and `LC_*`. A
project-local binary goes in `ZEROGRAPH_BIN`. Codex has no hook before a shell command, so nothing
answers a grep before it runs.

On Windows the hooks run their `commandWindows` form, which reads `PLUGIN_ROOT` inside `node`
instead of through a POSIX shell's `${PLUGIN_ROOT}`; it is checked under a POSIX shell in CI but has
not run under Codex on a Windows machine yet.

## Windows notes

Install through npm; nothing else is required. What can still trip a Windows user:

- **Floor.** Windows 10 1903, Windows 11 or Server 2022. The binary links ONNX Runtime's DirectML
  build, so an older system fails at load. Inference runs on the CPU.
- **SmartScreen.** The binary is not Authenticode-signed. A zip fetched with a browser and opened
  through Explorer shows "Windows protected your PC"; npm, pnpm, `curl` and `gh release download`
  never do. `Unblock-File .\zerograph.exe` clears it. [Verifying a download](#verifying-a-download)
  proves the file is ours.
- **Defender.** It scans the model as it lands (about 200 MB for the default) and the store on every
  write; the first download prints the `Add-MpPreference -ExclusionPath` line, which is optional. A
  heuristic false positive is restored from Protection History.
- **Serve.** `zerograph serve --detach` starts one with no window, `zerograph stop` ends it and
  removes its socket. `npm i -g` upgrades under one a 0.6.1 or later launcher started; a
  0.6.0 server, or `cargo install`, needs the server stopped first.
- **PowerShell encoding.** Output is UTF-8, but a *piped or captured* answer is decoded with
  `[Console]::OutputEncoding`, so Cyrillic turns to mojibake on a non-UTF-8 code page. Set it to
  UTF-8 in your profile.
- **Git Bash.** `FASTEMBED_CACHE_DIR`, `XDG_CONFIG_HOME` and `ZEROGRAPH_CONFIG` must be Windows-form
  paths even when set there.
- **Long paths.** A tree deeper than 260 characters needs `git config core.longpaths true`.
- **Synced folders.** Keep the repository out of OneDrive, Dropbox or Google Drive: every store
  write is a rename the client re-uploads.
