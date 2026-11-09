# Building and configuring AgentFX

## Requirements

- Rust stable with edition 2024 support (1.91 or newer) and a C linker (`cc`).
- At run time: `ffmpeg` and `ffprobe` (ffmpeg with libx264) on `PATH`, or pointed at by
  `AGENTFX_FFMPEG` / `AGENTFX_FFPROBE`.
- Optional: Vulkan on Linux or Metal on macOS for GPU effects (`--gpu`), VAAPI/QSV or NVENC on
  Linux, or VideoToolbox on macOS for hardware encoding, `piper` for text to speech (see
  [INSTALL-NOTES.md](INSTALL-NOTES.md)), Node.js
  with `playwright-core` only for the Chrome reference backend (`--backend chrome`).

Linux (x86_64) is the tested platform. macOS is built and CPU-tested in CI on `macos-latest` on demand (Actions > CI > Run workflow); GPU effects select Metal and
hardware encoding probes VideoToolbox there. Windows (x86_64, MSVC) builds and runs the CPU test
suite in CI on demand; see [Windows](#windows) below. GPU effects select DX12 or Vulkan there.

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
| `AGENTFX_CACHE` | agentfx | the data dir in a source tree, else `$XDG_CACHE_HOME/agentfx`, `~/Library/Caches/agentfx` on macOS, or `~/.cache/agentfx` | caches (`cache/tts`, `cache/mesh`, `cache/segments`) and scratch files |
| `AGENTFX_RENDERER` | agentfx | `agentfx-render` next to `agentfx`, then on `PATH` | renderer binary |
| `AGENTFX_FFMPEG` | both | `ffmpeg` on `PATH` | ffmpeg for x264, export formats, audio |
| `AGENTFX_FFPROBE` | both | `ffprobe` on `PATH` | ffprobe (TTS durations, tests) |
| `AGENTFX_FFMPEG_HW` | agentfx-render | `AGENTFX_FFMPEG`, then each `ffmpeg` on `PATH` | ffmpeg for VAAPI/QSV/NVENC; the first that passes a probe encode is used |
| `AGENTFX_VAAPI_DEVICE` | agentfx-render | `/dev/dri/renderD128` | Linux DRM render node for VAAPI/QSV |
| `AGENTFX_NVENC_SESSIONS` | agentfx-render | from the NVIDIA driver version (`/proc/driver/nvidia/version`, else `nvidia-smi`, e.g. on Windows), else 3 | concurrent NVENC sessions |
| `AGENTFX_GPU` | agentfx-render | `0` | GPU effects: `0`, `1` or `auto` (same as `--gpu`); `auto`: discrete GPUs only, after a start-up self-test against the CPU |
| `AGENTFX_GPU_FX` | agentfx-render | every ported effect | comma list of the effects that may move a frame to the GPU: `riso,halftone,rays,shadow,rgbsplit,glitch,vhs,grain,glass,optics,blur,drop-shadow,glow,kaleido,tunnel`; the others still run on the GPU when the frame is already there |
| `AGENTFX_GPU_MIN_PX` | agentfx-render | `65536` | smallest effect buffer (px) that goes to the GPU |
| `AGENTFX_GPU_BLUR_MIN_PX` | agentfx-render | `524288` | the same for a blur, drop shadow or glow that would start a GPU chain (they are cheap on the CPU next to a frame transfer) |
| `AGENTFX_GPU_RESIDENT` | agentfx-render | `1` | `0` reads the frame back after every GPU effect (measurement: what keeping a node's effects on the GPU saves) |
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

## GPU effects in containers, macOS and WSL

GPU effects (fluid simulation, blur kernels, mesh projection) use wgpu: Vulkan on Linux,
Metal on macOS, and DX12 or Vulkan on Windows. The renderer probes for a usable device at startup. When none is found it
falls back to CPU and prints a one-line notice; all effects still render, just slower.
Set `AGENTFX_GPU=0` to skip the probe entirely (the Dockerfile does this).

| Environment | Vulkan available? | Notes |
|---|---|---|
| Bare-metal Linux with a discrete GPU | Yes | install the vendor Vulkan ICD (`mesa-vulkan-drivers`, `nvidia-driver`, etc.) |
| Docker / OCI container | Rarely | requires `--device /dev/dri` plus the host ICD; most CI images lack both |
| Native Windows with an NVIDIA/AMD/Intel GPU | Yes | wgpu uses DX12 or Vulkan; the vendor driver ships both; see [Windows](#windows) |
| WSL 2 (Windows) | Sometimes | needs a recent Windows GPU driver with WSL Vulkan support; older setups fall back to CPU |
| macOS (Apple Silicon or Intel) | Yes | Metal is selected automatically; `WGPU_BACKEND` can override it for debugging |
| Headless cloud VM (no GPU) | No | CPU fallback is automatic |

**In practice, containers and CI always run CPU-only.** This is fine for correctness;
GPU mainly accelerates fluid simulation and large blur passes. The Dockerfile sets
`AGENTFX_GPU=0` so the probe is never attempted.

## Windows

Windows 10/11 on x86_64 builds natively with the MSVC toolchain. A CI job builds it and runs the
CPU test suite on `windows-latest` when the workflow is started by hand (Actions > CI > Run
workflow). Rendering on a real GPU box is not covered by CI yet.

### Prerequisites

- **Rust** via [rustup](https://rustup.rs) (`rustup-init.exe`), default host
  `x86_64-pc-windows-msvc`, stable 1.91 or newer.
- **Visual Studio Build Tools** (2019 or newer) with the *Desktop development with C++*
  workload (MSVC linker and the Windows SDK). rustup-init offers to install it.
- **Git for Windows**. The repository's `.gitattributes` checks text files out with LF, which
  the golden-file tests expect.
- **ffmpeg and ffprobe** on `PATH` (or `AGENTFX_FFMPEG` / `AGENTFX_FFPROBE` pointing at the
  `.exe`), a build with libx264, and NVENC for hardware encoding, for example the gyan.dev
  "full" build or the BtbN GPL build: `winget install Gyan.FFmpeg`, or unpack one and add
  its `bin` folder to `PATH`. ffmpeg 5.1 or newer is required (`-fps_mode`). Other programs
  put old ffmpeg builds on `PATH` (ImageMagick ships one); `where.exe ffmpeg` shows which one
  wins, and the renderer stops with "needs ffmpeg 5.1 or newer" naming the path it found.
  Pin the right one explicitly:

  ```powershell
  setx AGENTFX_FFMPEG  "C:\path\to\ffmpeg\bin\ffmpeg.exe"
  setx AGENTFX_FFPROBE "C:\path\to\ffmpeg\bin\ffprobe.exe"
  ```

  (`setx` affects new shells; use `$env:AGENTFX_FFMPEG = "..."` for the current one.)
- **GPU driver**: the current NVIDIA (or AMD/Intel) driver, which provides the DX12 and
  Vulkan runtimes that `--gpu` uses. RTX 50-series (Blackwell) cards need driver 570 or newer and
  an ffmpeg built against NVENC SDK 12.2 or newer for `--encoder nvenc`.
- Optional: Python 3 from python.org (helper tools, TTS: `pip install piper-tts`); the CLI
  calls `python` on Windows, or `AGENTFX_PYTHON`, or `<data dir>\.venv\Scripts\python.exe`.

### Build

From a "Developer PowerShell for VS" or any shell where `cargo` is on `PATH`:

```powershell
git clone https://github.com/agentfxhq/agentfx; cd agentfx
cargo build --release              # target\release\agentfx.exe and agentfx-render.exe
$env:AGENTFX_GPU = "0"; cargo test --release   # CPU suite; encode tests skip without ffmpeg
```

`tools/install.sh` is a POSIX shell script and does not run on Windows. Use the binaries
from `target\release` in place: `agentfx.exe` finds `agentfx-render.exe` next to itself and
the fonts, design data and tools in the source tree it was built from. To move the binaries
elsewhere, copy them together and set `AGENTFX_HOME` to a checkout (or a copy of `assets`,
`design`, `tools`, `docs`), and optionally `AGENTFX_CACHE` (default
`%LOCALAPPDATA%\agentfx` for an installed layout; the source tree itself in a build tree).

### Render

```powershell
.\target\release\agentfx.exe lint examples\quickstart.reel
.\target\release\agentfx.exe render examples\quickstart.reel --no-tts -o hello.mp4
# GPU effects (DX12 or Vulkan) and NVENC; `auto` probes and falls back to CPU / x264 with a notice
$env:AGENTFX_GPU = "auto"; $env:AGENTFX_NVENC_SESSIONS = "8"
.\target\release\agentfx.exe render examples\quickstart.reel --no-tts -o hello.mp4
```

`agentfx-render.exe render <scene.ir.json> --out x.mp4 --encoder nvenc --gpu` drives the
renderer directly with the same flags as on Linux.

### Known limits on Windows

- **Encoders:** x264 and NVENC (`nvenc`, `nvenc-hevc`). VAAPI and QSV look for a Linux DRM
  render node and report themselves unavailable; `--encoder auto` then uses x264.
- **NVENC sessions:** without `/proc` the driver version comes from `nvidia-smi`; when that
  is missing the concurrent session cap defaults to 3. Current GeForce drivers allow 8: set
  `AGENTFX_NVENC_SESSIONS=8` to be sure.
- **GPU backend:** wgpu picks DX12 or Vulkan (`WGPU_BACKEND=vulkan` or `dx12` forces one).
  NVIDIA, AMD and Intel drivers ship both; without either the renderer falls back to the CPU.
- **Fonts:** Reel Script uses the bundled fonts in `assets/fonts`. HTML scenes resolve the
  generic families to Liberation when installed, else to Arial, Times New Roman, Consolas
  and Segoe UI from `%WINDIR%\Fonts`, so text metrics can differ from Linux renders by a
  few pixels. Install the Liberation fonts in the per-user font folder for Linux parity.
- **Job status:** `--jobs-dir` records have no peak RSS, CPU time or caller chain (those
  read `/proc`); the host name comes from `COMPUTERNAME`.
- **Helper tools:** `tools/*.sh` (`install.sh`, `transcribe.sh`, `encode-loop.sh`) need a
  POSIX shell (Git Bash or WSL). The Python tools and the MCP server use only portable
  Python but are untested on Windows.
- Paths with non-ASCII characters and paths longer than 260 characters are untested.

### Alternative: WSL 2

Running the Linux build inside WSL 2 (Ubuntu 24.04) follows the Linux instructions
unchanged and is the closest match to the tested platform:

```sh
sudo apt-get install -y build-essential ffmpeg mesa-vulkan-drivers vulkan-tools
cargo build --release
```

GPU passthrough comes from the Windows driver through WSLg (`/dev/dxg`, D3D12). Vulkan in
WSL runs on Mesa's `dzn` driver (Vulkan on D3D12), which is new and may be missing or slow
in older Mesa; check with `vulkaninfo --summary`. When no Vulkan device is found the
renderer uses the CPU. NVENC inside WSL depends on the driver and the ffmpeg build;
`--encoder auto` probes it and falls back to x264. Keep projects on the Linux file system
(`~/...`), not under `/mnt/c`, which is much slower for the many small files a render writes.
