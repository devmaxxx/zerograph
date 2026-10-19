// The hook, exercised the way a harness runs it: a payload on stdin, a fake `zerograph` the hook
// resolves the way it resolves the real one, and a temporary root that has or lacks a store.
// Run: `node --test agent/claude-code/scripts/hook.test.mjs`, on every platform the hook is installed on.
import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, chmodSync, copyFileSync, symlinkSync, statSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, delimiter } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

import { queryWords, searchPattern, binary, onPath, bounded, rule, RULE } from './hook.mjs';

const HOOK = join(dirname(fileURLToPath(import.meta.url)), 'hook.mjs');

const WIN = process.platform === 'win32';

/**
 * A root with a fake `zerograph`; `answer` is what that fake prints on stdout.
 *
 * The fake is a Node script behind a launcher rather than a shell script, because Windows cannot
 * execute an extensionless `#!/bin/sh` file at all — the five tests that need the binary to answer
 * failed there for that reason alone. Node also keeps the answers' Cyrillic out of a batch file's
 * code page, and writes the argv line itself, so the log reads the same on both platforms whatever
 * quoting the launcher needed.
 */
function world({ store = true, answer = '', status = 0, version = '0.7.0' } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'zerograph-hook-test-'));
  if (store) {
    mkdirSync(join(root, '.zerograph'), { recursive: true });
    writeFileSync(join(root, '.zerograph', 'manifest.json'), '{}');
  }
  // Outside the root: a binary inside the checkout is one the hook refuses to run.
  const bin = mkdtempSync(join(tmpdir(), 'zerograph-hook-bin-'));
  mkdirSync(bin, { recursive: true });
  const argvLog = join(root, 'argv.txt');
  const budgetLog = join(root, 'budget.txt');
  const cwdLog = join(root, 'cwd.txt');
  const pathLog = join(root, 'path.txt');
  const fake = join(bin, 'fake.mjs');
  writeFileSync(fake, [
    "import { appendFileSync, writeSync } from 'node:fs';",
    `appendFileSync(${JSON.stringify(argvLog)}, process.argv.slice(2).join(' ') + '\\n', 'utf8');`,
    `appendFileSync(${JSON.stringify(budgetLog)}, process.argv[2] + ' ' + (process.env.ZEROGRAPH_READER_BUDGET ?? '-') + '\\n', 'utf8');`,
    `appendFileSync(${JSON.stringify(cwdLog)}, process.cwd() + '\\n', 'utf8');`,
    `appendFileSync(${JSON.stringify(pathLog)}, (process.env.PATH ?? '') + '\\n', 'utf8');`,
    `if (process.argv.includes('--version')) { writeSync(1, ${JSON.stringify(`zerograph ${version}\n`)}); process.exit(0); }`,
    `writeSync(1, ${JSON.stringify(answer + '\n')});`,
    `process.exit(${status});`,
  ].join('\n'), 'utf8');

  // The launcher is what the hook actually spawns: a `.cmd` on Windows, which is why the hook has
  // to know how to spawn one, and a shell script everywhere else, which is what `PATH` finds.
  const launcher = join(bin, WIN ? 'zerograph.cmd' : 'zerograph');
  writeFileSync(launcher, WIN
    ? `@echo off\r\n"${process.execPath}" "${fake}" %*\r\n`
    : `#!/bin/sh\nexec "${process.execPath}" "${fake}" "$@"\n`);
  if (!WIN) chmodSync(launcher, 0o755);
  return { root, bin, launcher, argvLog, budgetLog, cwdLog, pathLog };
}

/**
 * The environment less what says which harness runs the hook and where its project is: a suite run
 * from inside a Claude Code or Codex session would otherwise read that session's settings.
 */
function harnessless(env) {
  const { CLAUDE_PROJECT_DIR, CLAUDE_PLUGIN_ROOT, PLUGIN_ROOT, CODEX_HOME, ...rest } = env;
  return rest;
}

function fire(w, payload, env = {}, hook = HOOK, args = []) {
  const r = spawnSync(process.execPath, [hook, ...args], {
    input: JSON.stringify({ cwd: w.root, session_id: 'sess', ...payload }),
    encoding: 'utf8',
    env: withoutNulls({
      ...harnessless(process.env),
      PATH: `${w.bin}${delimiter}${process.env.PATH}`,
      // Windows resolves a bare name through `PATHEXT` only inside a shell, so `PATH` alone would
      // never find `zerograph.cmd`; the variable the hook already honours names it instead.
      ...(WIN ? { ZEROGRAPH_BIN: w.launcher } : {}),
      // `tmpdir()` reads TMPDIR on POSIX and TEMP/TMP on Windows. All three, or the per-session
      // state the once-and-cadence gates keep would be shared between tests on one platform.
      TMPDIR: join(w.root, 'tmp'), TEMP: join(w.root, 'tmp'), TMP: join(w.root, 'tmp'),
      // A machine's own update cache would add a message to every SessionStart here.
      ZEROGRAPH_NO_UPDATE_CHECK: '1',
      ...env,
    }),
  });
  assert.equal(r.status, 0, `the hook always exits 0; stderr: ${r.stderr}`);
  return r.stdout ? JSON.parse(r.stdout) : null;
}

/** `env` without the keys a test set to null, which is how a test unsets a default of `fire`. */
function withoutNulls(env) { return Object.fromEntries(Object.entries(env).filter(([, v]) => v !== null)); }

function context(out) { return out?.hookSpecificOutput?.additionalContext ?? null; }

function argv(w) { return existsSync(w.argvLog) ? readFileSync(w.argvLog, 'utf8').trim().split('\n') : []; }

/** The argv line matching `re`, waited for: a detached child writes after the hook has exited. */
async function waitFor(w, re, ms = 3000) {
  for (let waited = 0; waited < ms; waited += 50) {
    const hit = argv(w).find((l) => re.test(l));
    if (hit) return hit;
    await new Promise((r) => setTimeout(r, 50));
  }
  return null;
}

/**
 * What `changes --depth 1 --json` prints for a diff: each part touches `symbol` in `file` and has
 * `callers` direct callers spread over `files` files.
 */
function diff(parts, risk) {
  const touched = [];
  const affected = [];
  for (const { file, symbol, callers, files = callers } of parts) {
    touched.push({ id: `sym:${file}::${symbol}`, at: `${file}:3-40`, indexed: true });
    for (let i = 0; i < callers; i += 1) {
      affected.push({ id: `sym:src/${symbol}/c${i % files}.ts::caller${i}`, at: `src/${symbol}/c${i % files}.ts:${i + 1}`,
        depth: 1, kind: 'Calls', via: `sym:${file}::${symbol}` });
    }
  }
  return JSON.stringify({ touched, affected, files: [], risk });
}

function edit(w, ...segments) {
  return { hook_event_name: 'PostToolUse', tool_name: 'Edit', tool_input: { file_path: join(w.root, ...segments) } };
}

