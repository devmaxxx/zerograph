// The plugin's MCP launcher, exercised the way `.claude-plugin/plugin.json` runs it: the binary
// option as its one argument, the project in CLAUDE_PROJECT_DIR, and a fake `zerograph` that
// answers one MCP request. Run: `node --test agent/claude-code/scripts/mcp.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, mkdirSync, writeFileSync, chmodSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

import { binaryOf, argvOf, checkoutOf } from './mcp.mjs';

const LAUNCHER = join(dirname(fileURLToPath(import.meta.url)), 'mcp.mjs');
const WIN = process.platform === 'win32';

/** A checkout that ships a zerograph where a local install puts one, and one on the machine. */
function world() {
  const base = realpathSync(mkdtempSync(join(tmpdir(), 'zerograph-mcp-test-')));
  const checkout = join(base, 'checkout');
  mkdirSync(join(checkout, '.git'), { recursive: true });
  mkdirSync(join(checkout, 'packages', 'app'), { recursive: true });
  const local = join(checkout, 'node_modules', '.bin');
  const machine = join(base, 'machine');
  for (const dir of [local, machine]) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, WIN ? 'zerograph.exe' : 'zerograph'), '');
  }
  return { base, checkout, local, machine };
}

const sep = WIN ? ';' : ':';

test('an unset option, substituted or not, is the PATH lookup', () => {
  const w = world();
  const path = [w.machine].join(sep);
  for (const option of ['', '${user_config.binary}', 'zerograph']) {
    assert.strictEqual(binaryOf(option, { path, checkout: w.checkout, win: WIN }).bin, join(w.machine, WIN ? 'zerograph.exe' : 'zerograph'), option);
  }
});

test('a PATH entry inside the checkout is passed over, relative or absolute', () => {
  const w = world();
  const path = ['node_modules/.bin', w.local, w.machine].join(sep);
  assert.strictEqual(binaryOf('', { path, checkout: w.checkout, win: WIN }).bin, join(w.machine, WIN ? 'zerograph.exe' : 'zerograph'));
});

test('an absolute option is the one started, even inside the checkout', () => {
  const w = world();
  const named = join(w.local, 'zerograph');
  assert.strictEqual(binaryOf(named, { path: w.machine, checkout: w.checkout, win: WIN }).bin, named);
});

test('a relative option is refused by name rather than read against some directory', () => {
  const w = world();
  const found = binaryOf('node_modules/.bin/zerograph', { path: w.machine, checkout: w.checkout, win: WIN });
  assert.strictEqual(found.bin, undefined);
  assert.match(found.error, /node_modules\/\.bin\/zerograph\) is not an absolute path/);
});

test('nothing on PATH outside the checkout is an error that says how to fix it', () => {
  const w = world();
  assert.match(binaryOf('', { path: w.local, checkout: w.checkout, win: WIN }).error, /no zerograph on PATH outside this checkout/);
});

test('the server is told the checkout around the project, not a package inside it', () => {
  const w = world();
  assert.strictEqual(checkoutOf(join(w.checkout, 'packages', 'app')), w.checkout);
  assert.deepStrictEqual(argvOf('/usr/local/bin/zerograph', w.checkout), ['/usr/local/bin/zerograph', '--repo', w.checkout, 'mcp']);
});

test('an npm .cmd shim is started through cmd, an .exe directly', () => {
  assert.deepStrictEqual(argvOf('C:\\npm\\zerograph.cmd', 'C:\\work', 'C:\\Windows\\cmd.exe'), ['C:\\Windows\\cmd.exe', '/d', '/c', 'call', 'C:\\npm\\zerograph.cmd', '--repo', 'C:\\work', 'mcp']);
  assert.deepStrictEqual(argvOf('C:\\bin\\zerograph.exe', 'C:\\work'), ['C:\\bin\\zerograph.exe', '--repo', 'C:\\work', 'mcp']);
});

// The fake is a Node script so the protocol round trip reads the same on every platform.
test('the launcher hands stdin and stdout to the server untouched, and the server its checkout', { skip: WIN && 'a POSIX launcher for the fake' }, () => {
  const w = world();
  const fake = join(w.machine, 'zerograph');
  writeFileSync(fake, `#!/usr/bin/env node
let input = '';
process.stdin.on('data', d => { input += d; });
process.stdin.on('end', () => {
  const req = JSON.parse(input.trim());
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: req.id, result: { argv: process.argv.slice(2) } }) + '\\n');
});
`);
  chmodSync(fake, 0o755);
  const r = spawnSync(process.execPath, [LAUNCHER, ''], {
    cwd: w.base,
    input: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' }) + '\n',
    env: { PATH: [dirname(process.execPath), w.machine].join(sep), CLAUDE_PROJECT_DIR: join(w.checkout, 'packages', 'app') },
    encoding: 'utf8',
  });
  assert.strictEqual(r.status, 0, r.stderr);
  assert.deepStrictEqual(JSON.parse(r.stdout), { jsonrpc: '2.0', id: 1, result: { argv: ['--repo', w.checkout, 'mcp'] } });
});

test('no binary is a line in the server log and a failed start, never a word on stdout', () => {
  const w = world();
  const r = spawnSync(process.execPath, [LAUNCHER, 'relative/zerograph'], { cwd: w.base, input: '', env: { PATH: w.machine, CLAUDE_PROJECT_DIR: w.checkout }, encoding: 'utf8' });
  assert.strictEqual(r.status, 1);
  assert.strictEqual(r.stdout, '');
  assert.match(r.stderr, /relative\/zerograph\) is not an absolute path/);
});
