# Install

```bash
pnpm add -D zerograph              # npm: prebuilt binary for macOS arm64, Linux x64 and Windows x64
npm install -g zerograph           # or globally; pnpm add -g zerograph works the same
cargo install --path .             # from source; Rust 1.98, pinned in rust-toolchain.toml
```

The session hooks `install-agent` writes run the `zerograph` on `PATH` and never one inside the
checkout, so a global install is the simplest route. With only a project-local install (`pnpm add
-D`), set `ZEROGRAPH_BIN` to that binary's absolute path, written out
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

Re-run `zerograph install-agent` with the flags you used before. It replaces its own instructions
stanza and settings entries rather than adding second ones, and takes the
`post-merge`/`post-checkout` block an earlier release wrote out of the git hooks, deleting a hook
that held nothing else.

Since 0.6.4 the skill is `zerograph`, in `.claude/skills/zerograph/` and
`.codex/skills/zerograph/`, with the release that wrote it as `metadata.version`. The install removes
the `repo-query` directory beside it when it holds only the `SKILL.md` 0.6.0–0.6.3 wrote; an edited
one, a link or one with files of its own is left and named.

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

**With `install-agent --claude`.** The plugin's server replaces the one `install-agent --claude`
registers. Claude Code connects a plugin's server beside a local-scope one of the same name when
their commands differ, so the two would both run, and the model would see every tool twice, as
`mcp__zerograph__*` and as `mcp__plugin_zerograph_zerograph__*`. With the plugin installed, remove
the local one in each checkout that has it: `claude mcp remove zerograph -s local`. The session
hook and the skill `install-agent --claude` wrote can stay; only the server doubles. A later
`install-agent --claude` registers the server again, so remove it again after one.

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
changes `Cargo.toml`, `agent/claude-code/.claude-plugin/plugin.json` and the `ref` in
`.claude-plugin/marketplace.json` together; CI fails when they disagree.

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