test('a search pattern is read out of Bash, Grep and Glob, and nowhere else', () => {
  assert.equal(searchPattern('Bash', { command: 'rg -n "cancellation policy" apps/' }), 'cancellation policy',
    'a quoted phrase is one argument: splitting on whitespace would ask about its first word only');
  assert.equal(searchPattern('Bash', { command: 'grep -R withTenant .' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: 'ls -la' }), null);
  assert.equal(searchPattern('Grep', { pattern: 'tenant isolation' }), 'tenant isolation');
  assert.equal(searchPattern('Glob', { pattern: '**/tenant-*.ts' }), 'tenant');
  assert.equal(searchPattern('Glob', { pattern: '**/*.ts' }), null);
  assert.equal(searchPattern('Read', { file_path: 'a.ts' }), null, 'reading a named file is not a search');
});

test('`-e` and `--regexp` carry the pattern, not a value to skip', () => {
  assert.equal(searchPattern('Bash', { command: 'rg -n -e "cancellation policy" packages' }), 'cancellation policy',
    'skipped as a flag value, the graph was asked about the path after it');
  assert.equal(searchPattern('Bash', { command: 'grep -R --regexp withTenant .' }), 'withTenant');
});

test('a flag whose value is the next word is skipped, per tool, so the value is never the pattern', () => {
  // The issue's table (#34): each of these asked the graph about the flag's value, or about nothing.
  for (const [command, pattern] of [
    ['grep -rn --exclude-dir node_modules withTenant .', 'withTenant'],
    ['grep -rn --exclude-dir=node_modules withTenant .', 'withTenant'],
    ['grep -rn --include *.ts withTenant .', 'withTenant'],
    ['grep -rn -d recurse withTenant .', 'withTenant'],
    ['rg -M 200 withTenant packages', 'withTenant'],
    ['rg -j 4 withTenant packages', 'withTenant'],
    ['rg --max-depth 3 withTenant packages', 'withTenant'],
    ['rg --iglob *.ts withTenant packages', 'withTenant'],
    ['rg --context 3 withTenant packages', 'withTenant'],
    ['rg --color never withTenant packages', 'withTenant'],
    ['rg -E utf-8 withTenant packages', 'withTenant'],
    ['rg -r replacement withTenant packages', 'withTenant'],
  ]) assert.equal(searchPattern('Bash', { command }), pattern, command);
  // The same letter means different things to the two tools: `-r` and `-E` take no value under
  // grep, `-t` and `-d` mean different things under each. A list shared between them skips the
  // pattern itself.
  assert.equal(searchPattern('Bash', { command: 'grep -r withTenant packages' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: 'grep -E withTenant packages' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: 'rg -t ts withTenant packages' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: 'grep -T withTenant packages' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: '/usr/bin/grep -A 2 withTenant packages' }), 'withTenant',
    'the tool is named by its last path segment');
});

test('a stage a pipe feeds reads stdin, not the repository, and is not asked about', () => {
  for (const command of [
    'gh auth status 2>&1 | grep -E "Logged in|Active account"',
    'rg --files | rg tenancy',
    'git log --oneline |& grep cancellation',
    "ls -a packages | grep -i 'lintstaged\\|lint-staged'",
    'git log --oneline |\n  grep cancellation',
    'git log --oneline \\\n  | grep cancellation',
    'rg --files \\\n  | rg tenancy',
  ]) assert.equal(searchPattern('Bash', { command }), null, command);
  assert.equal(searchPattern('Bash', { command: 'rg -n "refund|cancellation" apps/ | head -5' }), 'refund|cancellation',
    'a `|` inside quotes is the pattern, and its stage reads the repository');
  assert.equal(searchPattern('Bash', { command: 'grep -R refund\\|cancellation apps' }), 'refund\\|cancellation',
    'an escaped `|` is not a pipe either');
  assert.equal(searchPattern('Bash', { command: 'cd apps && rg -n withTenant src' }), 'withTenant');
  assert.equal(searchPattern('Bash', { command: 'rg -n \\\n  cancellation packages' }), 'cancellation',
    'a line continuation reads as a space, so its backslash is not the pattern');
  assert.equal(searchPattern('Bash', { command: 'git diff --quiet || grep -R withTenant packages' }), 'withTenant',
    '`||` runs a second command, it does not feed one');

  const w = world({ answer: 'sym:a.ts::accountRow  a.ts:1  accountRow' });
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'gh auth status 2>&1 | grep -E "Logged in|Active account"' } };
  assert.equal(fire(w, payload), null);
  assert.deepEqual(argv(w), [], 'the graph was not asked');
});

test('a question is words, a path is not, and an identifier goes whole', () => {
  assert.deepEqual(queryWords('cancellation policy'), ['cancellation', 'policy']);
  assert.deepEqual(queryWords('отмена записи'), ['отмена', 'записи']);
  assert.equal(queryWords('src/app/'), null);
  assert.equal(queryWords('foo'), null, 'three letters is a flag more often than a concept');
  assert.deepEqual(queryWords('withTenant'), ['withTenant']);
  assert.deepEqual(queryWords('FR-PAY-22'), ['FR-PAY-22']);
  assert.deepEqual(queryWords('a.b*'), null);
  assert.equal(queryWords('cancellation refunds deposits invoices приложение расписание календарь').length, 6,
    'six words at most; the seventh never changed an answer and every one costs prompt');
});

test('a milestone or task id goes whole, and neither an escape nor a leading dash reaches ask', () => {
  assert.deepEqual(queryWords('BE-M17'), ['BE-M17'], 'no four-letter word to fall back on: without the id nothing is asked');
  assert.deepEqual(queryWords('BE-M01-T03'), ['BE-M01-T03']);
  assert.deepEqual(queryWords('\\bwithTenant\\b'), ['withTenant'], 'dropping only the backslash asks about bwithTenant');
  assert.deepEqual(queryWords('\\bFR-CAL-40\\b'), ['FR-CAL-40']);
  assert.deepEqual(queryWords('--rerank misses'), ['rerank', 'misses']);

  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  отмена' });
  fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg -n -- "--rerank misses" docs' } });
  assert.match(argv(w)[0], /ask --stale --seeds 3 rerank misses$/);
  assert.ok(!argv(w)[0].includes('--rerank'), `ask would read it as its paid flag: ${argv(w)[0]}`);
});

