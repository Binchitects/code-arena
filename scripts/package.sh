#!/usr/bin/env bash
# Packages one built binary: dist/code-arena-<version>-<rid>.tar.gz (.zip on Windows), with README.txt and LICENSE,
# then writes dist/SHA256SUMS for every package of that version in dist/.
#
#   scripts/package.sh <cargo target> <rid> <version>
#   scripts/package.sh x86_64-unknown-linux-musl linux-x64 0.1.0
set -euo pipefail
target=$1 rid=$2 version=$3
root=$(cd "$(dirname "$0")/.." && pwd)
target_dir=${CARGO_TARGET_DIR:-$root/target}
exe=code-arena
case $rid in win-*) exe=code-arena.exe ;; esac
bin=$target_dir/$target/release/$exe
[ -f "$bin" ] || { echo "no binary at $bin: build it first (cargo build --release --target $target)" >&2; exit 1; }

name=code-arena-$version-$rid
stage=$root/dist/$name
rm -rf "$stage" && mkdir -p "$stage"
cp "$bin" "$stage/"
cp "$root/LICENSE" "$stage/LICENSE"
cat > "$stage/README.txt" <<TXT
Code Arena $version ($rid)

Farzan Arena's coding agent. One program, nothing to install.

  1. Put $exe on your PATH.
  2. Sign in:      code-arena login --url https://YOUR-ARENA
                   (your API key is under Your account → API key in Arena)
  3. Check:        code-arena doctor
  4. Use it:       code-arena -p "what does this project do?"

Your settings are in ~/.config/code-arena (Windows: %APPDATA%\\code-arena).
Source and licence (MIT): https://github.com/Binchitects/code-arena
TXT

cd "$root/dist"
# The same bytes for the same inputs: no owner, the last commit's time on every file.
stamp=$(git -C "$root" log -1 --format=%ct 2>/dev/null || date +%s)
case $rid in
  win-*)
    rm -f "$name.zip"
    if command -v 7z >/dev/null; then 7z a -tzip -bso0 "$name.zip" "$name" ; else zip -qr "$name.zip" "$name"; fi
    ;;
  *)
    tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$stamp" -czf "$name.tar.gz" "$name" 2>/dev/null \
      || tar -czf "$name.tar.gz" "$name"
    ;;
esac
rm -rf "$stage"

sums=$(ls code-arena-"$version"-*.tar.gz code-arena-"$version"-*.zip 2>/dev/null || true)
if command -v sha256sum >/dev/null; then sha256sum $sums > SHA256SUMS; else shasum -a 256 $sums > SHA256SUMS; fi
echo "dist/$name: $(ls "$name".* | head -1)"
