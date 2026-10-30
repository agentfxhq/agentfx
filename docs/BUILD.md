# Building and configuring AgentFX

## Requirements

- Rust stable with edition 2024 support (1.85 or newer) and a C linker (`cc`).
- At run time: `ffmpeg` and `ffprobe` (ffmpeg with libx264) on `PATH`, or pointed at by
  `AGENTFX_FFMPEG` / `AGENTFX_FFPROBE`.
- Optional: a Vulkan driver for GPU effects (`--gpu`), VAAPI/QSV or NVENC for hardware
  encoding, `piper` for text to speech (see [INSTALL-NOTES.md](INSTALL-NOTES.md)), Node.js
  with `playwright-core` only for the Chrome reference backend (`--backend chrome`).

Linux (x86_64) is the tested platform. macOS and Windows are untested; the code is plain
Rust and may build there, but GPU effects use Vulkan only (no Metal or DirectX backend yet).

## Build and test

One Cargo workspace builds both binaries:

```sh
cargo build --release   # target/release/agentfx (CLI) and target/release/agentfx-render (renderer)
cargo test --release    # encode/export tests skip themselves when ffmpeg or an encoder is missing
```

| Crate | Path | Output |
|---|---|---|
| `agentfx-ir` | `crates/ir` | the intermediate representation (scene graph, tracks, assets) |
| `agentfx-render` | `crates/render` | binary `agentfx-render`: IR or HTML scene to video |
| `agentfx-script` | `crates/script` | binary `agentfx`: the Reel Script compiler and main CLI |

The patched Blitz, Parley and Stylo crates live in `vendor/` and are wired in through the
workspace `[patch.crates-io]` (see [vendor/README.md](../vendor/README.md)).

`agentfx` finds the renderer in this order: `$AGENTFX_RENDERER` (explicit override), then
`agentfx-render` next to its own executable, then on `PATH`, then the source tree's
`target/release/agentfx-render`.

## Install

```sh
tools/install.sh                   # ~/.local/bin/{agentfx,agentfx-render}, data in ~/.local/share/agentfx
PREFIX=$HOME/opt/agentfx tools/install.sh
```

No root access is needed. The data directory holds the fonts, design lenses and skins, the
helper scripts (TTS, transcription, critique), the MCP server and the spec.

## Linker

The repository ships no `.cargo/config.toml`; cargo uses the platform default linker.
To use a faster linker, configure it outside the repository, for example in
`~/.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
# linker = "/path/to/your/cc-wrapper"
# rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

## Environment variables

Every variable has an `AGENTFX_*` name. The older `REEL_*` name with the same suffix is
still accepted as a fallback (`AGENTFX_FFMPEG` wins over `REEL_FFMPEG`).

| Variable | Used by | Default | Purpose |
|---|---|---|---|
| `AGENTFX_HOME` | agentfx | `<exe>/../share/agentfx`, else the source tree | read-only data directory (fonts, design, tools) |
| `AGENTFX_CACHE` | agentfx | the data dir in a source tree, else `$XDG_CACHE_HOME/agentfx` or `~/.cache/agentfx` | caches (`cache/tts`, `cache/mesh`, `cache/segments`) and scratch files |
| `AGENTFX_RENDERER` | agentfx | `agentfx-render` next to `agentfx`, then on `PATH` | renderer binary |
| `AGENTFX_FFMPEG` | both | `ffmpeg` on `PATH` | ffmpeg for x264, export formats, audio |
| `AGENTFX_FFPROBE` | both | `ffprobe` on `PATH` | ffprobe (TTS durations, tests) |
| `AGENTFX_FFMPEG_HW` | agentfx-render | `AGENTFX_FFMPEG`, then each `ffmpeg` on `PATH` | ffmpeg for VAAPI/QSV/NVENC; the first that passes a probe encode is used |
| `AGENTFX_VAAPI_DEVICE` | agentfx-render | `/dev/dri/renderD128` | DRM render node for VAAPI/QSV |
| `AGENTFX_NVENC_SESSIONS` | agentfx-render | from the NVIDIA driver version, else 3 | concurrent NVENC sessions |
| `AGENTFX_GPU` | agentfx-render | `0` | GPU effects: `0`, `1` or `auto` (same as `--gpu`) |
| `AGENTFX_JOBS_DIR` | agentfx-render | unset (off) | write job status JSON files there (same as `--jobs-dir`) |
| `AGENTFX_TTS` | agentfx | `piper` | TTS engine: `piper`, `edge` (opt-in), `none` |
| `AGENTFX_PIPER` | tools/tts.py | `piper` next to the tools' Python, else on `PATH` | piper binary |
| `AGENTFX_PIPER_VOICES` | tools/tts.py | unset | extra voice directory, searched first |
| `AGENTFX_PIPER_DOWNLOAD` | tools/tts.py | `1` | `0` never downloads voices |
| `AGENTFX_PYTHON` | agentfx | `<home>/.venv/bin/python`, else `python3` | Python for the helper scripts |
| `AGENTFX_WHISPER_PY` | tools/transcribe.sh | a Python that imports `faster_whisper` | transcription |
| `AGENTFX_NODE` | agentfx | `node` on `PATH` | Chrome reference backend |
| `AGENTFX_CHROME` | tools/capture.mjs | Playwright's headless shell | Chrome binary for the reference backend |
| `AGENTFX_BIN` | mcp/server.py, tools | `agentfx` on `PATH`, else `target/release/agentfx` | CLI the MCP server calls |
| `AGENTFX_GIT_HASH` | build | `git rev-parse --short HEAD` | version string baked in at build time |

The renderer also reads a number of debugging and profiling switches (`AGENTFX_PROFILE`,
`AGENTFX_RANGE`, `AGENTFX_PLANNER`, `AGENTFX_GPU_CHECK`, ...). They are documented next to
their code (`git grep -n AGENTFX_ crates/render`) and are not a stable interface.

On Linux, when the render node cannot be opened but the user is in group `render` and
`sg` exists, hardware encodes run through `sg render`.

## GPU effects in containers and WSL

GPU effects (fluid simulation, blur kernels, mesh projection) use Vulkan via wgpu.
The renderer probes for a usable Vulkan device at startup. When none is found it
falls back to CPU and prints a one-line notice; all effects still render, just slower.
Set `AGENTFX_GPU=0` to skip the probe entirely (the Dockerfile does this).

| Environment | Vulkan available? | Notes |
|---|---|---|
| Bare-metal Linux with a discrete GPU | Yes | install the vendor Vulkan ICD (`mesa-vulkan-drivers`, `nvidia-driver`, etc.) |
| Docker / OCI container | Rarely | requires `--device /dev/dri` plus the host ICD; most CI images lack both |
| WSL 2 (Windows) | Sometimes | needs a recent Windows GPU driver with WSL Vulkan support; older setups fall back to CPU |
| macOS | No | wgpu requests Vulkan only; Metal is not wired in yet |
| Headless cloud VM (no GPU) | No | CPU fallback is automatic |

**In practice, containers and CI always run CPU-only.** This is fine for correctness;
GPU mainly accelerates fluid simulation and large blur passes. The Dockerfile sets
`AGENTFX_GPU=0` so the probe is never attempted.