test('the hook never takes a binary from the checkout it runs in, only ZEROGRAPH_BIN and PATH', () => {
  const w = world();
  // Everything a cloned repository could ship where a local install or a build would put one.
  const shipped = [['node_modules', '.bin'], ['target', 'release'], ['target', 'debug']];
  for (const dir of shipped) {
    mkdirSync(join(w.root, ...dir), { recursive: true });
    for (const name of ['zerograph', 'zerograph.cmd', 'zerograph.exe']) writeFileSync(join(w.root, ...dir, name), '#!/bin/sh\n');
  }
  const named = process.env.ZEROGRAPH_BIN;
  const path = process.env.PATH;
  delete process.env.ZEROGRAPH_BIN;
  for (const dir of shipped) for (const name of ['zerograph']) chmodSync(join(w.root, ...dir, name), 0o755);
  const machine = mkdtempSync(join(tmpdir(), 'zerograph-machine-bin-'));
  mkdirSync(machine, { recursive: true });
  const installed = join(machine, WIN ? 'zerograph.exe' : 'zerograph');
  writeFileSync(installed, '');
  chmodSync(installed, 0o755);
  // A relative entry resolves against the checkout, so it is the checkout's file too, however
  // early PATH names it; and a bare name handed to the spawn would let it, and on Windows the
  // working directory, win.
  const relative = ['node_modules/.bin', '.', join('target', 'release')];
  process.env.PATH = [...relative, join(w.root, 'empty-path')].join(delimiter);
  try {
    assert.equal(binary(w.root), null, 'nothing on PATH but the checkout\'s own: nothing to run');
    process.env.PATH = [...relative, machine].join(delimiter);
    assert.equal(binary(w.root), installed, 'the machine\'s binary, by its absolute path');
    process.env.PATH = [join(w.root, 'node_modules', '.bin'), machine].join(delimiter);
    assert.equal(binary(w.root), installed, 'an absolute entry inside the checkout, as an .envrc adds, is the checkout\'s too');
    const alias = join(mkdtempSync(join(tmpdir(), 'zerograph-alias-')), 'checkout');
    symlinkSync(w.root, alias, 'junction');
    process.env.PATH = [join(alias, 'node_modules', '.bin'), machine].join(delimiter);
    assert.equal(binary(w.root), installed, 'the checkout reached through a symlinked alias is still the checkout');
    process.env.PATH = [join(w.root, 'node_modules', '.bin'), machine].join(delimiter);
    assert.equal(binary(alias), installed, 'and so is a session opened through the alias');
    process.env.ZEROGRAPH_BIN = join('node_modules', '.bin', 'zerograph');
    assert.equal(binary(w.root), null, 'a relative ZEROGRAPH_BIN names the checkout\'s file, so it names nothing');
    process.env.ZEROGRAPH_BIN = '/opt/zerograph';
    assert.equal(binary(w.root), '/opt/zerograph', 'an absolute ZEROGRAPH_BIN is taken as it is');
  } finally {
    if (named === undefined) delete process.env.ZEROGRAPH_BIN; else process.env.ZEROGRAPH_BIN = named;
    process.env.PATH = path;
  }
});

test('the binary is started outside the checkout, so nothing a shim looks up resolves to the repository\'s files', async () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  cancellation' });
  fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation window' } });
  fire(w, { hook_event_name: 'SessionStart', source: 'startup' });
  await waitFor(w, / serve /);
  const cwds = readFileSync(w.cwdLog, 'utf8').trim().split('\n');
  assert.ok(cwds.length >= 2, cwds.join(' | '));
  assert.ok(cwds.every((d) => realpathSync(d) === realpathSync(w.bin)), cwds.join(' | '));
});

test('a cloned repository that ships an executable node_modules/.bin/zerograph never gets it started', { skip: WIN }, async () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  writeFileSync(join(w.root, 'zerograph.toml'), '');
  const shim = join(w.root, 'node_modules', '.bin');
  mkdirSync(shim, { recursive: true });
  copyFileSync(w.launcher, join(shim, 'zerograph'));
  chmodSync(join(shim, 'zerograph'), 0o755);
  const env = { PATH: join(w.root, 'nothing-here') };
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, env).systemMessage, MISSING_MESSAGE, 'no binary of its own: no build claimed');
  fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation window' } }, env);
  await new Promise((r) => setTimeout(r, 300));
  assert.deepEqual(argv(w), [], 'the shipped shim ran');
});

test('the hook keeps its session state in a directory private to the user', { skip: WIN }, () => {
  const w = world();
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation window' } };
  fire(w, payload);
  const root = join(w.root, 'tmp', `zerograph-hook-${process.getuid()}`);
  assert.equal(statSync(root).mode & 0o777, 0o700);
  assert.ok(!existsSync(join(w.root, 'tmp', 'zerograph-hook')), 'the predictable shared name is gone');
});

test('SessionStart prints the brief, and nothing where there is no store', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes\nmodel=onnx-community/embeddinggemma-300m-ONNX' });
  const c = context(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }));
  assert.ok(c.startsWith('zerograph:'), c);
  assert.ok(Buffer.byteLength(c) <= 700, `${Buffer.byteLength(c)} B`);
  assert.ok(argv(w).some((l) => /--no-dense prime/.test(l)), argv(w).join(' | '));

  const bare = world({ store: false });
  assert.equal(fire(bare, { hook_event_name: 'SessionStart', source: 'startup' }), null);
});

test('a .zerograph without a manifest is not built, and the binary is not asked', () => {
  // A build that died before its manifest, or a directory another tool made, leaves the name with
  // nothing in it the binary could answer from.
  const w = world({ store: false, answer: 'zerograph: 3 doc nodes' });
  mkdirSync(join(w.root, '.zerograph'), { recursive: true });
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }), null);
  assert.deepEqual(argv(w), []);
});

test('a session started in a subdirectory finds the index above it, and a nested checkout does not borrow it', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  const env = { ZEROGRAPH_HOOK_SERVE: '0' };
  const sub = join(w.root, 'packages', 'db');
  mkdirSync(sub, { recursive: true });
  const c = context(fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: sub }, env));
  assert.ok(c?.startsWith('zerograph:'), `a cd into a subdirectory lost the index: ${c}`);
  assert.ok(argv(w).some((l) => l.startsWith(`--repo ${w.root} --no-dense prime`)), argv(w).join(' | '));

  const nested = join(w.root, 'worktree');
  mkdirSync(join(nested, 'src'), { recursive: true });
  writeFileSync(join(nested, '.git'), 'gitdir: ../.git/worktrees/worktree\n');
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: join(nested, 'src') }, env), null,
    'a checkout of its own ends the walk: its parent\'s index is not its index');
});

test('in a checkout the store is its top level\'s, as the CLI picks it: a package\'s own store is not read', () => {
  const w = world({ store: false, answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  const env = { ZEROGRAPH_HOOK_SERVE: '0' };
  mkdirSync(join(w.root, '.git'));
  const pkg = join(w.root, 'packages', 'x');
  mkdirSync(join(pkg, '.zerograph'), { recursive: true });
  writeFileSync(join(pkg, '.zerograph', 'manifest.json'), '{}');
  fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: pkg }, env);
  assert.deepEqual(argv(w).filter((l) => l.includes(pkg)), [], 'the hook answered from a store `zerograph status` does not see');

  mkdirSync(join(w.root, '.zerograph'));
  writeFileSync(join(w.root, '.zerograph', 'manifest.json'), '{}');
  fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: pkg, session_id: 'second' }, env);
  assert.ok(argv(w).some((l) => l.startsWith(`--repo ${w.root} --no-dense prime`)), argv(w).join(' | '));
});

