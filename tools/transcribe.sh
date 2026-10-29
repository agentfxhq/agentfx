#!/usr/bin/env bash
# Transcribe audio to Whisper JSON with word timestamps (for `captions src=`).
# Picks a Python that can import faster_whisper: $AGENTFX_WHISPER_PY, then known local envs, then python3.
# Usage: tools/transcribe.sh talk.wav -o talk.json [--model small] [--lang en] [--prompt "Reel Script"]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
for py in "${AGENTFX_WHISPER_PY:-${REEL_WHISPER_PY:-}}" "${AGENTFX_HOME:-${REEL_HOME:-$here/..}}/.venv/bin/python" python3; do
  [ -n "$py" ] || continue
  if "$py" -c 'import faster_whisper' 2>/dev/null; then
    exec nice -n 10 "$py" -I "$here/transcribe.py" "$@"
  fi
done
echo "no Python with faster-whisper found; set AGENTFX_WHISPER_PY" >&2
exit 1
