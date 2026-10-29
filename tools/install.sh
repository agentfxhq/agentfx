#!/bin/sh
# Build AgentFX (release) and install it for the current user, no root needed:
#   $PREFIX/bin/agentfx            the CLI (lint, frame, sheet, render, catalog, ...)
#   $PREFIX/bin/agentfx-render     the renderer the CLI calls
#   $PREFIX/share/agentfx/         fonts, design lenses and skins, helper scripts, spec
# PREFIX defaults to ~/.local. The binaries are copies, so rebuilding the checkout does not
# change the installed tools.
#   tools/install.sh
#   PREFIX=/opt/agentfx JOBS=2 tools/install.sh
#   NO_BUILD=1 tools/install.sh     # install an existing target/release build
set -eu
cd "$(dirname "$0")/.."
prefix=${PREFIX:-$HOME/.local}
bin_dir=$prefix/bin
data_dir=$prefix/share/agentfx
jobs=${JOBS:-4}

if [ -z "${AGENTFX_GIT_HASH:-}" ] && command -v git >/dev/null 2>&1 && git rev-parse --git-dir >/dev/null 2>&1; then
  AGENTFX_GIT_HASH=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
  git diff --quiet HEAD -- . 2>/dev/null || AGENTFX_GIT_HASH="$AGENTFX_GIT_HASH-dirty"
  export AGENTFX_GIT_HASH
fi

if [ -z "${NO_BUILD:-}" ]; then
  cargo build --release -j "$jobs" --bin agentfx --bin agentfx-render
fi

mkdir -p "$bin_dir"
for b in agentfx agentfx-render; do
  cp target/release/$b "$bin_dir/.$b.tmp.$$"
  mv -f "$bin_dir/.$b.tmp.$$" "$bin_dir/$b"
done

# data: copied file by file (an existing data dir is updated in place, nothing is deleted)
copy_tree() {
  find "$1" -type f ! -name '*.pyc' | while IFS= read -r f; do
    mkdir -p "$data_dir/$(dirname "$f")"
    cp "$f" "$data_dir/$f"
  done
}
mkdir -p "$data_dir"
copy_tree assets
copy_tree design
copy_tree tools
copy_tree schema
copy_tree mcp
mkdir -p "$data_dir/docs"
cp docs/SPEC.md "$data_dir/docs/SPEC.md"

"$bin_dir/agentfx" --version
echo "installed: $bin_dir/agentfx, $bin_dir/agentfx-render, data in $data_dir"
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "note: $bin_dir is not on PATH in this shell";; esac
command -v ffmpeg >/dev/null 2>&1 || echo "note: ffmpeg not found on PATH; rendering video needs it (or set AGENTFX_FFMPEG)"
