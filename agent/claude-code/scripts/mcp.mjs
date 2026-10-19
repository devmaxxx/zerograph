#!/usr/bin/env node
// The plugin's MCP server: `zerograph --repo <checkout> mcp`, with the binary found by
// `launcher.mjs`, as the session hook finds it. A manifest can substitute the `binary` option but
// cannot branch on it, so the choice between the option and PATH is made here.
//
// Usage: node mcp.mjs '<binary option>'
import { spawn } from 'node:child_process';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { binaryOf, checkoutOf, childEnv, commandOf, pathKey, real } from './launcher.mjs';

export { binaryOf, checkoutOf, childEnv };

/** The argv that starts the server for `checkout`, or why it cannot be started safely. */
export function argvOf(bin, checkout, comspec) {
  return commandOf(bin, ['--repo', checkout, 'mcp'], comspec);
}

export function main() {
  const win = process.platform === 'win32';
  const env = process.env;
  // Claude Code names the project in the server's environment; Codex starts the server in the
  // project and names nothing, and neither documents the working directory for the other.
  const checkout = checkoutOf(env.CLAUDE_PROJECT_DIR || process.cwd());
  // The option comes as the one argument under Claude Code; Codex has no options and passes
  // ZEROGRAPH_BIN through instead.
  const found = binaryOf(process.argv[2] || env.ZEROGRAPH_BIN || '', { path: env[pathKey(env)], checkout, win });
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

// Imported by its tests and by Codex's bootstrap, run by the Claude Code manifest; a plugin cache
// reached through a link names this file by another spelling than the one Node resolved.
if (process.argv[1] && real(fileURLToPath(import.meta.url)) === real(process.argv[1])) main();