test('a PATH entry anywhere in the checkout is refused, even from a package with a store of its own', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  const pkg = join(w.root, 'packages', 'x');
  mkdirSync(join(pkg, '.zerograph'), { recursive: true });
  writeFileSync(join(pkg, '.zerograph', 'manifest.json'), '{}');
  const shim = join(w.root, 'node_modules', '.bin');
  mkdirSync(shim, { recursive: true });
  const name = WIN ? 'zerograph.exe' : 'zerograph';
  writeFileSync(join(shim, name), '');
  chmodSync(join(shim, name), 0o755);
  const saved = { bin: process.env.ZEROGRAPH_BIN, path: process.env.PATH };
  delete process.env.ZEROGRAPH_BIN;
  process.env.PATH = shim;
  try {
    assert.equal(binary(pkg), null, 'the checkout\'s node_modules/.bin was taken for the machine\'s');
  } finally {
    for (const [k, v] of [['ZEROGRAPH_BIN', saved.bin], ['PATH', saved.path]]) {
      if (v === undefined) delete process.env[k]; else process.env[k] = v;
    }
  }
});

test('the binary inherits no PATH entry inside the checkout, so the `node` an npm launcher asks for is never the checkout\'s', async () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  cancellation' });
  mkdirSync(join(w.root, '.git'));
  const planted = join(w.root, 'bin');
  const env = { PATH: [w.bin, planted, dirname(process.execPath)].join(delimiter), ZEROGRAPH_HOOK_SERVE: '0' };
  fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation window' } }, env);
  fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, env);
  await waitFor(w, / update /);
  const paths = readFileSync(w.pathLog, 'utf8').trim().split('\n');
  assert.ok(paths.length >= 2, paths.join(' | '));
  assert.ok(paths.every((p) => !p.split(delimiter).includes(planted)), paths.join(' | '));
  assert.ok(paths.every((p) => p.split(delimiter).includes(w.bin)), 'the machine\'s entries stay');
});

test('on Windows a zerograph.exe anywhere on PATH wins over an earlier npm .cmd shim', () => {
  const npm = mkdtempSync(join(tmpdir(), 'zerograph-npm-'));
  const cargo = mkdtempSync(join(tmpdir(), 'zerograph-cargo-'));
  writeFileSync(join(npm, 'zerograph.cmd'), '');
  writeFileSync(join(cargo, 'zerograph.exe'), '');
  const path = process.env.PATH;
  process.env.PATH = [npm, cargo].join(';');
  try {
    assert.equal(onPath(null, 'win32'), join(cargo, 'zerograph.exe'));
  } finally {
    process.env.PATH = path;
  }
});

test('a batch-file call that runs out of time has its whole tree killed, not just the shell', { skip: WIN }, () => {
  const dir = mkdtempSync(join(tmpdir(), 'zerograph-bounded-'));
  const killed = join(dir, 'killed.txt');
  const kill = [process.execPath, '-e', `require('fs').writeFileSync(${JSON.stringify(killed)}, process.argv[1]); process.kill(Number(process.argv[1]))`];
  const started = Date.now();
  const r = bounded('sleep', ['30'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }, kill, 300);
  assert.notEqual(r.status, 0);
  assert.ok(Date.now() - started < 5000, `waited ${Date.now() - started} ms`);
  assert.match(readFileSync(killed, 'utf8'), /^\d+$/, 'the tree killer was handed the shell\'s pid');
});

test('a hook run through a symlinked path still answers', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  const real = join(w.root, 'real');
  mkdirSync(real);
  copyFileSync(HOOK, join(real, 'hook.mjs'));
  copyFileSync(join(dirname(HOOK), 'launcher.mjs'), join(real, 'launcher.mjs'));
  const link = join(w.root, 'link');
  // A junction on Windows, which needs no symlink privilege; the type is ignored everywhere else.
  symlinkSync(real, link, 'junction');
  const c = context(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { ZEROGRAPH_HOOK_SERVE: '0' }, join(link, 'hook.mjs')));
  assert.ok(c?.startsWith('zerograph:'), `argv[1] keeps the link and import.meta.url resolves it: ${c}`);
});

test('SessionStart starts a resident serve, and the switch turns it off', async () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  fire(w, { hook_event_name: 'SessionStart', source: 'startup' });
  // Detached and never waited on, so the fake's line lands after the hook has already exited.
  const line = await waitFor(w, /serve --idle 1800 --idle-model 300/);
  assert.ok(line, `no serve was started: ${argv(w).join(' | ')}`);
  assert.ok(!line.includes('--no-dense'), `the resident answer is the fused one: ${line}`);

  const off = world({ answer: 'zerograph: 1 doc node' });
  fire(off, { hook_event_name: 'SessionStart', source: 'startup' }, { ZEROGRAPH_HOOK_SERVE: '0' });
  await new Promise((r) => setTimeout(r, 300));
  assert.equal(argv(off).filter((l) => l.includes('serve')).length, 0, argv(off).join(' | '));
});

test('a checkout that asked for an index and has none gets one built in the background from its first session', async () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  mkdirSync(join(w.root, '.claude', 'hooks'), { recursive: true });
  writeFileSync(join(w.root, '.claude', 'hooks', 'zerograph-hook.mjs'), '');
  const sub = join(w.root, 'src');
  mkdirSync(sub);
  const c = context(fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: sub }));
  assert.match(c, /being built in the background/);
  const line = await waitFor(w, /update --detach/);
  assert.equal(line, `--repo ${w.root} update --detach`, 'the checkout, not the subdirectory, and dense');
  assert.equal(argv(w).filter((l) => / prime$| serve /.test(l)).length, 0, argv(w).join(' | '));
});

test('a checkout that never asked for an index is not built, so a machine-wide hook stays silent', async () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }), null);
  await new Promise((r) => setTimeout(r, 300));
  assert.deepEqual(argv(w), []);
  for (const marker of [['zerograph.toml'], ['.codex', 'hooks', 'zerograph-hook.mjs']]) {
    const o = world({ store: false });
    mkdirSync(join(o.root, '.git'));
    mkdirSync(join(o.root, ...marker.slice(0, -1)), { recursive: true });
    writeFileSync(join(o.root, ...marker), '');
    assert.match(context(fire(o, { hook_event_name: 'SessionStart', source: 'startup' })), /being built/, marker.join('/'));
  }
});

test('a linked worktree whose main worktree holds the hook asked for an index too', async () => {
  const w = world({ store: false });
  const main = join(w.root, 'main');
  mkdirSync(join(main, '.git', 'worktrees', 'wt'), { recursive: true });
  writeFileSync(join(main, '.git', 'worktrees', 'wt', 'commondir'), '../..\n');
  mkdirSync(join(main, '.codex', 'hooks'), { recursive: true });
  writeFileSync(join(main, '.codex', 'hooks', 'zerograph-hook.mjs'), '');
  const wt = join(w.root, 'wt');
  mkdirSync(wt);
  writeFileSync(join(wt, '.git'), `gitdir: ${join(main, '.git', 'worktrees', 'wt')}\n`);
  assert.match(context(fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: wt })), /being built/);
  assert.equal(await waitFor(w, /update --detach/), `--repo ${wt} update --detach`);
});

