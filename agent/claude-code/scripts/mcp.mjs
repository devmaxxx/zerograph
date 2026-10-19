#!/usr/bin/env node
// The plugin's MCP server: `zerograph --repo <checkout> mcp`, with the binary found the way the
// status mod finds it. `.mcp.json` can substitute the `binary` option but cannot branch on it, so
// the choice between the option and PATH is made here.
//
// Usage: node mcp.mjs '<binary option>'
import { spawn } from 'node:child_process';
import { existsSync, realpathSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const UNSET = '${user_config.binary}';

/** Whether `path` is absolute in either platform's spelling. */
export const isAbsolute = path => path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');

/** Whether `dir` is `root` or below it; Windows paths name one file in any case. */
export function inside(root, dir, win) {
  const norm = p => {
    const slashed = p.replace(/\\/g, '/').replace(/\/+$/, '');
    return win ? slashed.toLowerCase() : slashed;
  };
  const r = norm(root);
  const d = norm(dir);
  return d === r || d.startsWith(`${r}/`);
}

/** `p` with every link followed, or as given when it does not resolve. */
export function real(p) {
  try { return realpathSync.native(p); } catch { return p; }
}

/**
 * The nearest directory at or above `start` holding `name`, or null. Walked from the real path, as
 * the CLI canonicalizes its working directory, so a session opened through a link finds the
 * checkout the CLI would.
 */
function nearest(start, name) {
  for (let d = real(start); ; d = dirname(d)) {
    if (existsSync(join(d, name))) return d;
    if (dirname(d) === d) return null;
  }
}

/**
 * The repository around `root`, found the way the CLI finds it: the checkout (a session opened in
 * a package of a monorepo still has the whole checkout around it), else the nearest directory
 * above holding a `.zerograph/`, else `root` itself. It is what zerograph indexes and what a PATH
 * entry must stay out of.
 */
export function checkoutOf(root) {
  return nearest(root, '.git') ?? nearest(root, '.zerograph') ?? root;
}

/**
 * PATH's absolute entries outside `checkout`, compared as spelled and with every link followed, so
 * neither a symlinked alias of the checkout on PATH nor a session opened through one lets the
 * checkout's file through.
 */
export function machineDirs(path, checkout, win, home = homedir()) {
  const cut = guarded(checkout, win, home);
  const resolved = real(checkout);
  return (path ?? '').split(win ? ';' : ':').filter(entry => {
    // Windows reads a quoted entry as the directory inside the quotes.
    const dir = win ? entry.replace(/^"(.*)"$/, '$1') : entry;
    return dir && isAbsolute(dir) && !(cut && (inside(checkout, dir, win) || inside(resolved, real(dir), win)));
  });
}

/**
 * Whether PATH entries inside `checkout` are cut. Not when it is a filesystem root or holds the home
 * directory: a `~/.zerograph` or a dotfiles `~/.git` makes the home directory the checkout, and the
 * cut would then take nvm, cargo and `~/.local/bin` with it, the very places zerograph and `node`
 * are installed.
 */
function guarded(checkout, win, home) {
  if (!checkout || dirname(checkout) === checkout) return false;
  return !(home && (inside(checkout, home, win) || inside(real(checkout), real(home), win)));
}

/** The PATH variable's name: any case on Windows, where the environment ignores it, else `PATH`. */
export function pathKey(env, win) {
  return (win && Object.keys(env).find(k => k.toUpperCase() === 'PATH')) || 'PATH';
}

/**
 * The configured binary when it is an absolute path, else PATH's by its absolute path, and never
 * one under the checkout: a cloned repository that ships `node_modules/.bin/zerograph` would
 * otherwise have it run the moment a session opened in it. A relative option is refused rather
 * than read against a working directory nobody chose; a bare name means the PATH lookup.
 */
export function binaryOf(configured, { path, checkout, win, home }) {
  const option = configured === UNSET ? '' : configured.trim();
  if (option && /[\\/]/.test(option)) {
    return isAbsolute(option) ? { bin: option } : { error: `the plugin's binary option (${option}) is not an absolute path; set one in /config, or clear it to use PATH` };
  }
  const dirs = machineDirs(path, checkout, win, home);
  // Every `.exe` before any `.cmd`: npm's global shim is a `.cmd`, which only `cmd` can start.
  const names = win ? ['zerograph.exe', 'zerograph.cmd'] : ['zerograph'];
  for (const name of names) {
    for (const dir of dirs) {
      const file = join(dir, name);
      const stat = statSync(file, { throwIfNoEntry: false });
      if (stat?.isFile() && (win || (stat.mode & 0o111))) return { bin: file };
    }
  }
  return { error: 'no zerograph on PATH outside this checkout; install it (npm i -g zerograph) or name its absolute path in the plugin\'s binary option' };
}

// What `cmd` acts on outside quotes (`&|<>^` run or redirect, `"` toggles quoting) or expands
// even inside them (`%`, and `!` where delayed expansion is on).
const CMD_SPECIAL = /[&|<>^"%!]/;

/**
 * The argv that starts `bin`, or why it cannot be started safely. Node has refused to start a
 * batch file without a shell since CVE-2024-27980, so a `.cmd` goes through `cmd` by hand; `call`
 * keeps the command line from opening on a quote, which `cmd /c` would strip along with the last
 * one. `cmd` parses the line Node builds, and Node quotes an argument only when it holds a space
 * or a quote, so a checkout like `C:\src\a&calc` would reach it bare and its `&` start a second
 * command; a quote added here is escaped as `\"`, which `cmd` reads as one that closes. A path
 * `cmd` would act on is therefore refused.
 */
export function argvOf(bin, checkout, comspec) {
  const args = ['--repo', checkout, 'mcp'];
  if (!/\.(cmd|bat)$/i.test(bin)) return { argv: [bin, ...args] };
  const unsafe = [bin, checkout].find(a => CMD_SPECIAL.test(a));
  if (unsafe) {
    return { error: `${unsafe} holds a character cmd would act on (& | < > ^ " % !), so the npm shim ${bin} is not started through it; name the zerograph.exe the npm package ships in the plugin's binary option` };
  }
  return { argv: [comspec || 'C:\\Windows\\System32\\cmd.exe', '/d', '/c', 'call', bin, ...args] };
}

/** The environment the server runs in: PATH cut to the machine's entries, as the binary was. */
export function childEnv(env, checkout, win, home) {
  const key = pathKey(env, win);
  return { ...env, [key]: machineDirs(env[key], checkout, win, home).join(win ? ';' : ':') };
}

function main() {
  const win = process.platform === 'win32';
  // Claude Code names the project in the server's environment; the working directory it starts
  // a plugin's server in is not documented.
  const checkout = checkoutOf(process.env.CLAUDE_PROJECT_DIR || process.cwd());
  const env = process.env;
  const found = binaryOf(process.argv[2] ?? '', { path: env[pathKey(env, win)], checkout, win });
  const launch = found.error ? found : argvOf(found.bin, checkout, env.ComSpec);
  if (launch.error) {
    // stderr is the server log the harness shows; stdout belongs to the protocol.
    process.stderr.write(`zerograph: ${launch.error}\n`);
    process.exit(1);
  }
  const [cmd, ...args] = launch.argv;
  // Never from the checkout: npm's `.cmd` shim starts a bare `node`, and `cmd` looks for that in
  // the working directory before PATH. The cut PATH keeps a checkout's `node_modules/.bin/node`
  // from being the one the shim, or anything the server starts, finds.
  const child = spawn(cmd, args, { stdio: 'inherit', cwd: dirname(found.bin), windowsHide: true, env: childEnv(env, checkout, win) });
  child.on('error', err => { process.stderr.write(`zerograph: ${found.bin}: ${err.message}\n`); process.exit(1); });
  child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
  for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(sig, () => child.kill(sig));
}

// Imported by its tests, run by `.claude-plugin/plugin.json`; a plugin cache reached through a
// link names this file by another spelling than the one Node resolved.
if (process.argv[1] && real(fileURLToPath(import.meta.url)) === real(process.argv[1])) main();
