#!/bin/sh
# Copies the Claude Code plugin's launcher scripts into the Codex plugin.
#
# Each plugin is installed from its own directory, and neither agent's installer is known to carry a
# symlink out of it (a Windows checkout without core.symlinks turns one into a text file), so the
# Codex plugin ships copies. agent/codex/plugin.test.mjs fails while they differ; run this after
# editing anything in agent/claude-code/scripts.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
src="$repo/agent/claude-code/scripts"
dst="$repo/agent/codex/scripts"
for f in "$src"/*.mjs; do
  case "$f" in *.test.mjs) continue ;; esac
  cp "$f" "$dst/"
done
echo "synced $(ls "$src"/*.mjs | grep -vc '\.test\.mjs$') scripts into agent/codex/scripts"
