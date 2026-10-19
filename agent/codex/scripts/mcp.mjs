#!/usr/bin/env node
// The plugin's MCP server: `zerograph --repo <checkout> mcp`, with the binary found by
// `launcher.mjs`, as the session hook finds it. A manifest can substitute the `binary` option but
// cannot branch on it, so the choice between the option and PATH is made here.
//
// Usage: node mcp.mjs '<binary option>'
import { spawn, spawnSync } from 'node:child_process';
import { statSync } from 'node:fs';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { binaryOf, checkoutOf, childEnv, commandOf, pathKey, real } from './launcher.mjs';

export { binaryOf, checkoutOf, childEnv };

/** The argv that starts the server for `checkout`, or why it cannot be started safely. */
export function argvOf(bin, checkout, comspec) {
  return commandOf(bin, ['--repo', checkout, 'mcp'], comspec);
}

/** The release `bin` reports, or null when it cannot say. */
function versionOf(bin, comspec) {
  const c = commandOf(bin, ['--version'], comspec);
  if (!c.argv) return null;
  const r = spawnSync(c.argv[0], c.argv.slice(1), { stdio: ['ignore', 'pipe', 'ignore'], encoding: 'utf8', timeout: 5000, windowsHide: true });
  return r.stdout?.trim().match(/^zerograph (\S+)$/)?.[1] ?? null;
}

/**
 * A line naming the release now installed at `bin` when it is not the one this server started, or
 * null. A session keeps its server for its whole life and a plugin cannot restart it, so after an
 * upgrade the old binary goes on writing a store the new one owns; all that can be done is to say
 * so. The binary is asked again only when the file it resolves to has moved, which an install over
 * it always does, so a tool call pays one stat.
 */
function driftCheck(bin, comspec) {
  const where = () => { try { const r = real(bin); const st = statSync(r); return `${r}:${st.ino}:${st.mtimeMs}:${st.size}`; } catch { return null; } };
  const serving = versionOf(bin, comspec);
  let seen = where();
  let line = null;
  return () => {
    const now = where();
    if (serving && now !== seen) {
      seen = now;
      const installed = versionOf(bin, comspec);
      line = installed && installed !== serving
        ? `zerograph was updated to ${installed} (this session runs ${serving}) — restart the session or run /reload-plugins`
        : null;
    }
    return line;
  };
}

/** One protocol line from the server, with `line` put first in a tool result's content. */
function withLine(raw, line) {
  if (!line || !raw.includes('"content"')) return raw;
  try {
    const msg = JSON.parse(raw);
    if (!Array.isArray(msg?.result?.content)) return raw;
    msg.result.content.unshift({ type: 'text', text: line });
    return JSON.stringify(msg);
  } catch { return raw; }
}

export function main() {
  const win = process.platform === 'win32';
  const env = process.env;
  // Claude Code names the project in the server's environment; Codex starts the server in the
  // project and names nothing, and neither documents the working directory for the other.
  const checkout = checkoutOf(env.CLAUDE_PROJECT_DIR || process.cwd());
  // ZEROGRAPH_BIN before the option, in the hook's order, so the two never run different binaries.
  // Claude Code passes the option as the one argument, unsubstituted while it is unset, which
  // `binaryOf` reads as empty; Codex has no options and passes ZEROGRAPH_BIN through.
  const found = binaryOf(env.ZEROGRAPH_BIN || process.argv[2] || '', { path: env[pathKey(env, win)], checkout, win });
  const launch = found.error ? found : argvOf(found.bin, checkout, env.ComSpec);
  if (launch.error) {
    // stderr is the server log the harness shows; stdout belongs to the protocol.
    process.stderr.write(`zerograph: ${launch.error}\n`);
    process.exit(1);
  }
  const drift = driftCheck(found.bin, env.ComSpec);
  const [cmd, ...args] = launch.argv;
  // Never from the checkout: npm's `.cmd` shim starts a bare `node`, and `cmd` looks for that in
  // the working directory before PATH. The cut PATH keeps a checkout's `node_modules/.bin/node`
  // from being the one the shim, or anything the server starts, finds.
  // stdout is read line by line, the stdio transport's framing, so a tool result can carry the line.
  const child = spawn(cmd, args, { stdio: ['inherit', 'pipe', 'inherit'], cwd: dirname(found.bin), windowsHide: true, env: childEnv(env, checkout, win) });
  let pending = '';
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    pending += chunk;
    const lines = pending.split('\n');
    pending = lines.pop();
    const line = lines.length ? drift() : null;
    if (lines.length) process.stdout.write(lines.map((l) => withLine(l, line)).join('\n') + '\n');
  });
  child.stdout.on('end', () => { if (pending) process.stdout.write(pending); });
  child.on('error', err => { process.stderr.write(`zerograph: ${found.bin}: ${err.message}\n`); process.exit(1); });
  // `close`, not `exit`, and no process.exit: the last result may still be on its way to stdout.
  child.on('close', (code, signal) => { process.exitCode = code ?? (signal ? 1 : 0); });
  for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(sig, () => child.kill(sig));
}

// Imported by its tests and by Codex's bootstrap, run by the Claude Code manifest; a plugin cache
// reached through a link names this file by another spelling than the one Node resolved.
if (process.argv[1] && real(fileURLToPath(import.meta.url)) === real(process.argv[1])) main();
