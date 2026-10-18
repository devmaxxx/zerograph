# Install

```bash
pnpm add -D zerograph              # npm: prebuilt binary for macOS arm64, Linux x64 and Windows x64
npm install -g zerograph           # or globally; pnpm add -g zerograph works the same
cargo install --path .             # from source; Rust 1.98, pinned in rust-toolchain.toml
```

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

## Coming from repograph

zerograph was called repograph until 0.6.0. Nothing needs rebuilding or re-downloading, and
nothing is copied or deleted, but whatever still runs as `repograph` has to stop first: the first
write by zerograph moves the store (below) out from under it.

1. **Stop what runs as `repograph`.** A `repograph serve` (an agent session starts one in the
   background) or a `repograph watch` that outlives the move reads the store as empty and builds a
   second `.repograph/` of its own. `pkill -f 'repograph.* (serve|watch)'`, and end the agent
   sessions that use the old MCP server.
2. **Swap the package.** `pnpm remove @devmaxxx/repograph`, then `pnpm add -D zerograph`, and
   change scripts that call `repograph` to call `zerograph`: there is no command under the old name,
   and `@devmaxxx/repograph` gets no release after 0.5. Installed globally, `npm uninstall -g
   @devmaxxx/repograph` or `pnpm remove -g @devmaxxx/repograph`. Installed from source, run `cargo
   uninstall repograph`, or the old binary stays on `PATH` and goes on writing a `.repograph/` of
   its own. zerograph comes from the public npm registry, so the `@devmaxxx` lines in `~/.npmrc` can
   go.
3. **Re-run `zerograph install-agent`** with the flags you used before, straight away. The old hook
   script looks for `.repograph/` only, so once the store has moved it says nothing, and the
   install is what replaces it.

What zerograph reads from the old install:

- **The store.** A `.repograph/` directory is read where it stands. The first write — `build`,
  `update`, a reader's inline refresh, `serve` starting — renames it to `.zerograph/` in one step,
  `.gitignore` inside it included. When both exist, `.zerograph/` is the store and `.repograph/` is
  left alone for you to remove; one that comes back after the move was made by something from step
  1 or 2. A symlink at either name is refused, as it always was.
- **The project file.** `zerograph.toml` is read, else `repograph.toml`; `zerograph model` rewrites
  whichever one it found. With both present only `zerograph.toml` is read, and a notice says so.
  `git mv repograph.toml zerograph.toml` when convenient.
- **The machine.** `~/.config/repograph/config.toml` is read while there is no
  `~/.config/zerograph/config.toml`; with both, only the new one is read and a notice says so, so
  move what the old one set before creating the new. A model trusted under either directory stays
  trusted; new trusts are written to the new one. Each model cache under `~/.cache/repograph/` is
  used in place until one under `~/.cache/zerograph/` exists.
- **The environment.** Every `REPOGRAPH_*` variable is read while its `ZEROGRAPH_*` name is unset;
  a `ZEROGRAPH_*` that is set, even to nothing, wins.
- **Agents and git hooks.** The install in step 3 replaces the old instructions stanza, settings
  entries and git hook blocks rather than adding second ones, deletes
  `.claude/hooks/repograph-hook.mjs`, and removes the local-scope Claude Code MCP server registered
  as `repograph` once `zerograph` is registered (one in another scope is named, with the command
  that removes it). Codex's old script stays until `~/.codex/hooks.json` points at
  `zerograph-hook.mjs`; the install prints the block to paste.
- **The skill (0.6.4).** It is `zerograph` now, in `.claude/skills/zerograph/` and
  `.codex/skills/zerograph/`, with the release that wrote it as `metadata.version`. The install
  removes the `repo-query` directory beside it when it holds only the `SKILL.md` 0.6.0–0.6.3 wrote;
  an edited one, a link or one with files of its own is left and named.

## Claude Code status mod

`agent/claude-code/` is a Claude Code mod that puts the index's state at the end of the line under
the prompt, read from `zerograph status --json` every ten seconds: `◇ zerograph · in sync`, a bar
and the time left while a refresh runs, the files behind, vectors still to embed, a warning count
and whether `serve` answers. `/zerograph` prints the full `zerograph status` report. Install it at
a Claude Code prompt:

```
/plugin install zerograph-status --marketplace devmaxxx/zerograph
```

It asks `zerograph` on `PATH`, then the project's `node_modules/.bin`, then `target/release` and
`target/debug`; the `binary` option in `/config` names another one. A zerograph without `status`
shows nothing on the line, and `/zerograph` falls back to the tail of `background.log`.

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
  write is a rename the client re-uploads, and the socket is a reparse point they do not know.
