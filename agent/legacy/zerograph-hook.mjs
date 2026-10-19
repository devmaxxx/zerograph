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
 * ZEROGRAPH_HOOK_DEBUG. Every call is --no-dense; every call but `changes` is --stale.
 */
import { existsSync, readFileSync, writeFileSync, mkdirSync, realpathSync, lstatSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { join, dirname, relative, delimiter, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';

const MAX_INJECTIONS = 12;   // 12 × ~107 tokens: a third of the CLAUDE.md sections this replaces
const SEEDS = 3;
const LINE_CUT = 160;
const REMINDER_EVERY = 40;
const TIMEOUT_MS = 5000;
/** What the autostarted `serve` is told: leave after half an hour unasked, forget the model after five minutes. */
const SERVE_IDLE = 1800;
const SERVE_IDLE_MODEL = 300;   // measured on the 33.5k-row copy: 0.08 s resident, 0.77 s after a drop

/**
 * How this repository runs the binary, in every hint the hook prints. `install-agent --command`
 * rewrites this one line with the command JSON-encoded, so nothing in it reaches the script as code.
 * A workspace that installs zerograph as a dependency has no bare `zerograph` on PATH, and a hint
 * naming a program the shell cannot find is one the agent runs, fails, and pays for twice.
 */
const COMMAND = "zerograph";

const HERE = dirname(fileURLToPath(import.meta.url));

/**
 * The rule handed to every subagent. Read from `rule.txt` beside this file when the installer put
 * one there, and otherwise the copy below — the two are compared by a test, so a hook installed
 * without its text still says the same thing.
 */
const RULE_FALLBACK = `This repository has a zerograph index. Ask it before grepping for a concept: \`${COMMAND} ask <words>\`
answers by meaning, \`${COMMAND} impact <Symbol>\` names who calls a symbol, \`${COMMAND} changes\` maps
your diff onto the callers it reaches. Read only the \`path:line\` those answers print, and cite one
for every claim; when nothing printed holds it, say so.
Grep is still the right tool for a literal — an env var name, a string in a test, a config key.
`;

const BUILD_NOTICE =
  `zerograph: this repository has no index yet. \`${COMMAND} build\` writes one in a couple of minutes ` +
  'and answers by meaning afterwards; until then this notice is all it can say.';

function rule() {
  const beside = join(HERE, 'rule.txt');
  try { return existsSync(beside) ? readFileSync(beside, 'utf8') : RULE_FALLBACK; } catch { return RULE_FALLBACK; }
}

// The name a setting had before the rename is read while the new one is unset, so a shell profile
// written then keeps working.
function env(suffix) {
  return process.env[`ZEROGRAPH_${suffix}`] ?? process.env[`REPOGRAPH_${suffix}`];
}

function debug(...args) {
  if (env('HOOK_DEBUG')) console.error('[zerograph-hook]', ...args);
}

/**
 * Whether a `zerograph` the machine's own PATH resolves exists, so that it can win over the
 * repository's. A relative entry (`.`, `node_modules/.bin`) resolves against the repository the
 * hook runs in, so it is the repository's file and not the machine's.
 */
function onPath() {
  const names = process.platform === 'win32' ? ['zerograph.exe', 'zerograph.cmd'] : ['zerograph'];
  return (process.env.PATH || '').split(delimiter).some((d) => d && isAbsolute(d) && names.some((n) => existsSync(join(d, n))));
}

/**
 * `ZEROGRAPH_BIN`, then PATH, then the package's own bin, and nothing else: a hook that installed
 * things would not be a hook. The repository's `node_modules/.bin` comes after PATH because it is
 * the repository's own file, and a cloned repository that ships one would otherwise run its
 * binary the moment a session starts in it. npm, pnpm and yarn all write the same trio under `node_modules/.bin` — an
 * extensionless shell script, a `.cmd` and a `.ps1` — and on Windows the `.cmd` is the one that
 * starts. The extensionless shim is worse than nothing there: returning it spawns a file
 * `CreateProcess` cannot read, where falling through finds whatever `zerograph.exe` an installer
 * put on PATH.
 */
function binary(root) {
  const named = env('BIN');
  if (named) return named;
  if (onPath()) return 'zerograph';
  const local = join(root, 'node_modules', '.bin', 'zerograph');
  const shim = process.platform === 'win32' ? `${local}.cmd` : local;
  return existsSync(shim) ? shim : 'zerograph';
}

/**
 * How to hand one command to `spawn`. A `.cmd` or `.bat` is read by the command interpreter rather
 * than by `CreateProcess`, and Node has refused to spawn one without a shell since CVE-2024-27980.
 * The shell joins the file and its arguments into one command line without quoting them, so they
 * are quoted here — and nothing that reaches this can close a quote: a Windows path may not contain
 * one, and the words come from `queryWords`, which keeps letters, digits, `_` and `-` and nothing
 * else. Everything that is not a batch file is spawned as it always was.
 *
 * Two things the quoting does not buy, both only on that batch path. `cmd` expands `%NAME%` inside
 * quotes and the command line has no escape for it — only a batch file's own `%%` — so a repository
 * under a directory that pairs two `%` around an environment variable's name is asked about by the
 * expanded path. And `spawnSync`'s timeout kills the interpreter rather than its child, so a wedged
 * `zerograph` outlives the five seconds this hook waits. Both need a local install on Windows to
 * reach at all; neither is worth the shell it would take to avoid, and neither is silent about it.
 */
function spawnable(bin, args) {
  // Windows and a batch file, both: the extension alone would open the shell on a POSIX machine
  // whose `ZEROGRAPH_BIN` happens to end in `.cmd`, where a repository path may contain a quote
  // and the sentence above stops being true.
  if (process.platform !== 'win32' || !/\.(cmd|bat)$/i.test(bin)) return { file: bin, args, opts: {} };
  return { file: `"${bin}"`, args: args.map((a) => `"${a}"`), opts: { shell: true } };
}

function run(root, args) {
  const { file, args: argv, opts } = spawnable(binary(root), ['--repo', root, '--no-dense', ...args]);
  const r = spawnSync(file, argv, {
    encoding: 'utf8', timeout: TIMEOUT_MS, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true, ...opts,
  });
  if (r.status !== 0) debug('exit', r.status, args.join(' '), (r.stderr || '').slice(0, 200));
  // Identifiers and headings in the answer are the repository's text, and it is about to enter an
  // agent's context and possibly a terminal log; control characters other than tab and newline carry nothing.
  return r.status === 0 ? r.stdout.replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, '') : '';
}

// The directory the binary reads: `.zerograph` whenever anything stands at that name, else a
// `.repograph` no write has moved yet. A manifest under the other name is no index it answers from.
function built(root) {
  const standing = (p) => { try { lstatSync(p); return true; } catch { return false; } };
  const dir = standing(join(root, '.zerograph')) ? '.zerograph' : '.repograph';
  return existsSync(join(root, dir, 'manifest.json'));
}

/**
 * The nearest directory at or above `start` that carries an index. The harness's `cwd` follows a
 * `cd`, and a subdirectory has no `.zerograph/` of its own; the walk stops at a `.git`, file or
 * directory, so a worktree nested inside a checkout never borrows the checkout's index.
 */
function indexRoot(start) {
  for (let d = start; ; d = dirname(d)) {
    if (built(d)) return d;
    if (existsSync(join(d, '.git')) || dirname(d) === d) return null;
  }
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
  if (payload.tool_name === 'Bash' && /\b(?:zerograph|repograph)\b/.test(String(input.command || ''))) return null;
  if (/\.(?:zerograph|repograph)\//.test(JSON.stringify(input))) return null;
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

function riskLine(payload, root, sessionKey) {
  const file = String((payload.tool_input || {}).file_path || '');
  if (!/\.tsx?$/.test(file)) return null;
  let r;
  try { r = JSON.parse(run(root, ['changes', '--depth', '1', '--json'])); } catch { return null; }
  if (!r || !['MEDIUM', 'HIGH', 'CRITICAL'].includes(r.risk)) return null;
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
  if (env('HOOK_SERVE') === '0') return;
  try {
    const { file, args, opts } = spawnable(binary(root),
      ['--repo', root, 'serve', '--idle', String(SERVE_IDLE), '--idle-model', String(SERVE_IDLE_MODEL)]);
    const child = spawn(file, args, { detached: true, stdio: 'ignore', windowsHide: true, ...opts });
    // A spawn that fails reports it as an event, not as a throw, and an unheard `error` event ends
    // this process — which would turn "no binary on PATH" into a hook that dies on stderr at every
    // session start. The listener is what keeps the contract: exit 0, and say nothing.
    child.on('error', (e) => debug('serve', e.message));
    child.unref();
  } catch (e) { debug('serve', e.message); }
}

const handlers = {
  SessionStart: (p, root) => {
    if (!built(root)) return null;
    autostartServe(root);
    return brief(root);
  },
  SubagentStart: (p, root) => (built(root) ? rule() : null),
  PreToolUse: (p, root, key) => {
    if (p.tool_name === 'Agent') {
      if (env('HOOK_AGENT_INPUT') !== '1') return null;
      const prompt = String((p.tool_input || {}).prompt || '');
      return { updatedInput: { ...(p.tool_input || {}), prompt: `${prompt}\n\n${rule()}` } };
    }
    if (!built(root)) {
      return isWatchedCall(p.tool_name, p.tool_input) && once(key, 'build', 'once') ? BUILD_NOTICE : null;
    }
    return intercept(p, root, key) ?? reminder(p, key);
  },
  PostToolUse: (p, root, key) => (built(root) ? riskLine(p, root, key) : null),
};

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

async function main() {
  let payload;
  try { payload = JSON.parse(await readStdin()); } catch { return; }
  const event = payload.hook_event_name;
  const handler = handlers[event];
  if (!handler) return;
  // The harness's cwd follows a `cd` into a subdirectory, which has no `.zerograph/` of its own.
  const root = (payload.cwd && indexRoot(payload.cwd)) || payload.cwd || process.env.CLAUDE_PROJECT_DIR || process.cwd();
  // The subagent's own id is in the key: a subagent shares its parent's session_id, so a counter
  // keyed by session alone finds the count already past zero on a subagent's first call and says
  // nothing — which is a second reason 140 unprimed subagents went on grepping.
  const sessionKey = `${payload.session_id || 'nosession'}-${payload.agent_id || 'main'}`;
  let out;
  try { out = handler(payload, root, sessionKey); } catch (e) { debug('handler', e); return; }
  if (!out) return;
  const body = typeof out === 'string'
    ? { hookEventName: event, additionalContext: out }
    : { hookEventName: event, ...out };
  // Written without process.exit: on Windows the pipe may still be draining when exit would cut it.
  process.stdout.write(JSON.stringify({ hookSpecificOutput: body }));
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

export { rule, RULE_FALLBACK, COMMAND, binary, intercept, riskLine, handlers, MAX_INJECTIONS };
