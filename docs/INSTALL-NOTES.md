# Install notes: where `agentfx` looks for things

The `agentfx` CLI needs read-only data (fonts, design lenses and skins, helper scripts) and a
writable place for caches and scratch files. Both are resolved at run time, so the same
binary works from a build tree, from an install prefix, or with everything pointed at by
environment variables.

## Data directory (`AGENTFX_HOME`)

First match wins:

1. `$AGENTFX_HOME`, if set.
2. `<exe dir>/../share/agentfx`, if it contains `assets/fonts/fonts.json`. `tools/install.sh` puts
   the binaries in `<prefix>/bin/` and the data in `<prefix>/share/agentfx/`.
3. The source tree the binary was compiled from (the repository root), if it still exists.
   This is the developer fallback: `cargo run` and a copied dev build keep working.

What `agentfx` reads from it:

| Path | Used for |
|---|---|
| `assets/fonts/` | bundled fonts (`fonts.json`, `*.ttf`) |
| `design/lenses/`, `design/skins/` | design lenses and component skins |
| `tools/*.py`, `tools/capture.mjs` | TTS, harvest, critique, pitch, picks, Chrome capture |
| `.venv/bin/python` | Python for the tools, if present (else `$AGENTFX_PYTHON`, else `python3`) |

A data directory for an install therefore needs `assets/`, `design/` and `tools/` copied
from the source tree.

## Cache and scratch directory

First match wins:

1. `$AGENTFX_CACHE`, if set.
2. The data directory itself, if it came from `$AGENTFX_HOME` or from the build tree. Caches
   then live in `<home>/cache/` and scratch files in `<home>/out/tmp/` (the layout the build
   tree has always used; both are git-ignored).
3. Otherwise (installed under `share/agentfx`, which may be read-only):
   `$XDG_CACHE_HOME/agentfx`, else `~/.cache/agentfx`.

The layout inside is the same in every case: `cache/{tts,mesh,segments}` and `out/tmp`.

Not covered yet: audio analysis is cached next to the document (`<doc dir>/cache/audio`),
and `agentfx picks add` appends to `design/picks.jsonl` inside the data directory.

## External tools

Every `AGENTFX_*` variable also accepts its older `REEL_*` name. The full table is in
[BUILD.md](BUILD.md#environment-variables).

| Variable | Default | Used for |
|---|---|---|
| `AGENTFX_FFMPEG` | `ffmpeg` on `PATH` | audio analysis, audio mix, TTS post-processing |
| `AGENTFX_FFPROBE` | `ffprobe` on `PATH` | TTS durations |
| `AGENTFX_RENDERER` | `agentfx-render` next to `agentfx`, then on `PATH`, then `<source tree>/target/release/agentfx-render` | direct-mode rendering |
| `AGENTFX_PYTHON` | `<home>/.venv/bin/python`, else `python3` | helper scripts |
| `AGENTFX_NODE` | `node` on `PATH` | Chrome capture (`--backend chrome`) |

## Text to speech

`say` lines get their audio and word timings from a TTS engine, picked with `--tts <engine>`
(or `$AGENTFX_TTS`); `tools/tts.py` does the work.

| Engine | Default | What it is |
|---|---|---|
| `piper` | yes | Local, offline. Runs the `piper` CLI ([piper-tts](https://github.com/OHF-Voice/piper1-gpl), GPL-3.0, called as a separate program, not linked). Install it into the Python `agentfx` uses (`.venv/bin/pip install piper-tts`) or point `$AGENTFX_PIPER` at the binary. |
| `none` | when piper is missing | No audio; word timings are estimated (15 chars/s scaled by `rate`), lint reports `I_TTS_ESTIMATED`. `--no-tts` is the same. |
| `edge` | never | Microsoft Edge's online read-aloud voices through the third-party [`edge-tts`](https://github.com/rany2/edge-tts) package (`pip install edge-tts`). This is an undocumented service with no published terms for third-party use; check whether you may use it before you do. |

**Default voice:** `en_US-ljspeech-medium` from [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices)
(repository MIT). Its model card says it was trained from scratch on the
[LJ Speech dataset](https://keithito.com/LJ-Speech-Dataset/), which is in the **public domain**.
Other piper voices work with `voice=<name>` (e.g. `voice=en_US-ljspeech-high`); check each
voice's `MODEL_CARD` for its dataset licence first, several are research-only. A path to an
`.onnx` file also works.

Voices are looked up as `<name>.onnx` + `<name>.onnx.json` in `$AGENTFX_PIPER_VOICES`, then in
`cache/piper-voices/` next to the TTS cache, and downloaded there from Hugging Face on first use
(about 60 MB; `AGENTFX_PIPER_DOWNLOAD=0` turns the download off). piper reports no word boundaries,
so word times are spread over the real audio by word length and snapped to the nearest pause;
anchors land within about a tenth of a second. Voice names in the `edge` form
(`en-US-AvaNeural`) fall back to the default piper voice with a note on stderr.

| Variable | Default | Used for |
|---|---|---|
| `AGENTFX_TTS` | `piper` | engine: `piper`, `edge`, `none` |
| `AGENTFX_PIPER` | `piper` next to the tools' Python, else on `PATH` | piper binary |
| `AGENTFX_PIPER_VOICES` | unset | extra voice directory, searched first |
| `AGENTFX_PIPER_DOWNLOAD` | `1` | `0` = never download voices |

## Layout switches

`agentfx` lays out with the renderer's patched Blitz crates in its own mode (whole-pixel box
rounding, no per-line text snapping, rounded line tops; see `native::layout_mode`). It sets
`AGENTFX_PARLEY_ROUND_LINES=1` for its own process unless the variable is already set.
