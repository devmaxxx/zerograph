#!/usr/bin/env node
/**
 * The one zerograph hook, dispatched on the event the harness names in its payload.
 *
 *  SessionStart   → a repository brief, so the graph's state and its five commands are in context
 *                   at start and again after a compaction, without a 7k-token CLAUDE.md.
 *  PreToolUse     → a Bash/Grep/Glob search gets the graph's top three lines beside it, under gates
 *                   written to keep an injection out rather than to put one in: an injection is
 *                   paid once as output and then on every later turn as input. Read is not a search.
 *  PostToolUse    → an Edit/Write to TypeScript gets one risk line when the touched symbol's
 *                   callers cross MEDIUM. The one call here that may refresh the store.
 *  SubagentStart  → every subagent gets the rule. Prose asking the parent to pass it on reached 19
 *                   of 146 subagent spawns in the recorded sessions, and 0 of the 140 that did not
 *                   carry it ever ran the tool.
 *
 * Contract: exit 0 always, stdout only when there is something to say, stderr only under
 * ZEROGRAPH_HOOK_DEBUG. Every call it waits on is --no-dense, and every one but `changes` is --stale;
 * what it starts and leaves running — a `serve`, an `update --detach` — is neither.
 */
import { existsSync, readFileSync, writeFileSync, mkdirSync, realpathSync, lstatSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { join, dirname, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir, homedir } from 'node:os';
import { createHash } from 'node:crypto';

import { binaryOf, checkoutOf as repositoryOf, childEnv as cutEnv, commandOf, pathKey } from './launcher.mjs';

const MAX_INJECTIONS = 12;   // 12 × ~107 tokens: a third of the CLAUDE.md sections this replaces
const SEEDS = 3;
const LINE_CUT = 160;
const REMINDER_EVERY = 40;
const TIMEOUT_MS = 5000;
/**
 * The seconds of in-process refresh a call gets, passed as `ZEROGRAPH_READER_BUDGET`. Under the
 * binary's own 10 s a refresh of 5–10 s was killed at the timeout above with its work lost, and the
 * next call started it again; at 2 s what does not fit goes to a detached `update`, and the other
 * 3 s are the process start, the store load, the walk and the answer.
 */
const HOOK_BUDGET = 2;
/** What the autostarted `serve` is told: leave after half an hour unasked, forget the model after five minutes. */
const SERVE_IDLE = 1800;
const SERVE_IDLE_MODEL = 300;   // measured on the 33.5k-row copy: 0.08 s resident, 0.77 s after a drop

const COMMAND = 'zerograph';

/** The rule handed to every subagent. */
const RULE = `This repository has a zerograph index. Ask it before grepping for a concept: \`${COMMAND} ask <words>\`
answers by meaning, \`${COMMAND} impact <Symbol>\` names who calls a symbol, \`${COMMAND} changes\` maps
your diff onto the callers it reaches. Read only the \`path:line\` those answers print, and cite one
for every claim; when nothing printed holds it, say so.
Grep is still the right tool for a literal — an env var name, a string in a test, a config key.
`;

const BUILDING_NOTICE =
  `zerograph: this checkout has no index yet, so one is being built in the background ` +
  `(\`${COMMAND} status\` shows how far it has got); until it lands, search as usual.`;

const BUILD_NOTICE =
  `zerograph: this repository has no index yet. \`${COMMAND} build\` writes one in a few minutes on a ` +
  'small repository and longer on a large one, though rows another checkout on this machine already ' +
  'embedded come from its vector cache instead of being embedded again. It answers by meaning ' +
  'afterwards; until then this notice is all it can say.';

function rule() {
  return RULE;
}

function env(suffix) {
  return process.env[`ZEROGRAPH_${suffix}`];
}

function debug(...args) {
  if (env('HOOK_DEBUG')) console.error('[zerograph-hook]', ...args);
}

const WIN = process.platform === 'win32';

/** The checkout `root` is in, or `root` outside git: what a PATH entry must stay out of. */
function checkoutOf(root) {
  return root && repositoryOf(root);
}

/**
 * The `zerograph` the machine's own PATH resolves, as an absolute path, or null; never one in the
 * checkout, which the hook runs in every repository a session opens. On Windows every `.exe`
 * outranks every `.cmd`.
 */
function onPath(root, platform = process.platform) {
  return binaryOf('', { path: process.env[pathKey(process.env, platform === 'win32')], checkout: checkoutOf(root), win: platform === 'win32' }).bin ?? null;
}

/**
 * `ZEROGRAPH_BIN`, else the plugin's `binary` option, else PATH; null when none names one, and
 * then nothing is spawned. The rules are the MCP server's, from `launcher.mjs`: a relative path
 * names the checkout's file and is refused.
 */
function binary(root) {
  const named = env('BIN') || process.env.CLAUDE_PLUGIN_OPTION_BINARY || '';
  const found = binaryOf(named, { path: process.env[pathKey(process.env, WIN)], checkout: checkoutOf(root), win: WIN });
  if (found.error) debug(found.error);
  return found.bin ?? null;
}

function childEnv(root, extra = {}) {
  return cutEnv(process.env, checkoutOf(root), WIN, extra);
}

/** `bin args` as an argv `spawn` takes without a shell, or null when `cmd` would read an argument as code. */
function command(bin, args) {
  const c = commandOf(bin, args, process.env.ComSpec);
  if (c.error) debug(c.error);
  return c.argv ? c : null;
}

/**
 * `HOOK_BUDGET`, or a smaller whole number of seconds the environment or the repository's
 * `zerograph.toml` already sets. The variable outranks every file, so passing 2 would otherwise
 * lift a repository's `reader_budget = 0` back to 2.
 */
function readerBudget(root) {
  const set = String(env('READER_BUDGET') ?? '').trim();
  let file = '';
  try { file = readFileSync(join(root, 'zerograph.toml'), 'utf8').match(/^\s*reader_budget\s*=\s*(\d+)\s*(?:#.*)?$/m)?.[1] ?? ''; } catch {}
  const smallest = [set, file].filter((v) => /^\d+$/.test(v)).map(Number).reduce((a, b) => Math.min(a, b), HOOK_BUDGET);
  return String(smallest);
}

// Starts a command and, once `ms` pass, hands its pid to `kill`. Run in a Node of its own because
// `spawnSync`'s timeout kills only the process it started: the `zerograph` a batch file's `cmd`
// started would outlive the five seconds this hook waits, and go on holding the store.
const BOUNDED = `
const { spawn, spawnSync } = require('node:child_process');
const [file, args, ms, kill] = JSON.parse(process.argv[1]);
const child = spawn(file, args, { stdio: ['ignore', 'inherit', 'inherit'], windowsHide: true });
const timer = setTimeout(() => {
  spawnSync(kill[0], [...kill.slice(1), String(child.pid)], { stdio: 'ignore', windowsHide: true });
  process.exit(124);
}, ms);
child.on('error', () => process.exit(127));
child.on('exit', (code) => { clearTimeout(timer); process.exit(code ?? 1); });
`;

/** `spawnSync` of a command whose whole process tree `kill` ends after `ms`. */
function bounded(file, args, options, kill, ms = TIMEOUT_MS) {
  // The outer timeout is only a backstop for the helper itself.
  return spawnSync(process.execPath, ['-e', BOUNDED, JSON.stringify([file, args, ms, kill])], { ...options, timeout: ms + 2000 });
}

// By its full path: a bare `taskkill` would be looked up in the working directory first.
function treeKill() {
  return [join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'taskkill.exe'), '/T', '/F', '/PID'];
}

function run(root, args) {
  const bin = binary(root);
  const c = bin && command(bin, ['--repo', root, '--no-dense', ...args]);
  if (!c) return '';
  const [file, ...argv] = c.argv;
  const options = {
    // Never the checkout: npm's Windows `.cmd` shim runs a bare `node` through `cmd`, which looks
    // in the working directory first. The binary is told its repository by `--repo`.
    cwd: dirname(bin),
    encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
    env: childEnv(root, { ZEROGRAPH_READER_BUDGET: readerBudget(root) }),
  };
  const r = c.batch ? bounded(file, argv, options, treeKill()) : spawnSync(file, argv, { ...options, timeout: TIMEOUT_MS });
  if (r.status !== 0) debug('exit', r.status, args.join(' '), (r.stderr || '').slice(0, 200));
  // Identifiers and headings in the answer are the repository's text, and it is about to enter an
  // agent's context and possibly a terminal log; control characters other than tab and newline carry nothing.
  return r.status === 0 ? r.stdout.replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, '') : '';
}

function built(root) {
  return existsSync(join(root, '.zerograph', 'manifest.json'));
}

/**
 * The repository the hook asks about, chosen the way the CLI chooses it without `--repo`: inside a
 * checkout its top level, whether or not a store is built there, so the hook never answers from a
 * package's store that `zerograph status` run beside it does not see. Outside git, the nearest
 * directory at or above `start` that carries an index. The harness's `cwd` follows a `cd`, and a
 * nested worktree is a checkout of its own, so it never borrows its parent's index.
 */
function indexRoot(start) {
  const checkout = checkoutRoot(start);
  if (checkout) return checkout;
  for (let d = start; ; d = dirname(d)) {
    if (built(d)) return d;
    if (dirname(d) === d) return null;
  }
}

/** The checkout `start` is in: the nearest directory at or above it holding a `.git`, file or directory. */
function checkoutRoot(start) {
  for (let d = start; ; d = dirname(d)) {
    if (existsSync(join(d, '.git'))) return d;
    if (dirname(d) === d) return null;
  }
}

/**
 * Whether a checkout asked for an index: it carries a `zerograph.toml`, it or its main worktree has
 * a built index, or either holds the hook an older `install-agent` wrote. The plugin runs this hook
 * in every repository a session opens, and a build started in each of them would be the cost a git
 * hook used to pay on every worktree add.
 */
function optedIn(checkout) {
  const asked = (dir) => ['zerograph.toml', '.claude/hooks/zerograph-hook.mjs', '.codex/hooks/zerograph-hook.mjs']
    .some((p) => existsSync(join(dir, ...p.split('/'))));
  const main = mainCheckout(checkout);
  return asked(checkout) || built(checkout) || (main !== null && (asked(main) || built(main)));
}

/** The main worktree of a linked one, read off its `.git` file and the `commondir` beside it, or null. */
function mainCheckout(checkout) {
  try {
    const gitdir = readFileSync(join(checkout, '.git'), 'utf8').match(/^gitdir:\s*(.+?)\s*$/m)?.[1];
    if (!gitdir) return null;
    const linked = isAbsolute(gitdir) ? gitdir : join(checkout, gitdir);
    const common = readFileSync(join(linked, 'commondir'), 'utf8').trim();
    const commonDir = isAbsolute(common) ? common : join(linked, common);
    return /^\.git$/.test(commonDir.split(/[\\/]/).filter(Boolean).pop() ?? '') ? dirname(commonDir) : null;
  } catch { return null; }
}

/** Whether `binary` names something that will start: an explicit one, or PATH's. */
function binaryFound(checkout) {
  return binary(checkout) !== null;
}

/**
 * Words worth asking, or null. Four letters is the floor because a shorter token is a flag, an
 * extension or a variable name far more often than it is a concept; six words is the ceiling
 * because the seventh never changed an answer and every one costs prompt.
 */
export function queryWords(pattern) {
  if (!pattern) return null;
  if (/\//.test(pattern) && !/\s/.test(pattern)) return null;            // a path, not a question
  // A class escape (`\b`, `\w`, `\s`) goes whole: dropping only the backslash glues its letter to
  // the word beside it, and `\bwithTenant\b` would ask about `bwithTenant`.
  const bare = pattern.replace(/\\[A-Za-z]/g, ' ').replace(/[\\^$.*+?()[\]{}|]/g, ' ').trim();
  if (!bare) return null;
  // A lone identifier or a document id is sent whole: the exact stage answers it in 60 ms and
  // 20–40 tokens, which is the best case this hook has. A milestone or task id (`BE-M17`,
  // `BE-M01-T03`) ends in a capital and digits, and has no four-letter word to fall back on.
  if (/^[A-Za-z][A-Za-z0-9_]*$/.test(bare) && bare.length >= 4) return [bare];
  if (/^[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*-[A-Z]?\d{1,4}$/.test(bare)) return [bare];
  // A leading dash is dropped: `--rerank` sent as a word is parsed by `ask` as its paid flag.
  const words = bare.split(/[^\p{L}\p{N}_-]+/u).map((w) => w.replace(/^-+/, ''))
    .filter((w) => /\p{L}{4,}/u.test(w)).slice(0, 6);
  return words.length ? words : null;
}

/**
 * A shell line cut into its stages, each marked by whether a pipe feeds it. Quoted strings and
 * backslash escapes are kept whole, so the `|` in `grep -E "a|b"` or `grep a\|b` is a pattern and
 * not a pipe.
 */
function stages(command) {
  const out = [{ piped: false, text: '' }];
  for (const [part] of command.matchAll(/(?:"[^"]*"|'[^']*'|\\[\s\S]|[^"'\\|;&\n])+|\|&|\|\||&&|[|;&\n]/g)) {
    const last = out[out.length - 1];
    // A newline after an operator continues the line: `git log |\n  grep x` is still fed by the pipe.
    if (part === '\n' && !last.text.trim()) continue;
    if (/^(?:\|&|\|\||&&|[|;&\n])$/.test(part)) out.push({ piped: part === '|' || part === '|&', text: '' });
    // A backslash-newline continues the line and reads as a space; kept, its backslash is taken for
    // the pattern of an `rg` whose arguments start on the next line.
    else last.text += part.replace(/\\\r?\n/g, ' ');
  }
  return out;
}

/**
 * The flags whose value is the next word, one list per tool, read off `rg --help` and
 * `grep --help`: a shared list is wrong for both, because the same letter means different things
 * under each — `-r` is a replacement under `rg` and recursion under `grep`, `-E` an encoding and
 * extended syntax, `-T` a type to exclude and a leading tab. A flag missing here hands its value to
 * the hook as the pattern, and `--exclude-dir node_modules` spent an injection on `node_modules`.
 * A value attached with `=` is one token that starts with `-` and needs no entry; `--color` under
 * `grep` only takes its value that way, so it is absent from that list on purpose.
 */
const VALUE_FLAGS = {
  rg: /^-(A|B|C|E|M|T|d|f|g|j|m|r|t|-after-context|-before-context|-context|-color|-colors|-context-separator|-dfa-size-limit|-encoding|-engine|-field-context-separator|-field-match-separator|-file|-generate|-glob|-hostname-bin|-hyperlink-format|-iglob|-ignore-file|-max-columns|-max-count|-max-depth|-max-filesize|-path-separator|-pre|-pre-glob|-regex-size-limit|-replace|-sort|-sortr|-threads|-type|-type-add|-type-clear|-type-not)$/,
  grep: /^-(A|B|C|D|d|f|m|-after-context|-before-context|-context|-devices|-directories|-file|-max-count|-exclude|-exclude-dir|-exclude-from|-include|-include-dir|-label|-binary-files|-group-separator)$/,
};

/**
 * What a tool call searches for. Bash is the surface that matters: a pass over 38 recorded sessions
 * found Grep 0, Glob 0 and Bash 2,727 calls, 933 of them running `rg` or `grep` — the harness tells
 * the model to search through Bash, so a hook that only watched Grep watched nothing.
 */
export function searchPattern(tool, input) {
  if (!input) return null;
  if (tool === 'Grep') return input.pattern || null;
  if (tool === 'Glob') {
    const segments = String(input.pattern || '').split(/[/*]+/)
      .map((s) => s.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, ''))
      .filter((s) => /\p{L}{4,}/u.test(s));
    return segments.sort((a, b) => b.length - a.length)[0] || null;
  }
  if (tool !== 'Bash') return null;
  const command = String(input.command || '');
  if (!/\b(rg|grep)\b/.test(command)) return null;
  for (const stage of stages(command)) {
    // A stage a pipe feeds searches what the stage before it printed, not the repository:
    // `gh auth status 2>&1 | grep -E "Logged in|Active account"` came back with three graph lines
    // about accounts. `| xargs grep` is skipped too — a missed injection costs nothing, and a wrong
    // one is paid on every later turn.
    if (stage.piped) continue;
    // Quoted first: a phrase is one argument, and splitting on whitespace would send the graph the
    // first word of `rg -n "cancellation policy"` and dedup every later phrase starting the same way.
    const tokens = stage.text.match(/"[^"]*"|'[^']*'|\S+/g) || [];
    const at = tokens.findIndex((t) => /^(rg|grep)$/.test(t) || /\/(rg|grep)$/.test(t));
    if (at < 0) continue;
    const takesValue = VALUE_FLAGS[tokens[at].replace(/.*\//, '')];
    for (let i = at + 1; i < tokens.length; i += 1) {
      const t = tokens[i];
      // `-e` and `--regexp` carry the pattern itself; skipping their value would ask about the path after it.
      if (/^-(e|-regexp)$/.test(t)) return (tokens[i + 1] || '').replace(/^['"]|['"]$/g, '') || null;
      if (takesValue.test(t)) { i += 1; continue; }
      if (t.startsWith('-')) continue;
      return t.replace(/^['"]|['"]$/g, '');
    }
  }
  return null;
}

/**
 * The directory the hook keeps its markers in: one per user under the temp directory, made 0700
 * and checked to be ours on every use. A name anyone can predict in a shared `/tmp` is a name
 * another user can create first, or point at a file of yours; null here means no state, which
 * costs a repeated answer and nothing else. Windows' temp directory is already per-user.
 */
function stateRoot() {
  const uid = process.getuid?.();
  const root = join(tmpdir(), uid === undefined ? 'zerograph-hook' : `zerograph-hook-${uid}`);
  if (uid === undefined) return root;
  try {
    mkdirSync(root, { recursive: true, mode: 0o700 });
    const st = lstatSync(root);
    return st.isDirectory() && st.uid === uid && (st.mode & 0o077) === 0 ? root : null;
  } catch { return null; }
}

/**
 * The per-session state directory. The key is built from ids the harness supplies, so it is
 * reduced to word characters before it becomes a path segment: an id carrying a slash or a `..`
 * would otherwise put this hook's empty marker files anywhere the process can write.
 */
function stateDir(sessionKey) {
  const root = stateRoot();
  return root && join(root, sessionKey.replace(/\W+/g, '-'));
}

/** True the first time this session asks about this key; an unwritable tmp answers once and never dedups. */
function once(sessionKey, kind, key) {
  const dir = stateDir(sessionKey);
  if (!dir) return true;
  const file = join(dir, `${kind}-${createHash('sha1').update(key).digest('hex').slice(0, 16)}`);
  if (existsSync(file)) return false;
  try { mkdirSync(dir, { recursive: true }); writeFileSync(file, ''); } catch { /* answer this one */ }
  return true;
}

function count(sessionKey, name) {
  const dir = stateDir(sessionKey);
  if (!dir) return 0;
  try { return Number(readFileSync(join(dir, `count-${name}`), 'utf8')) || 0; } catch { return 0; }
}

function bump(sessionKey, name) {
  const n = count(sessionKey, name) + 1;
  const dir = stateDir(sessionKey);
  if (!dir) return n;
  try { mkdirSync(dir, { recursive: true }); writeFileSync(join(dir, `count-${name}`), String(n)); } catch { /* no state, no cadence */ }
  return n;
}

function isWatchedCall(tool, input) {
  if (tool === 'Bash') return /\b(rg|grep)\b/.test(String((input || {}).command || '')) || /\.(ts|tsx|md)\b/.test(String((input || {}).command || ''));
  return tool === 'Grep' || tool === 'Glob';
}

/**
 * On by default because the harness said so, under a rule committed before the number was read:
 * interception ships when its tokens per hit are lower than reminder-only's. Sonnet, twice over the
 * twelve tasks — 137,991 and 129,243 tokens per hit with it, 194,011 and 169,036 without, the two
 * ranges not overlapping — and 11/11 hits against 11/9. `ZEROGRAPH_HOOK_INTERCEPT=0` turns it off.
 * `docs/bench/2026-09-09-agent-surface-results.md` has all four runs and what they are worth.
 */
function intercept(payload, root, sessionKey) {
  if (env('HOOK_INTERCEPT') === '0') return null;
  const input = payload.tool_input || {};
  // A search the agent runs *with* zerograph is not one to answer beside.
  if (payload.tool_name === 'Bash' && /\bzerograph\b/.test(String(input.command || ''))) return null;
  if (/\.zerograph\//.test(JSON.stringify(input))) return null;
  const words = queryWords(searchPattern(payload.tool_name, input));
  if (!words) return null;
  if (!once(sessionKey, 'ask', words.join(' ').toLowerCase())) return null;
  if (count(sessionKey, 'injections') >= MAX_INJECTIONS) return null;
  const lines = run(root, ['ask', '--stale', '--seeds', String(SEEDS), ...words])
    .split('\n').filter((l) => l && !l.startsWith('  ')).map((l) => l.slice(0, LINE_CUT));
  if (!lines.length) return null;
  bump(sessionKey, 'injections');
  return [`${COMMAND} ask ${words.join(' ')} →`, ...lines, `(read only these path:line; \`${COMMAND} ask\` for more)`].join('\n');
}

function reminder(payload, sessionKey) {
  if (!isWatchedCall(payload.tool_name, payload.tool_input)) return null;
  const n = bump(sessionKey, 'watched');
  return n % REMINDER_EVERY === 0
    ? `zerograph: this repository has an index — \`${COMMAND} ask <words>\` answers by meaning, no tokens.`
    : null;
}

/**
 * The files an edit wrote. Claude Code names one in `file_path`; Codex's `apply_patch` carries a
 * patch in `command`, where every file it adds, updates or moves to has its own header line, relative
 * to the project.
 */
function editedFiles(input, root) {
  if (input.file_path) return [String(input.file_path)];
  const patch = typeof input.command === 'string' ? input.command : '';
  const files = [...patch.matchAll(/^\*\*\* (?:Add File|Update File|Move to): (.+?)\s*$/gm)].map((m) => m[1]);
  return [...new Set(files)].map((f) => (isAbsolute(f) ? f : join(root, f)));
}

function riskLine(payload, root, sessionKey) {
  const files = editedFiles(payload.tool_input || {}, root).filter((f) => /\.tsx?$/.test(f));
  if (!files.length) return null;
  let r;
  try { r = JSON.parse(run(root, ['changes', '--depth', '1', '--json'])); } catch { return null; }
  if (!r || !['MEDIUM', 'HIGH', 'CRITICAL'].includes(r.risk)) return null;
  const lines = files.map((file) => fileRisk(r, file, root, sessionKey)).filter(Boolean);
  return lines.length ? lines.join('\n') : null;
}

function fileRisk(r, file, root, sessionKey) {
  // `changes` covers the whole working diff: keep only what the edited file contributes, or an edit to
  // a leaf file is told about the callers of symbols some other file in the diff touched.
  const rel = relative(root, file).split('\\').join('/');
  const name = (id) => String(id || '').split('::').pop();
  const touched = (r.touched || []).filter((t) => t.indexed !== false);
  const mine = touched.filter((t) => String(t.at || '').startsWith(`${rel}:`));
  // A caller that imports through a barrel is reached by the barrel's alias, `sym:<index.ts>::<Name>`,
  // so a symbol is matched by name as well — unless another file in the diff touched the same name.
  const theirs = new Set(touched.filter((t) => !mine.includes(t)).map((t) => name(t.id)));
  const names = mine.map((t) => name(t.id)).filter((n) => !theirs.has(n));
  const direct = (r.affected || []).filter((d) => d.depth === 1 && (String(d.via || '').startsWith(`sym:${rel}::`)
    || names.some((n) => name(d.via) === n || name(d.via).startsWith(`${n}.`))));
  if (!direct.length) return null;
  const files = new Set(direct.map((d) => String(d.at || '').split(':')[0])).size;
  // `r.risk` scores the whole diff; the level is this file's own direct callers, on the thresholds
  // `impact::risk` prints beside its counts.
  const n = direct.length;
  const risk = n >= 30 || files >= 25 ? 'CRITICAL' : n >= 15 || files >= 10 ? 'HIGH' : n >= 5 || files >= 3 ? 'MEDIUM' : null;
  if (!risk) return null;
  if (!once(sessionKey, 'risk', `${file}:${risk}`)) return null;
  const called = [...new Set(direct.map((d) => name(d.via)))].slice(0, 3).join(', ');
  return `zerograph changes: risk ${risk} — ${n} direct callers of ${called} in ${files} files; \`${COMMAND} changes --depth 1\` lists them.`;
}

const EXACT = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const isExact = (v) => typeof v === 'string' && EXACT.test(v);

/**
 * A newer release the CLI's daily check left in `~/.cache/zerograph/update.json` for the binary
 * this hook runs, as a line for the brief and a message for the person, or null. Never fetched:
 * the hook has no budget for the registry. The binary is asked its version, once and only when the
 * file names a newer release at all, since an upgrade made by hand leaves the old entry in place.
 */
function updateNotice(root) {
  if ('NO_UPDATE_NOTIFIER' in process.env || env('NO_UPDATE_CHECK') !== undefined) return null;
  let versions;
  try { versions = JSON.parse(readFileSync(join(homedir(), '.cache', 'zerograph', 'update.json'), 'utf8')).versions; } catch { return null; }
  if (!versions || typeof versions !== 'object' || !Object.values(versions).some((e) => isExact(e?.latest))) return null;
  const current = run(root, ['--version']).trim().match(/^zerograph (\S+)$/)?.[1];
  const entry = isExact(current) && Object.hasOwn(versions, current) ? versions[current] : null;
  const latest = entry?.checked ? entry.latest : null;
  if (!isExact(latest) || !newer(latest, current)) return null;
  const [npm, pnpm] = installCommands(`zerograph@${latest}`);
  return {
    brief: `zerograph: ${latest} is out and this is ${current}; \`${npm}\` or \`${pnpm}\`.`,
    message: `▲ zerograph ${latest} available (you have ${current})\n  ${npm} · ${pnpm}`,
  };
}

function installCommands(spec) {
  return [`npm i -g ${spec}`, `pnpm add -g ${spec}`];
}

/**
 * For a checkout that asked for an index and has no binary to run, built or not: a teammate who got
 * the plugin from the repository's settings would otherwise get a silent hook and never learn why.
 */
function missingNotice(cwd) {
  const checkout = cwd && checkoutRoot(cwd);
  if (!checkout || !optedIn(checkout) || binaryFound(checkout)) return null;
  const [npm, pnpm] = installCommands('zerograph');
  return {
    brief: `zerograph: this repository uses zerograph and it is not installed here, so \`${COMMAND}\` and the graph tools are unavailable until \`${npm}\` or \`${pnpm}\` installs it.`,
    message: `▲ zerograph is not installed — this repository uses it\n  ${npm} · ${pnpm}`,
  };
}

function sessionNotice(p, root) {
  return missingNotice(p.cwd || root) ?? updateNotice(root);
}

/** Whether `a` is a later release than `b`: by the three numbers, then a release over its pre-release. */
function newer(a, b) {
  const split = (v) => { const [core, pre] = v.split('+')[0].split(/-(.*)/s); return [core.split('.').map(Number), pre]; };
  const [[x, xp], [y, yp]] = [split(a), split(b)];
  for (let i = 0; i < 3; i += 1) if (x[i] !== y[i]) return x[i] > y[i];
  return yp !== undefined && xp === undefined;
}

function brief(root) {
  const text = run(root, ['prime']);
  return text ? text.trimEnd() : null;
}

/**
 * Starts a resident `serve` for this repository, detached, and never waits on it. No probe first:
 * a second `serve` refuses to bind while one answers and exits by itself, so the start *is* the
 * check. What it buys is the difference between a gate that fires and one that times out — a fused
 * ask is ~0.4 s against a cold process and ~80 ms through the socket, inside a 5 s hook budget on a
 * machine that is also running a build. `--idle-model` is what keeps that cheap between questions.
 */
function autostartServe(root) {
  if (env('HOOK_SERVE') === '0') return false;
  return detached(root, ['--repo', root, 'serve', '--idle', String(SERVE_IDLE), '--idle-model', String(SERVE_IDLE_MODEL)]);
}

/**
 * Starts `zerograph <args>` and leaves it running. Whether the start was handed to the OS, which
 * is not whether the binary was found: that failure arrives later, as an event.
 */
function detached(root, args) {
  const bin = binary(root);
  if (!bin) return false;
  try {
    const c = command(bin, args);
    if (!c) return false;
    const [file, ...argv] = c.argv;
    const child = spawn(file, argv, { cwd: dirname(bin), detached: true, stdio: 'ignore', windowsHide: true, env: childEnv(root) });
    // A spawn that fails reports it as an event, not as a throw, and an unheard `error` event ends
    // this process — which would turn "no binary on PATH" into a hook that dies on stderr at every
    // session start. The listener is what keeps the contract: exit 0, and say nothing.
    child.on('error', (e) => debug(args[2], e.message));
    child.unref();
    return true;
  } catch (e) { debug(args[2], e.message); return false; }
}

/**
 * A detached `update`, at background priority, which walks the tree itself and writes nothing when
 * nothing moved — so the hook never pays a walk to decide whether one is owed. Dense, as every
 * refresh nobody waits on is: the background band keeps the embed out of the way.
 */
function startUpdate(root) {
  return detached(root, ['--repo', root, 'update', '--detach']);
}

/**
 * A checkout with no index of its own — a new worktree, typically — gets one built in the
 * background from its first session, where a git hook used to start one on every `git worktree add`.
 */
function bootstrap(cwd) {
  const checkout = cwd && checkoutRoot(cwd);
  // A spawn reports a missing binary only later, as an event, so the notice would claim a build
  // that never started.
  if (!checkout || !optedIn(checkout) || !binaryFound(checkout)) return null;
  return startUpdate(checkout) ? BUILDING_NOTICE : null;
}

const handlers = {
  SessionStart: (p, root) => {
    let text;
    if (!built(root)) text = bootstrap(p.cwd || root);
    else {
      // The resident serve catches the store up as it starts; without one, a detached update does.
      if (!autostartServe(root)) startUpdate(root);
      text = brief(root);
    }
    const notice = sessionNotice(p, root);
    if (!notice) return text;
    return { additionalContext: [text, notice.brief].filter(Boolean).join('\n'), systemMessage: notice.message };
  },
  SubagentStart: (p, root) => (built(root) ? rule() : null),
  PreToolUse: (p, root, key) => {
    if (p.tool_name === 'Agent') {
      if (env('HOOK_AGENT_INPUT') !== '1') return null;
      const prompt = String((p.tool_input || {}).prompt || '');
      return { updatedInput: { ...(p.tool_input || {}), prompt: `${prompt}\n\n${rule()}` } };
    }
    if (!built(root)) {
      if (!isWatchedCall(p.tool_name, p.tool_input) || !once(key, 'build', 'once')) return null;
      // A machine-wide plugin says nothing in a repository that never asked for an index.
      const checkout = p.cwd && checkoutRoot(p.cwd);
      if (!checkout || !optedIn(checkout)) return null;
      // SessionStart already started the build there; a `build` on top of it would embed twice.
      return binaryFound(checkout) ? BUILDING_NOTICE : BUILD_NOTICE;
    }
    return intercept(p, root, key) ?? reminder(p, key);
  },
  PostToolUse: (p, root, key) => (built(root) ? riskLine(p, root, key) : null),
};

const LEGACY_HOOK = 'zerograph-hook.mjs';

/**
 * What a group's `matcher` is tested against for `event`, as both harnesses do: the tool for the
 * tool events, how the session began for `SessionStart`, the subagent's type for `SubagentStart`.
 */
function matchSubject(event, payload) {
  if (event === 'PreToolUse' || event === 'PostToolUse') return payload.tool_name ?? '';
  if (event === 'SessionStart') return payload.source ?? '';
  if (event === 'SubagentStart') return payload.agent_type ?? '';
  return null;
}

/** A matcher absent, empty or `*` matches everything; any other is a regex over the whole subject. */
function matches(matcher, subject) {
  if (subject === null || matcher === undefined || matcher === null || matcher === '' || matcher === '*') return true;
  try { return new RegExp(`^(?:${matcher})$`).test(subject); } catch { return matcher === subject; }
}

/** The commands a harness hooks file runs for this payload's event, or none when the file is absent or unreadable. */
function hookCommands(file, event, payload) {
  let parsed;
  try { parsed = JSON.parse(readFileSync(file, 'utf8')); } catch { return []; }
  const groups = parsed?.hooks?.[event];
  if (!Array.isArray(groups)) return [];
  const subject = matchSubject(event, payload);
  return groups.filter((g) => matches(g?.matcher, subject))
    .flatMap((g) => (Array.isArray(g?.hooks) ? g.hooks : []))
    .map((h) => [h?.command, ...(Array.isArray(h?.args) ? h.args : [])].filter((w) => typeof w === 'string').join(' '));
}

/** `~`, `$HOME` and `${HOME}` as the shell running a hook command would expand them. */
function expandHome(path) {
  return path.replace(/^(?:~|\$\{HOME\}|\$HOME)(?=\/|\\|$)/, homedir());
}

/**
 * Whether a hook an earlier `install-agent` wired answers this event already, so this one stays
 * silent rather than put the brief, the seeds and the risk line in context twice. In Claude Code
 * that is a repository's committed `.claude/settings.json`, which `install-agent` leaves for the
 * team to retire; in Codex it is the machine's `$CODEX_HOME/hooks.json`, which nothing a
 * repository ships can touch. Either way the old script has to be there too: an entry whose
 * script is gone answers nothing. The harness is named by the plugin's own hook command, since
 * Codex sets `CLAUDE_PLUGIN_ROOT` as well and no variable tells the two apart.
 */
function legacyAnswers(event, payload, root, harness) {
  if (harness === 'codex') {
    const home = process.env.CODEX_HOME || join(homedir(), '.codex');
    return hookCommands(join(home, 'hooks.json'), event, payload).some((cmd) => {
      const script = cmd.match(/(?:"([^"]*zerograph-hook\.mjs)"|'([^']*zerograph-hook\.mjs)'|(\S*zerograph-hook\.mjs))/);
      const path = script && expandHome(script[1] ?? script[2] ?? script[3]);
      return Boolean(path) && isAbsolute(path) && existsSync(path);
    });
  }
  // Only the project Claude Code read its settings from: the old entries run the script under
  // `$CLAUDE_PROJECT_DIR`, so a main worktree's copy, or a nested clone's the session cd'd into,
  // answers nothing here.
  const project = process.env.CLAUDE_PROJECT_DIR || root;
  return existsSync(join(project, '.claude', 'hooks', LEGACY_HOOK))
    && hookCommands(join(project, '.claude', 'settings.json'), event, payload).some((cmd) => cmd.includes(LEGACY_HOOK));
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

async function main({ harness = process.argv.includes('--codex') ? 'codex' : 'claude' } = {}) {
  let payload;
  try { payload = JSON.parse(await readStdin()); } catch { return; }
  const event = payload.hook_event_name;
  const handler = handlers[event];
  if (!handler) return;
  // The harness's cwd follows a `cd` into a subdirectory, which has no `.zerograph/` of its own.
  const root = (payload.cwd && indexRoot(payload.cwd)) || payload.cwd || process.env.CLAUDE_PROJECT_DIR || process.cwd();
  let answer = handler;
  if (legacyAnswers(event, payload, root, harness)) {
    debug('legacy hook answers', event);
    if (event !== 'SessionStart') return;
    // The old hook has no notice of either kind, so standing down keeps those and drops the rest.
    answer = (p, r) => {
      const notice = sessionNotice(p, r);
      return notice && { additionalContext: notice.brief, systemMessage: notice.message };
    };
  }
  // The subagent's own id is in the key: a subagent shares its parent's session_id, so a counter
  // keyed by session alone finds the count already past zero on a subagent's first call and says
  // nothing — which is a second reason 140 unprimed subagents went on grepping.
  const sessionKey = `${payload.session_id || 'nosession'}-${payload.agent_id || 'main'}`;
  let out;
  try { out = answer(payload, root, sessionKey); } catch (e) { debug('handler', e); return; }
  if (!out) return;
  // `systemMessage` is a top-level field Claude Code shows the person; Codex is given only context.
  const { systemMessage, ...rest } = typeof out === 'string' ? { additionalContext: out } : out;
  const shown = harness === 'claude' && systemMessage ? { systemMessage } : {};
  // Written without process.exit: on Windows the pipe may still be draining when exit would cut it.
  process.stdout.write(JSON.stringify({ ...shown, hookSpecificOutput: { hookEventName: event, ...rest } }));
}

// Importable for the tests, which exercise the pure parts directly; only the real run reads stdin.
// `import.meta.url` is the resolved path and argv[1] keeps a symlink in the project path, so both are
// resolved: compared as given, a project reached through a link ran the hook and it said nothing.
function invokedDirectly() {
  try { return Boolean(process.argv[1]) && fileURLToPath(import.meta.url) === realpathSync(process.argv[1]); } catch { return false; }
}
if (invokedDirectly()) {
  main();
}

// `main` for Codex on Windows, whose hook command imports this file rather than naming its path in
// a shell variable.
export { main, rule, RULE, COMMAND, binary, onPath, bounded, intercept, riskLine, handlers, MAX_INJECTIONS };
