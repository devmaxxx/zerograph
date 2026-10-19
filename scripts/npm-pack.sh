#!/usr/bin/env bash
# Assembles the npm package from prebuilt binaries and packs it into dist/npm/.
# Publishing stays a separate, deliberate step: a version number, once published, is spent —
# the registry never takes a second upload of it, even after the first is deleted.
#
#   scripts/npm-pack.sh [version] [darwin-arm64=<binary>] [linux-x64=<binary>] [win32-x64=<binary.exe>]
#
# The version defaults to Cargo.toml's. On an Apple-silicon Mac, darwin-arm64 defaults to
# target/release/zerograph; any platform not given is left out of this pack run, and the launcher
# says so on that platform.
# Plain variables rather than an associative array: macOS still ships bash 3.2.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
darwin_arm64=""
linux_x64=""
win32_x64=""
if [ "$(uname -s)-$(uname -m)" = "Darwin-arm64" ] && [ -x target/release/zerograph ]; then
  darwin_arm64=target/release/zerograph
fi
for arg in "$@"; do
  case "$arg" in
    darwin-arm64=*) darwin_arm64=${arg#*=} ;;
    linux-x64=*) linux_x64=${arg#*=} ;;
    win32-x64=*) win32_x64=${arg#*=} ;;
    *=*) echo "unknown platform in $arg" >&2; exit 2 ;;
    *) version=$arg ;;
  esac
done

out=dist/npm
rm -rf "$out" && mkdir -p "$out"

bin=npm/zerograph/bin
# What an earlier run left must not ride along under this version.
for platform in darwin-arm64 linux-x64 win32-x64; do rm -rf "${bin:?}/$platform"; done

# A file, not an executable: the Windows binary reaches this script out of a zip on a Linux
# runner, where it has no execute bit to carry and needs none to be packed.
add() {
  local platform=$1 src=$2 name=$3
  [ -f "$src" ] || { echo "$platform: $src is not a file" >&2; exit 2; }
  mkdir -p "$bin/$platform"
  cp "$src" "$bin/$platform/$name" && chmod 755 "$bin/$platform/$name"
}

# The npm page shows the README packed with the version, so it is the repository's own, cut at
# pack time: a hand-kept copy drifted from it. npm resolves no relative path, so each one is
# pinned to this version's tag — images to the raw file, links to the GitHub page. An HTML
# `src="…"`, as the logo is written, is an image like any other.
repo=https://github.com/devmaxxx/zerograph
sed -E \
  -e "s#!\[([^]]*)\]\(([^):\#][^):]*)\)#![\1](https://raw.githubusercontent.com/devmaxxx/zerograph/v$version/\2)#g" \
  -e "s#src=\"([^\":\#/][^\":]*)\"#src=\"https://raw.githubusercontent.com/devmaxxx/zerograph/v$version/\1\"#g" \
  -e "s#\]\(([^):\#][^):]*)\)#]($repo/blob/v$version/\1)#g" \
  README.md >npm/zerograph/README.md

[ -n "$darwin_arm64" ] && add darwin-arm64 "$darwin_arm64" zerograph
[ -n "$linux_x64" ] && add linux-x64 "$linux_x64" zerograph
[ -n "$win32_x64" ] && add win32-x64 "$win32_x64" zerograph.exe

(cd npm/zerograph && npm pkg set version="$version" && npm pack --silent --pack-destination "$root/$out")

echo "packed $version into $out:"
ls -1 "$out"
echo
echo "publish with: npm publish $out/zerograph-$version.tgz"
