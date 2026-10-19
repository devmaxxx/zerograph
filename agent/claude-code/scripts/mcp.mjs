#!/usr/bin/env node
// The plugin's MCP server: `zerograph --repo <checkout> mcp`, with the binary found the way the
// status mod finds it. `.mcp.json` can substitute the `binary` option but cannot branch on it, so
// the choice between the option and PATH is made here.
//
// Usage: node mcp.mjs '<binary option>'
import { spawn } from 'node:child_process';
import { existsSync, realpathSync, statSync } from 'node:fs';
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

/**
 * The checkout around `root`: a session opened in a package of a monorepo still has the whole
 * checkout around it, and that is what zerograph indexes and what a PATH entry must stay out of.
 */
export function checkoutOf(root) {
  for (let d = root; ;) {
    if (existsSync(join(d, '.git'))) return d;
    const up = dirname(d);
    if (up === d) return root;
    d = up;
  }
}

/**
 * The configured binary when it is an absolute path, else PATH's by its absolute path, and never
 * one under the checkout: a cloned repository that ships `node_modules/.bin/zerograph` would
 * otherwise have it run the moment a session opened in it. A relative option is refused rather
 * than read against a working directory nobody chose; a bare name means the PATH lookup.
 */
export function binaryOf(configured, { path, checkout, win }) {
  const option = configured === UNSET ? '' : configured.trim();
  if (option && /[\\/]/.test(option)) {
    return isAbsolute(option) ? { bin: option } : { error: `the plugin's binary option (${option}) is not an absolute path; set one in /config, or clear it to use PATH` };
  }
  const dirs = (path ?? '').split(win ? ';' : ':').filter(dir => dir && isAbsolute(dir) && !inside(checkout, dir, win));
  // Every `.exe` before any `.cmd`: npm's global shim is a `.cmd`, which only `cmd` can start.
  const names = win ? ['zerograph.exe', 'zerograph.cmd'] : ['zerograph'];
  for (const name of names) {
    for (const dir of dirs) {
      const file = join(dir, name);
      if (statSync(file, { throwIfNoEntry: false })?.isFile()) return { bin: file };
    }
  }
  return { error: 'no zerograph on PATH outside this checkout; install it (npm i -g zerograph) or name its absolute path in the plugin\'s binary option' };
}

/**
 * The argv that starts `bin`. Node has refused to start a batch file without a shell since
 * CVE-2024-27980, so a `.cmd` goes through `cmd` by hand; `call` keeps the command line from
 * opening on a quote, which `cmd /c` would strip along with the last one.
 */
export function argvOf(bin, checkout, comspec) {
  const head = /\.(cmd|bat)$/i.test(bin) ? [comspec || 'C:\\Windows\\System32\\cmd.exe', '/d', '/c', 'call', bin] : [bin];
  return [...head, '--repo', checkout, 'mcp'];
}

function main() {
  const win = process.platform === 'win32';
  // Claude Code names the project in the server's environment; the working directory it starts
  // a plugin's server in is not documented.
  const checkout = checkoutOf(process.env.CLAUDE_PROJECT_DIR || process.cwd());
  const found = binaryOf(process.argv[2] ?? '', { path: process.env.PATH ?? process.env.Path, checkout, win });
  if (found.error) {
    // stderr is the server log the harness shows; stdout belongs to the protocol.
    process.stderr.write(`zerograph: ${found.error}\n`);
    process.exit(1);
  }
  const [cmd, ...args] = argvOf(found.bin, checkout, process.env.ComSpec);
  // Never from the checkout: npm's `.cmd` shim starts a bare `node`, and `cmd` looks for that in
  // the working directory before PATH.
  const child = spawn(cmd, args, { stdio: 'inherit', cwd: dirname(found.bin), windowsHide: true });
  child.on('error', err => { process.stderr.write(`zerograph: ${found.bin}: ${err.message}\n`); process.exit(1); });
  child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
  for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(sig, () => child.kill(sig));
}

// Imported by its tests, run by `.claude-plugin/plugin.json`; a plugin cache reached through a
// link names this file by another spelling than the one Node resolved.
const real = p => { try { return realpathSync(p); } catch { return p; } };
if (process.argv[1] && real(fileURLToPath(import.meta.url)) === real(process.argv[1])) main();