test('a linked worktree of a main worktree with a built index asked for one too', async () => {
  const w = world({ store: false });
  const main = join(w.root, 'main');
  mkdirSync(join(main, '.git', 'worktrees', 'wt'), { recursive: true });
  writeFileSync(join(main, '.git', 'worktrees', 'wt', 'commondir'), '../..\n');
  mkdirSync(join(main, '.zerograph'), { recursive: true });
  writeFileSync(join(main, '.zerograph', 'manifest.json'), '{}');
  const wt = join(w.root, 'wt');
  mkdirSync(wt);
  writeFileSync(join(wt, '.git'), `gitdir: ${join(main, '.git', 'worktrees', 'wt')}\n`);
  assert.match(context(fire(w, { hook_event_name: 'SessionStart', source: 'startup', cwd: wt })), /being built/);
  assert.equal(await waitFor(w, /update --detach/), `--repo ${wt} update --detach`);
});

test('the plugin\'s binary option names the binary when ZEROGRAPH_BIN does not', { skip: WIN }, () => {
  const w = world({ answer: 'zerograph: 3 doc nodes' });
  const c = context(fire(w, { hook_event_name: 'SessionStart', source: 'startup' },
    { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '', CLAUDE_PLUGIN_OPTION_BINARY: w.launcher, ZEROGRAPH_HOOK_SERVE: '0' }));
  assert.equal(c, 'zerograph: 3 doc nodes');
});

const MISSING_MESSAGE = '▲ zerograph is not installed — this repository uses it\n  npm i -g zerograph · pnpm add -g zerograph';

test('an opted-in checkout with no binary says so to the person, claims no build, and tells the model the tools are out', () => {
  for (const store of [false, true]) {
    const w = world({ store });
    mkdirSync(join(w.root, '.git'));
    writeFileSync(join(w.root, 'zerograph.toml'), '');
    const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '' });
    assert.equal(out.systemMessage, MISSING_MESSAGE, `store ${store}`);
    const c = context(out);
    assert.equal(c.split('\n').length, 1, c);
    assert.match(c, /not installed/);
    assert.doesNotMatch(c, /being built/, 'nothing was started');
  }
});

test('a checkout opted in by nothing but its own index hears that the binary is missing', () => {
  const w = world();
  mkdirSync(join(w.root, '.git'));
  const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '' });
  assert.equal(out?.systemMessage, MISSING_MESSAGE);
});

test('the missing binary is only the context line under Codex', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  writeFileSync(join(w.root, 'zerograph.toml'), '');
  const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '' }, HOOK, ['--codex']);
  assert.equal(out.systemMessage, undefined);
  assert.match(context(out), /not installed/);
});

test('a repository that never asked for zerograph hears nothing about a missing binary', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '' }), null);
});

