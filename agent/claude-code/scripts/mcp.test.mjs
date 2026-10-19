// The plugin's MCP launcher, exercised the way `.claude-plugin/plugin.json` runs it: the binary
// option as its one argument, the project in CLAUDE_PROJECT_DIR, and a fake `zerograph` that
// answers one MCP request. Run: `node --test agent/claude-code/scripts/mcp.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, chmodSync, realpathSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

import { binaryOf, argvOf, checkoutOf, childEnv } from './mcp.mjs';
import { machineDirs, pathKey } from './launcher.mjs';

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
    const file = join(dir, WIN ? 'zerograph.exe' : 'zerograph');
    writeFileSync(file, '');
    chmodSync(file, 0o755);
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

test('a symlinked alias of the checkout on PATH is the checkout', { skip: WIN && 'symlinks need a privilege' }, () => {
  const w = world();
  const alias = join(w.base, 'alias');
  symlinkSync(w.checkout, alias);
  const path = [join(alias, 'node_modules', '.bin'), w.machine].join(sep);
  assert.strictEqual(binaryOf('', { path, checkout: w.checkout, win: WIN }).bin, join(w.machine, 'zerograph'));
  assert.strictEqual(binaryOf('', { path, checkout: alias, win: WIN }).bin, join(w.machine, 'zerograph'), 'a session opened through the alias');
});

test('a file on PATH that is not executable is not a zerograph', { skip: WIN && 'no exec bit' }, () => {
  const w = world();
  const plain = join(w.base, 'plain');
  mkdirSync(plain);
  writeFileSync(join(plain, 'zerograph'), '');
  assert.strictEqual(binaryOf('', { path: [plain, w.machine].join(sep), checkout: w.checkout, win: WIN }).bin, join(w.machine, 'zerograph'));
});

test('the server PATH is cut to the machine entries', () => {
  const w = world();
  const env = { [WIN ? 'Path' : 'PATH']: ['node_modules/.bin', w.local, w.machine].join(sep), HOME: '/h' };
  assert.deepStrictEqual(childEnv(env, w.checkout, WIN), { [WIN ? 'Path' : 'PATH']: w.machine, HOME: '/h' });
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
  assert.deepStrictEqual(argvOf('/usr/local/bin/zerograph', w.checkout).argv, ['/usr/local/bin/zerograph', '--repo', w.checkout, 'mcp']);
});

test('outside git the repository is the nearest directory above holding a store, else the project', () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), 'zerograph-mcp-test-')));
  const project = join(base, 'repo', 'packages', 'app');
  mkdirSync(project, { recursive: true });
  assert.strictEqual(checkoutOf(project), project);
  mkdirSync(join(base, 'repo', '.zerograph'));
  assert.strictEqual(checkoutOf(project), join(base, 'repo'));
});

test('an npm .cmd shim is started through cmd, an .exe directly', () => {
  assert.deepStrictEqual(argvOf('C:\\npm\\zerograph.cmd', 'C:\\work', 'C:\\Windows\\cmd.exe').argv, ['C:\\Windows\\cmd.exe', '/d', '/c', 'call', 'C:\\npm\\zerograph.cmd', '--repo', 'C:\\work', 'mcp']);
  assert.deepStrictEqual(argvOf('C:\\bin\\zerograph.exe', 'C:\\work').argv, ['C:\\bin\\zerograph.exe', '--repo', 'C:\\work', 'mcp']);
});

test('a .cmd shim is refused when cmd would act on a character in its path or the checkout', () => {
  for (const [bin, checkout] of [['C:\\npm\\zerograph.cmd', 'C:\\src\\R&D'], ['C:\\npm\\zerograph.cmd', 'C:\\x&calc'], ['C:\\100%\\zerograph.cmd', 'C:\\work'], ['C:\\npm\\zerograph.cmd', 'C:\\a^b|c<d>e"f!g']]) {
    const launch = argvOf(bin, checkout, 'cmd.exe');
    assert.strictEqual(launch.argv, undefined, checkout);
    assert.match(launch.error, /character cmd would act on/);
  }
  assert.ok(argvOf('C:\\bin\\zerograph.exe', 'C:\\src\\R&D').argv, 'an .exe takes no cmd, so nothing is refused');
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

test('ZEROGRAPH_BIN outranks the option, set or left unsubstituted, as it does in the hook', { skip: WIN && 'a POSIX launcher for the fake' }, () => {
  const w = world();
  const named = join(w.base, 'named');
  mkdirSync(named);
  const seen = join(w.base, 'seen.txt');
  writeFileSync(join(named, 'zerograph'), `#!/bin/sh\necho named > '${seen}'\n`);
  chmodSync(join(named, 'zerograph'), 0o755);
  for (const option of ['${user_config.binary}', join(w.machine, 'zerograph')]) {
    const r = spawnSync(process.execPath, [LAUNCHER, option], { cwd: w.base, input: '', env: { PATH: '/usr/bin:/bin', CLAUDE_PROJECT_DIR: w.checkout, ZEROGRAPH_BIN: join(named, 'zerograph') }, encoding: 'utf8' });
    assert.strictEqual(r.status, 0, r.stderr);
    assert.strictEqual(readFileSync(seen, 'utf8').trim(), 'named', option);
  }
});

test('a checkout that is the home directory, or above it, cuts nothing from PATH', () => {
  const w = world();
  const home = join(w.checkout, 'me');
  const nvm = join(home, '.nvm', 'bin');
  const path = [nvm, w.machine].join(sep);
  assert.deepStrictEqual(machineDirs(path, w.checkout, WIN, home), [nvm, w.machine], 'an ancestor of home');
  assert.deepStrictEqual(machineDirs(path, home, WIN, home), [nvm, w.machine], 'home itself');
  assert.deepStrictEqual(machineDirs(path, WIN ? 'C:\\' : '/', WIN, home), [nvm, w.machine], 'a filesystem root');
  assert.strictEqual(childEnv({ PATH: path }, home, WIN, {}, home).PATH, path);
  assert.deepStrictEqual(machineDirs([join(w.checkout, 'bin'), w.machine].join(sep), w.checkout, WIN, w.machine), [w.machine], 'a checkout beside home is still cut');
});

test('PATH is the only PATH on POSIX, and any case of it on Windows', () => {
  const w = world();
  assert.strictEqual(pathKey({ Path: 'x', PATH: 'y' }, false), 'PATH');
  assert.strictEqual(pathKey({ Path: 'x' }, false), 'PATH');
  assert.strictEqual(pathKey({ Path: 'x' }, true), 'Path');
  const env = childEnv({ Path: join(w.checkout, 'bin'), PATH: w.machine }, w.checkout, false);
  assert.strictEqual(env.PATH, w.machine);
  assert.strictEqual(env.Path, join(w.checkout, 'bin'), 'another variable, left as it is');
});

test('a quoted Windows PATH entry is judged without its quotes and kept as written', () => {
  const quoted = '"C:\\Program Files\\nodejs"';
  assert.deepStrictEqual(machineDirs([quoted, '"C:\\src\\app\\bin"'].join(';'), 'C:\\src\\app', true, 'C:\\Users\\me'), [quoted]);
  assert.strictEqual(childEnv({ Path: quoted }, 'C:\\src\\app', true, {}, 'C:\\Users\\me').Path, quoted);
});

test('a session opened through a link finds the checkout by its real path, as the CLI does', { skip: WIN && 'symlinks need a privilege' }, () => {
  const w = world();
  const link = join(w.base, 'link');
  symlinkSync(join(w.checkout, 'packages', 'app'), link);
  assert.strictEqual(checkoutOf(link), w.checkout);
});
