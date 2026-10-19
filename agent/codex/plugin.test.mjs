// The Codex plugin against the Claude Code one it mirrors: the same scripts byte for byte, the same
// commands as skills, one version everywhere, and the MCP bootstrap started the way Codex starts
// it. Run: `node --test agent/codex/plugin.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, writeFileSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const CODEX = dirname(fileURLToPath(import.meta.url));
const REPO = join(CODEX, '..', '..');
const CLAUDE = join(REPO, 'agent', 'claude-code');
const json = (...p) => JSON.parse(readFileSync(join(...p), 'utf8'));
const WIN = process.platform === 'win32';

test('the scripts are the Claude Code plugin\'s, byte for byte', () => {
  for (const file of readdirSync(join(CLAUDE, 'scripts')).filter((f) => f.endsWith('.mjs') && !f.endsWith('.test.mjs'))) {
    assert.ok(existsSync(join(CODEX, 'scripts', file)), `agent/codex/scripts/${file} is missing`);
    assert.equal(readFileSync(join(CODEX, 'scripts', file), 'utf8'), readFileSync(join(CLAUDE, 'scripts', file), 'utf8'),
      `agent/codex/scripts/${file} differs; copy it from agent/claude-code/scripts`);
  }
});

test('one version: both plugins, the bootstrap\'s cache path and the crate', () => {
  const version = json(CODEX, '.codex-plugin', 'plugin.json').version;
  assert.equal(json(CLAUDE, '.claude-plugin', 'plugin.json').version, version);
  assert.equal(readFileSync(join(REPO, 'Cargo.toml'), 'utf8').match(/^version = "(.+)"$/m)[1], version);
  // Codex installs a plugin under its version, and the bootstrap names that directory.
  assert.ok(json(CODEX, '.mcp.json').mcpServers.zerograph.args.join(' ').includes(`'${version}'`));
});

test('every Claude Code command is a Codex skill, explicit-only where the command is', () => {
  const front = (file) => readFileSync(file, 'utf8').match(/^---\n([\s\S]*?)\n---\n/)[1];
  const field = (text, k) => text.match(new RegExp(`^${k}: (.*)$`, 'm'))?.[1];
  const names = readdirSync(join(CLAUDE, 'skills'));
  assert.deepEqual(readdirSync(join(CODEX, 'skills')).sort(), names.sort());
  for (const name of names) {
    const theirs = front(join(CLAUDE, 'skills', name, 'SKILL.md'));
    const ours = join(CODEX, 'skills', name);
    const text = readFileSync(join(ours, 'SKILL.md'), 'utf8');
    assert.equal(field(front(join(ours, 'SKILL.md')), 'name'), name);
    assert.equal(field(front(join(ours, 'SKILL.md')), 'description'), field(theirs, 'description'), name);
    assert.doesNotMatch(text, /mcp__|\$ARGUMENTS|\/zerograph:/, `${name} names something only Claude Code has`);
    const explicit = /disable-model-invocation: true/.test(theirs);
    assert.match(readFileSync(join(ours, 'agents', 'openai.yaml'), 'utf8'), new RegExp(`allow_implicit_invocation: ${!explicit}`), name);
  }
});

test('the hooks run the shipped hook, and the marketplace points at this directory', () => {
  const hooks = json(CODEX, 'hooks', 'hooks.json').hooks;
  assert.deepEqual(Object.keys(hooks).sort(), ['PostToolUse', 'SessionStart', 'SubagentStart']);
  for (const groups of Object.values(hooks)) {
    for (const h of groups.flatMap((g) => g.hooks)) assert.equal(h.command, 'node "${PLUGIN_ROOT}/scripts/hook.mjs"');
  }
  const market = json(REPO, '.agents', 'plugins', 'marketplace.json');
  assert.equal(market.name, 'zerograph');
  assert.equal(realpathSync(join(REPO, market.plugins[0].source.path)), realpathSync(CODEX));
  const manifest = json(CODEX, '.codex-plugin', 'plugin.json');
  for (const key of ['skills', 'mcpServers', 'hooks']) assert.ok(existsSync(join(CODEX, manifest[key])), key);
});

test('the MCP bootstrap finds the installed plugin under CODEX_HOME and serves the project it starts in', { skip: WIN && 'a POSIX launcher for the fake' }, () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), 'zerograph-codex-test-')));
  const version = json(CODEX, '.codex-plugin', 'plugin.json').version;
  const home = join(base, 'codex-home');
  cpSync(CODEX, join(home, 'plugins', 'cache', 'zerograph', 'zerograph', version), { recursive: true });
  const project = join(base, 'project');
  mkdirSync(join(project, '.git'), { recursive: true });
  mkdirSync(join(project, 'src'));
  const bin = join(base, 'bin');
  mkdirSync(bin);
  const seen = join(base, 'seen.txt');
  writeFileSync(join(bin, 'zerograph'), `#!/bin/sh\necho "$@" > '${seen}'\n`);
  chmodSync(join(bin, 'zerograph'), 0o755);
  const server = json(CODEX, '.mcp.json').mcpServers.zerograph;
  // Codex hands the server only the variables it names and a few of its own, and no `cwd` keeps
  // the session's project as the working directory.
  const env = { PATH: `${bin}:${process.env.PATH}`, HOME: base, CODEX_HOME: home };
  const r = spawnSync(server.command, server.args, { cwd: join(project, 'src'), env, encoding: 'utf8' });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(readFileSync(seen, 'utf8').trim(), `--repo ${project} mcp`);
});