test('while the first session builds, a search is told the build is under way, not to start one', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  writeFileSync(join(w.root, 'zerograph.toml'), '');
  const c = context(fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg "cancellation policy"' } }));
  assert.match(c, /being built in the background/);
  assert.doesNotMatch(c, /zerograph build`/);
});

test('with serve off, a built index is caught up by a detached update instead', async () => {
  const w = world({ answer: 'zerograph: 3 doc nodes' });
  fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { ZEROGRAPH_HOOK_SERVE: '0' });
  assert.ok(await waitFor(w, /^--repo .* update --detach$/), argv(w).join(' | '));
  const on = world({ answer: 'zerograph: 3 doc nodes' });
  fire(on, { hook_event_name: 'SessionStart', source: 'startup' });
  await waitFor(on, /serve --idle/);
  await new Promise((r) => setTimeout(r, 200));
  assert.equal(argv(on).filter((l) => l.includes('update')).length, 0, 'serve catches up as it starts');
});

test('every call the hook waits on refreshes in process for less than its timeout, or less when told', () => {
  const budgets = (w) => readFileSync(w.budgetLog, 'utf8').trim().split('\n').filter((l) => l.startsWith('--repo')).map((l) => l.split(' ')[1]);
  const env = { ZEROGRAPH_HOOK_SERVE: '0' };
  const edit = { hook_event_name: 'PostToolUse', tool_name: 'Edit', tool_input: { file_path: 'src/a.ts' } };
  for (const [set, want] of [[undefined, '2'], ['30', '2'], ['1', '1'], ['0', '0'], ['soon', '2']]) {
    const w = world({ answer: '{}' });
    fire(w, edit, { ...env, ZEROGRAPH_READER_BUDGET: set });
    assert.deepEqual(budgets(w), [want], `set ${set}`);
  }
  for (const [line, want] of [['reader_budget = 0', '0'], ['reader_budget = 5 # generous', '2']]) {
    const w = world({ answer: '{}' });
    writeFileSync(join(w.root, 'zerograph.toml'), `${line}\n`);
    fire(w, edit, { ...env, ZEROGRAPH_READER_BUDGET: undefined });
    assert.deepEqual(budgets(w), [want], line);
  }
});

/** A checkout carrying what a 0.6.x `install-agent` committed: the script and its entries for `events`. */
function legacyClaude(w, events, matchers = {}) {
  mkdirSync(join(w.root, '.claude', 'hooks'), { recursive: true });
  writeFileSync(join(w.root, '.claude', 'hooks', 'zerograph-hook.mjs'), '// 0.6.x\n');
  const entry = { hooks: [{ type: 'command', command: 'node "$CLAUDE_PROJECT_DIR/.claude/hooks/zerograph-hook.mjs"', timeout: 10 }] };
  const hooks = Object.fromEntries(events.map((e) => [e, [{ matcher: matchers[e] ?? '.*', ...entry }]]));
  writeFileSync(join(w.root, '.claude', 'settings.json'), JSON.stringify({ hooks }));
}

test('a committed 0.6.x hook answers alone: the plugin is silent on the events it registers', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  legacyClaude(w, ['SessionStart', 'PreToolUse', 'PostToolUse']);
  const env = { CLAUDE_PROJECT_DIR: w.root, ZEROGRAPH_HOOK_SERVE: '0' };
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, env), null);
  assert.equal(fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg -n cancellation' } }, env), null);
  assert.deepEqual(argv(w), [], 'a silent hook asks the binary nothing either');
  assert.equal(context(fire(w, { hook_event_name: 'SubagentStart', agent_id: 'a1' }, env)), RULE,
    'an event the old entries never registered is still the plugin\'s');
});

test('entries whose script is gone leave the plugin answering', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  legacyClaude(w, ['SessionStart']);
  const env = { CLAUDE_PROJECT_DIR: w.root, ZEROGRAPH_HOOK_SERVE: '0' };
  rmSync(join(w.root, '.claude', 'hooks', 'zerograph-hook.mjs'));
  assert.ok(context(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, env))?.startsWith('zerograph:'));
});

test('a legacy entry silences the plugin only where its matcher matches', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  legacyClaude(w, ['SessionStart', 'PreToolUse'], { SessionStart: 'startup', PreToolUse: 'Grep|Glob|Agent' });
  const env = { CLAUDE_PROJECT_DIR: w.root, ZEROGRAPH_HOOK_SERVE: '0' };
  assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, env), null);
  assert.ok(context(fire(w, { hook_event_name: 'SessionStart', source: 'resume', session_id: 's2' }, env))?.startsWith('zerograph:'),
    'a resume the old entry does not match is the plugin\'s');
  assert.equal(fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation' } }, env), null);
  const w2 = world({ answer: 'FR-PAY-22  docs/a.md:1  cancellation' });
  legacyClaude(w2, ['PreToolUse'], { PreToolUse: 'Grep|Glob|Agent' });
  fire(w2, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg -n cancellation' } }, { CLAUDE_PROJECT_DIR: w2.root, ZEROGRAPH_HOOK_SERVE: '0' });
  assert.ok(argv(w2).some((a) => a.includes('ask')), 'Bash is not in Grep|Glob|Agent, so the plugin asks');
});

test('only the project Claude Code runs can stand the plugin down: not a main worktree, not a clone the session cd\'d into', () => {
  const grep = { hook_event_name: 'PreToolUse', tool_name: 'Grep', tool_input: { pattern: 'cancellation window' } };
  // The old entries run `$CLAUDE_PROJECT_DIR/.claude/hooks/zerograph-hook.mjs`, so a worktree whose
  // branch retired the script runs nothing of its main's.
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  cancellation' });
  const main = mkdtempSync(join(tmpdir(), 'zerograph-main-'));
  const linked = join(main, '.git', 'worktrees', 'wt');
  mkdirSync(linked, { recursive: true });
  writeFileSync(join(linked, 'commondir'), '../..\n');
  writeFileSync(join(w.root, '.git'), `gitdir: ${linked}\n`);
  legacyClaude({ root: main }, ['PreToolUse']);
  fire(w, grep, { CLAUDE_PROJECT_DIR: w.root });
  assert.ok(argv(w).some((a) => a.includes('ask')), 'the plugin answers the worktree');

  const a = world({ store: false });
  mkdirSync(join(a.root, '.git'));
  const b = join(a.root, 'vendor', 'b');
  mkdirSync(join(b, '.git'), { recursive: true });
  legacyClaude({ root: b }, ['PreToolUse']);
  assert.notEqual(fire(a, { ...grep, cwd: b }, { CLAUDE_PROJECT_DIR: a.root }), null, 'B\'s settings are not the session\'s');

  const own = world({ answer: 'FR-PAY-22  docs/a.md:1  cancellation' });
  mkdirSync(join(own.root, '.git'));
  mkdirSync(join(own.root, 'apps', 'web'), { recursive: true });
  legacyClaude(own, ['PreToolUse']);
  assert.equal(fire(own, { ...grep, cwd: join(own.root, 'apps', 'web') }, { CLAUDE_PROJECT_DIR: own.root }), null, 'a cd below the project changes nothing');
});

test('standing down at SessionStart keeps the notices the 0.6.x hook never shows, and nothing else', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  legacyClaude(w, ['SessionStart']);
  const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { ...updateHome(w, CACHED), CLAUDE_PROJECT_DIR: w.root });
  assert.match(out.systemMessage, /^▲ zerograph 0\.7\.1 available/);
  assert.match(context(out), /^zerograph: 0\.7\.1 is out/, 'no brief: the old hook gave it');
  assert.deepEqual(argv(w).map((l) => l.split(' ').pop()), ['--version'], 'no serve, no update, no prime');

  const bare = world({ store: false });
  mkdirSync(join(bare.root, '.git'));
  legacyClaude(bare, ['SessionStart']);
  const missing = fire(bare, { hook_event_name: 'SessionStart', source: 'startup' },
    { CLAUDE_PROJECT_DIR: bare.root, PATH: join(bare.root, 'nothing-here'), ZEROGRAPH_BIN: '' });
  assert.equal(missing.systemMessage, MISSING_MESSAGE);
});

/** A machine `hooks.json` as 0.6.x printed it for Codex, running `script` as spelled. */
function legacyCodex(home, spelled, matcher = 'startup|resume|clear|compact') {
  writeFileSync(join(home, 'hooks.json'), JSON.stringify({ hooks: { SessionStart: [{ matcher, hooks: [{ type: 'command', command: `node '${spelled}'` }] }] } }));
}

test('under Codex the machine hooks file decides, and a repository\'s .claude settings do not', () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  legacyClaude(w, ['SessionStart']);
  const home = mkdtempSync(join(tmpdir(), 'zerograph-codex-home-'));
  // What Codex really sets: both roots, so only the plugin's `--codex` argument names the harness.
  const env = { PLUGIN_ROOT: join(home, 'plugin'), CLAUDE_PLUGIN_ROOT: join(home, 'plugin'), CODEX_HOME: home, ZEROGRAPH_HOOK_SERVE: '0' };
  const codex = (payload) => fire(w, payload, env, HOOK, ['--codex']);
  assert.ok(context(codex({ hook_event_name: 'SessionStart', source: 'startup' }))?.startsWith('zerograph:'),
    'Codex never runs .claude/settings.json');

  const script = join(w.root, '.codex', 'hooks', 'zerograph-hook.mjs');
  mkdirSync(dirname(script), { recursive: true });
  writeFileSync(script, '// 0.6.x\n');
  legacyCodex(home, script);
  assert.equal(codex({ hook_event_name: 'SessionStart', source: 'startup', session_id: 's2' }), null);
  assert.equal(context(codex({ hook_event_name: 'SubagentStart', agent_id: 'a1' })), RULE);
});

test('under Codex a legacy entry spelled from the home directory is found', { skip: WIN && 'HOME is not the home directory there' }, () => {
  const w = world({ answer: 'zerograph: 3076 doc nodes, 5240 code nodes' });
  const home = realpathSync(mkdtempSync(join(tmpdir(), 'zerograph-home-')));
  const codexHome = join(home, '.codex');
  mkdirSync(join(codexHome, 'hooks'), { recursive: true });
  writeFileSync(join(codexHome, 'hooks', 'zerograph-hook.mjs'), '// 0.6.x\n');
  const env = { HOME: home, PLUGIN_ROOT: join(home, 'plugin'), CLAUDE_PLUGIN_ROOT: join(home, 'plugin'), ZEROGRAPH_HOOK_SERVE: '0' };
  ['~/.codex/hooks/zerograph-hook.mjs', '$HOME/.codex/hooks/zerograph-hook.mjs', '${HOME}/.codex/hooks/zerograph-hook.mjs'].forEach((spelled, i) => {
    legacyCodex(codexHome, spelled);
    assert.equal(fire(w, { hook_event_name: 'SessionStart', source: 'startup', session_id: `s${i}` }, env, HOOK, ['--codex']), null, spelled);
  });
});

test('SubagentStart carries the rule', () => {
  const w = world();
  const c = context(fire(w, { hook_event_name: 'SubagentStart', agent_id: 'a1' }));
  assert.equal(c, RULE);
  assert.match(c, /`zerograph ask <words>`/);
  assert.deepEqual(argv(w), [], 'the rule costs no call');
});

test('a Bash rg with a word asks once, with three seeds, and keeps only the seed lines', () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  отмена\nFR-PAY-26  docs/a.md:9  возврат\n  BE-M10  docs/b.md:3  neighbour' });
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg -n "отмена записи" apps/' } };
  const c = context(fire(w, payload));
  assert.match(argv(w)[0], /ask --stale --seeds 3/);
  assert.ok(!c.includes('BE-M10'), `the indented neighbour line is dropped: ${c}`);
  assert.equal(c.split('\n').filter((l) => l.startsWith('FR-')).length, 2, c);
  assert.equal(fire(w, payload), null, 'the same query is asked once a session');
});

test('a search that touches zerograph itself, or the store, is left alone', () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  отмена' });
  for (const command of ['zerograph ask отмена | rg FR', 'rg -l TODO .zerograph/']) {
    assert.equal(fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command } }), null, command);
  }
  assert.deepEqual(argv(w), [], 'the fake never ran');
});

test('an answer that is empty, or a command that fails, is silence', () => {
  for (const w of [world({ answer: '' }), world({ answer: 'x', status: 1 })]) {
    const out = fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg "cancellation policy"' } });
    assert.equal(out, null);
  }
});

test('the twelfth injection is the last', () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  отмена' });
  let seen = 0;
  for (let i = 0; i < 15; i += 1) {
    const out = fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: `rg "cancellation policy number${i}part"` } });
    if (out) seen += 1;
  }
  assert.equal(seen, 12, 'a bounded worst case is what makes this a bet rather than a tax');
});

test('with the interceptor off, the reminder still keeps its cadence, per agent', () => {
  const w = world({ answer: 'FR-PAY-22  docs/a.md:1  отмена' });
  const env = { ZEROGRAPH_HOOK_INTERCEPT: '0' };
  let reminders = 0;
  for (let i = 0; i < 40; i += 1) {
    const out = fire(w, { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: `rg "thing${i}"` } }, env);
    if (out) reminders += 1;
  }
  assert.equal(reminders, 1, 'one reminder in forty watched calls');
  assert.deepEqual(argv(w), [], 'and no ask was run');

  // A subagent shares its parent's session id and must still count from zero.
  const out = fire(w, { hook_event_name: 'PreToolUse', agent_id: 'sub1', tool_name: 'Bash', tool_input: { command: 'rg "thing0"' } }, env);
  assert.equal(out, null, 'the subagent is at call one of its own forty, not at forty-one');
});

test('an edit to a hub injects one risk line, once, and only above LOW', () => {
  const changes = diff([{ file: 'src/db.ts', symbol: 'DatabaseService.withTenant', callers: 18 }], 'HIGH');
  const w = world({ answer: changes });
  const payload = edit(w, 'src', 'db.ts');
  const c = context(fire(w, payload));
  assert.match(c, /risk HIGH — 18 direct callers of DatabaseService\.withTenant in 18 files/);
  assert.match(argv(w)[0], /changes --depth 1 --json/);
  assert.equal(fire(w, payload), null, 'once per file and risk');

  const low = world({ answer: JSON.stringify({ risk: 'LOW', touched: [], affected: [], files: [] }) });
  assert.equal(fire(low, edit(low, 'src', 'db.ts')), null);

  const doc = world({ answer: changes });
  assert.equal(fire(doc, edit(doc, 'x.md')), null);
  assert.deepEqual(argv(doc), [], 'a document edit does not even ask');
});

test('a Codex apply_patch is scored on every file its patch writes', () => {
  const w = world({ answer: diff([
    { file: 'src/db.ts', symbol: 'DatabaseService.withTenant', callers: 18 },
    { file: 'src/slots.ts', symbol: 'freeSlots', callers: 6, files: 2 },
  ], 'HIGH') });
  const patch = ['*** Begin Patch', '*** Update File: src/db.ts', '@@', '-a', '+b', '*** Add File: notes.md', '+x',
    '*** Update File: src/old.ts', '*** Move to: src/slots.ts', '@@', '-c', '+d', '*** End Patch'].join('\n');
  const c = context(fire(w, { hook_event_name: 'PostToolUse', tool_name: 'apply_patch', tool_input: { command: patch } }));
  assert.match(c, /risk HIGH — 18 direct callers of DatabaseService\.withTenant/);
  assert.match(c, /risk MEDIUM — 6 direct callers of freeSlots in 2 files/, 'a moved file is scored at its new path');

  const doc = world({ answer: diff([{ file: 'src/db.ts', symbol: 'x', callers: 18 }], 'HIGH') });
  const md = { hook_event_name: 'PostToolUse', tool_name: 'apply_patch', tool_input: { command: '*** Begin Patch\n*** Add File: x.md\n+y\n*** End Patch' } };
  assert.equal(fire(doc, md), null);
  assert.deepEqual(argv(doc), [], 'a patch that writes no TypeScript does not even ask');
});

test('an edit is scored on its own file\'s callers, not on the rest of the diff', () => {
  const w = world({ answer: diff([
    { file: 'src/db.ts', symbol: 'DatabaseService.withTenant', callers: 40 },
    { file: 'src/claims.ts', symbol: 'staleClaims', callers: 1 },
    { file: 'src/slots.ts', symbol: 'freeSlots', callers: 6, files: 2 },
  ], 'CRITICAL') });
  assert.equal(fire(w, edit(w, 'src', 'claims.ts')), null, 'one caller is silence, whatever else the diff holds');
  assert.match(context(fire(w, edit(w, 'src', 'slots.ts'))), /risk MEDIUM — 6 direct callers of freeSlots in 2 files/,
    'the level is the file\'s own, not the CRITICAL the whole diff scored');
});

test('a caller that imports through a barrel is still a caller of the edited file', () => {
  const via = 'sym:src/index.ts::DatabaseService.withTenant';
  const affected = Array.from({ length: 6 }, (_, i) => ({ id: `sym:src/c${i}.ts::caller`, at: `src/c${i}.ts:1`, depth: 1, via }));
  const touched = [{ id: 'sym:src/db.ts::DatabaseService', at: 'src/db.ts:3-40', indexed: true }];
  const w = world({ answer: JSON.stringify({ touched, affected, files: [], risk: 'HIGH' }) });
  assert.match(context(fire(w, edit(w, 'src', 'db.ts'))), /risk MEDIUM — 6 direct callers of DatabaseService\.withTenant in 6 files/,
    'the barrel alias names the member, not the file it was declared in');
  const clash = world({ answer: JSON.stringify({ touched: [...touched, { id: 'sym:src/other.ts::DatabaseService', at: 'src/other.ts:1-9', indexed: true }], affected, files: [], risk: 'HIGH' }) });
  assert.equal(fire(clash, edit(clash, 'src', 'db.ts')), null, 'a name two files in the diff share is not attributed by name');
});

test('the level is the binary\'s: MEDIUM at 5 callers or 3 files, HIGH at 15 or 10, CRITICAL at 30 or 25', () => {
  for (const [callers, files, level] of [
    [4, 2, null], [5, 1, 'MEDIUM'], [4, 3, 'MEDIUM'],
    [15, 1, 'HIGH'], [10, 10, 'HIGH'], [30, 1, 'CRITICAL'], [25, 25, 'CRITICAL'],
  ]) {
    const w = world({ answer: diff([{ file: 'src/hub.ts', symbol: 'hub', callers, files }], 'CRITICAL') });
    const c = context(fire(w, edit(w, 'src', 'hub.ts')));
    assert.equal(c?.match(/risk (\w+)/)?.[1] ?? null, level, `${callers} callers in ${files} files: ${c}`);
  }
});

test('the Agent prompt is extended only when the fallback is asked for', () => {
  const w = world();
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Agent', tool_input: { prompt: 'find the tenant guard' } };
  assert.equal(fire(w, payload), null, 'off by default: SubagentStart is the primary and this would double it');
  const out = fire(w, payload, { ZEROGRAPH_HOOK_AGENT_INPUT: '1' });
  assert.ok(out.hookSpecificOutput.updatedInput.prompt.startsWith('find the tenant guard'), JSON.stringify(out));
  assert.ok(out.hookSpecificOutput.updatedInput.prompt.endsWith(rule()), JSON.stringify(out));
  assert.equal(out.hookSpecificOutput.permissionDecision, undefined, 'no decision: the call proceeds as it would have');
});

test('a repository that asked for an index and cannot build one gets one build notice and then silence', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  writeFileSync(join(w.root, 'zerograph.toml'), '');
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg "cancellation policy"' } };
  const nothing = { PATH: join(w.root, 'nothing-here'), ZEROGRAPH_BIN: '' };
  const notice = context(fire(w, payload, nothing));
  assert.match(notice, /zerograph build/);
  assert.match(notice, /already embedded.*vector cache/, 'a large repository is not told it costs a whole embed');
  assert.doesNotMatch(notice, /couple of minutes/, 'a large repository takes far longer than that');
  assert.equal(fire(w, { ...payload, tool_input: { command: 'rg "another question here"' } }, nothing), null, 'once a session');
});

test('a repository that never asked for an index hears nothing from a search', () => {
  const w = world({ store: false });
  mkdirSync(join(w.root, '.git'));
  const payload = { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command: 'rg "cancellation policy"' } };
  assert.equal(fire(w, payload), null);
});

test('malformed input is silence with exit zero', () => {
  const w = world();
  const r = spawnSync(process.execPath, [HOOK], { input: 'not json at all', encoding: 'utf8', env: { ...process.env, TMPDIR: join(w.root, 'tmp') } });
  assert.equal(r.status, 0);
  assert.equal(r.stdout, '');
});

test('a home directory that is a checkout keeps its own PATH entries', { skip: WIN && 'HOME is not the Windows profile' }, () => {
  const home = realpathSync(mkdtempSync(join(tmpdir(), 'zerograph-home-')));
  mkdirSync(join(home, '.git'));
  mkdirSync(join(home, 'notes'));
  const bin = join(home, '.nvm', 'bin');
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, 'zerograph'), '');
  chmodSync(join(bin, 'zerograph'), 0o755);
  const saved = { HOME: process.env.HOME, PATH: process.env.PATH };
  Object.assign(process.env, { HOME: home, PATH: [bin, '/usr/bin'].join(delimiter) });
  try {
    assert.equal(onPath(join(home, 'notes')), join(bin, 'zerograph'));
  } finally {
    Object.assign(process.env, saved);
  }
});

/** A home whose update cache holds `body`, and the variables that make the hook read it. */
function updateHome(w, body) {
  const home = join(w.root, 'home');
  mkdirSync(join(home, '.cache', 'zerograph'), { recursive: true });
  if (body !== undefined) writeFileSync(join(home, '.cache', 'zerograph', 'update.json'), body);
  return { HOME: home, USERPROFILE: home, ZEROGRAPH_NO_UPDATE_CHECK: null, NO_UPDATE_NOTIFIER: null, ZEROGRAPH_HOOK_SERVE: '0' };
}

const cached = (versions) => JSON.stringify({ versions });
const CACHED = cached({ '0.7.0': { checked: 1, latest: '0.7.1' } });

test('SessionStart shows a cached newer release to the person and tells the model once', () => {
  const w = world({ answer: 'zerograph: 3 doc nodes' });
  const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, updateHome(w, CACHED));
  assert.equal(out.systemMessage, '▲ zerograph 0.7.1 available (you have 0.7.0)\n  npm i -g zerograph@0.7.1 · pnpm add -g zerograph@0.7.1');
  const c = context(out);
  assert.ok(c.startsWith('zerograph: 3 doc nodes'), c);
  assert.equal(c.split('\n').filter((l) => l.includes('0.7.1')).length, 1, c);

  const bare = world({ store: false });
  const shown = fire(bare, { hook_event_name: 'SessionStart', source: 'startup' }, updateHome(bare, CACHED));
  assert.ok(shown.systemMessage.startsWith('▲ zerograph 0.7.1'), JSON.stringify(shown));
});

test('the message is about the binary the hook runs, not the one that last wrote the cache', () => {
  for (const [version, versions] of [
    ['0.7.1', { '0.7.0': { checked: 1, latest: '0.7.1' } }],
    ['0.6.0', { '0.7.0': { checked: 1, latest: '0.7.1' } }],
    ['0.8.0', { '0.8.0': { checked: 1, latest: '0.7.1' } }],
    ['0.7.0', { '0.7.0': { claimed: 1, latest: '0.7.1' } }],
  ]) {
    const w = world({ answer: 'zerograph: 3 doc nodes', version });
    const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, updateHome(w, cached(versions)));
    assert.equal(out.systemMessage, undefined, `${version} ${JSON.stringify(versions)}`);
    assert.equal(context(out), 'zerograph: 3 doc nodes');
  }
  const pre = world({ answer: 'zerograph: 3 doc nodes', version: '0.8.0-rc.1' });
  const out = fire(pre, { hook_event_name: 'SessionStart', source: 'startup' }, updateHome(pre, cached({ '0.8.0-rc.1': { checked: 1, latest: '0.8.0' } })));
  assert.match(out.systemMessage, /0\.8\.0 available \(you have 0\.8\.0-rc\.1\)/);
});

test('Codex gets the brief line and no systemMessage', () => {
  const w = world({ answer: 'zerograph: 3 doc nodes' });
  const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, updateHome(w, CACHED), HOOK, ['--codex']);
  assert.equal(out.systemMessage, undefined);
  assert.match(context(out), /0\.7\.1 is out/);
});

test('no update message, and no version asked, when the cache is missing, garbled or names nothing newer, or the person opted out', () => {
  for (const [body, extra] of [
    [undefined, {}],
    ['{not json', {}],
    [cached({ '0.7.0': { checked: 1, latest: null } }), {}],
    [cached({ '0.7.0': { checked: 1, latest: '0.7.1; rm -rf ~' } }), {}],
    [CACHED, { ZEROGRAPH_NO_UPDATE_CHECK: '1' }],
    [CACHED, { NO_UPDATE_NOTIFIER: '1' }],
  ]) {
    const w = world({ answer: 'zerograph: 3 doc nodes' });
    const out = fire(w, { hook_event_name: 'SessionStart', source: 'startup' }, { ...updateHome(w, body), ...extra });
    assert.equal(out.systemMessage, undefined, `${body} ${JSON.stringify(extra)}`);
    assert.equal(context(out), 'zerograph: 3 doc nodes');
    assert.ok(!argv(w).some((l) => l.includes('--version')), argv(w).join(' | '));
  }
});
