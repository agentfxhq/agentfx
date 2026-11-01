# Render performance

Where `agentfx-render` spends its time, what was changed in the `perf/profile` pass
(2026-11-01), and what scene authors can do about the rest. Measured on HORIZON, the
60 s vaporwave video of the 2026-10-31 workshop (26 glow, 22 blur, 8 grain, 4 vhs, 2
fluid, 1 point swarm, about 38 animated ellipses), on the office box: i7-2600K (4 cores /
8 threads), 16 GB, GTX 1050 Ti (Vulkan).

## Measuring

```sh
agentfx ir horizon.reel --target reel-ir -o ir/scene.ir.json --no-tts
AGENTFX_PROFILE=1 AGENTFX_PROFILE_OUT=prof.jsonl AGENTFX_RANGE=900:903 \
  agentfx-render render ir/scene.ir.json --out x.mp4 --no-encode
```

- `AGENTFX_PROFILE=1`: per-stage table on stderr (exclusive ms per composited frame);
  `AGENTFX_PROFILE_OUT` appends one JSON line per frame (`stages`: name -> [excl ms, calls, px]).
- `AGENTFX_PROFILE=2`: additionally one `fxtrace` line per offscreen effect node: name,
  effects, raster size and position, ink, region, cache hit, ms. This is the quickest way to
  find a node whose raster is far larger than the frame.
- `AGENTFX_RANGE=a:b` renders frames [a, b) only. The first frame of a range is full damage
  and fills the caches; compare later frames. A fluid node simulates every step from 0 to the
  frame (161 ms per step at 1080p here), so for sweeps `--hide <fluid>` keeps it out.
- `perf` is not usable as a normal user on this box (`perf_event_paranoid` = 4).

The numbers below are the fastest frame after the first of each 3-frame range, with the
fluid hidden, while other agents were compiling on the same machine (load average 15 to 25,
plus thermal `idle_inject`): absolute times are pessimistic, ratios between the columns
were measured under the same conditions and are the useful part.

## Results (HORIZON, CPU, ms per frame)

| t | 720p before | 720p after | 1080p before | 1080p after | 1080p speedup |
|---|---:|---:|---:|---:|---:|
| 3 s | 1580 | 700 | 1600 | 660 | 2.4x |
| 12 s | 12370 | 900 | 28190 | 1480 | 19x |
| 20 s | 12560 | 850 | 29280 | 1710 | 17x |
| 30 s | 16210 | 2220 | 39490 | 4390 | 9.0x |
| 40 s | 1020 | 540 | 1890 | 1000 | 1.9x |
| 50 s | 3290 | 1200 | 4570 | 2020 | 2.3x |
| 57 s | 70 | 50 | 180 | 50 | 3.9x |
| mean | 6730 | 920 | 15030 | 1620 | 9.3x |

A second back-to-back run while the box was quieter (load 9 to 15), 1080p: t = 12 s
24375 -> 994 ms (25x), t = 30 s 28063 -> 2202 ms (13x), t = 50 s 2913 -> 1265 ms (2.3x).

Full 60 s at 1080p30 (1800 frames): before about 7.5 h of frame rendering plus the fluid
(about 3 min); after about 49 min plus the fluid and encoding (convert + pipe about 20 ms
per frame), so roughly 50 to 55 min on this loaded box, less on an idle one.

### Stages (mean over the 7 sample frames, 1080p, ms per frame)

| stage | before | after | what it is |
|---|---:|---:|---|
| fx.glow | 10317 | 382 | glow effect: tint, blur, composite under the layer |
| vello.rasterize | 1905 | 148 | vello_cpu fine rasterization of contexts |
| fx.vhs | 1324 | 360 | VHS look (whole raster of the `#fx` / `#tape` layer) |
| draw.paint | 499 | 290 | display lists; mostly dithered gradient images (conic petals) |
| fx.rgbsplit | 446 | 142 | RGB split look |
| fx.subtree | 135 | 8 | offscreen subtree setup |
| time.echo | 105 | 45 | echo time effect (5 trailing copies of `#kal`) |
| mblur.cloud | 92 | 78 | point swarm with motion blur |
| fx.grain | 84 | 97 | grain look (already parallel and AVX2) |
| fx.glitch | 70 | 29 | glitch look |
| fx.blur | 33 | 23 | blur effect |
| fluid.sim | 161 | 161 | per step while the fluid is on screen (21 s to 36 s) |
| convert.nv12 + encode.pipe | 21 | 21 | RGBA -> NV12 and the pipe to ffmpeg (60-frame run) |

## What was slow

1. **A glow on a layer scaled far past the frame.** `#px` is a 12 px square with
   `glow=20`, stretched with `sx=110 sy=0.4` into the horizon line. Effect parameters scale
   with the layer's largest axis scale, so the glow sigma became about 3300 px at 1080p and the
   offscreen raster 21780 x 19808 px (431 Mpx, 1.7 GB per buffer) for a frame of 2 Mpx:
   10 to 15 s per frame from 12 s to 30 s, about two thirds of the whole render.
2. **Digital looks were single-threaded.** `vhs`, `rgbsplit` and `glitch` ran one pixel
   loop on one core over the whole (frame-plus-margin) raster, with an unpremultiply per
   sample (10 per pixel for VHS).
3. **Offscreen rasterization was single-threaded.** Every offscreen context (effect inputs:
   the kaleidoscope, the echo samples) used vello_cpu without worker threads.
4. **Dithered gradients are evaluated per pixel on the CPU** (`dither.rs`, Chrome parity
   against banding). Rotating conic/radial gradients miss the gradient cache every frame;
   the evaluation was single-threaded.
5. **`--gpu` did nothing for HORIZON.** By default only riso, halftone, rays and shadow
   start a GPU chain; blur and glow only join a chain that is already on the GPU. The run
   reported `"gpu": null`.

## What changed

| change | where | output |
|---|---|---|
| Tiled large blur / glow: a single blur or glow whose raster is at least 4 Mpx and 4x the area near the visible region is rendered in 1024 px tiles that feed the blur's reduction directly (`effects::TiledBlur`); the result is reconstructed only for the crop that can reach the frame | `vraster.rs` `tiled_blur_pixmap`, `effects.rs` | same algorithm and arithmetic; at most a few hundred bytes per 1080p frame differ by 1 (2 bytes by 2) because vello rasterizes the tiles at other integer offsets |
| Visibility culling below digital looks: a vhs / rgbsplit / glitch layer keeps its full raster (its edge clamping depends on it) but only content within reach of the visible region is drawn into it, and blur-type rasters below it are cropped to that area on the blur's reduction grid; cleared for every cached, resampled or non-local render | `vraster.rs` (`vis`, `fx_reach`) | bitwise identical on the HORIZON frames checked |
| Digital looks run rows in parallel and unpremultiply the source once | `digital.rs` | bitwise identical |
| Blur reconstruction: column taps computed once per buffer, all-transparent taps skipped | `effects.rs` `upsample` | bitwise identical |
| Offscreen contexts of at least 512 K px use vello worker threads (`AGENTFX_OFFSCREEN_THREADS`, default min(cores, 8), 0 = off) | `vraster.rs` `render_view` | bitwise identical (vello_cpu MT = ST) |
| Dithered gradient images computed in parallel rows | `dither.rs` | bitwise identical |
| `AGENTFX_PROFILE=2` offscreen trace | `prof.rs`, `vraster.rs` | no output change |

`AGENTFX_FX_CULL=0` turns off the culling and the tiled path (A/B checks).

Attribution at 1080p, frames at 12 s + 30 s together: before 67.7 s; with everything except
the tiled path and the culling (`AGENTFX_FX_CULL=0`) 26.0 s (mostly the reconstruction
skipping the transparent bulk of the 431 Mpx glow raster, and the parallel digital looks);
with everything 5.9 s. The offscreen worker threads made no measurable difference on the
loaded box (26.0 s with, 25.6 s without); they are bitwise neutral and may help on an idle
machine, `AGENTFX_OFFSCREEN_THREADS=0` turns them off.

## GPU

### Before (the profile pass)

With the default routing `--gpu` changed nothing on HORIZON (`"gpu": null`): only riso,
halftone, rays and shadow started a GPU chain, blur and glow merely joined one. Routing blur
and glow by hand (`AGENTFX_GPU_FX=riso,halftone,rays,shadow,blur,glow,drop-shadow`) took about
40 % off the glow time on the 1050 Ti, within the noise of the frame total: about 75 % of a
frame was CPU work without a GPU port.

### What is on the GPU now (branch `feat/gpu-looks`)

| effect | kernel | vs the CPU |
|---|---|---|
| blur, drop shadow, glow | box passes, block reduction, integer bilinear reconstruction (`comp.wgsl` `up`, now up to scale 1024 = sigma ~7000) | bit-exact |
| tiled large blur / glow (layer scaled past the frame) | reduced buffer up, blur + reconstruction of the crop + sharp layer over it on the GPU (`fx::tiled_finish`) | bit-exact |
| rgbsplit, glitch, vhs | gather through the host's sampling tables (`digital::Tab`, shared with the CPU loop), unpremultiply through a 64 K exact-division table | max 1 / 255 (rgbsplit, glitch: 0) |
| grain (fine, coarse, live) | in place, integer hashes, value noise | max 1 / 255 |
| riso, halftone, rays, shadow | (unchanged) | max 3, PSNR >= 45 dB |

Routing with `--gpu` (`gpu/fx.rs`): every ported effect may move a frame to the GPU
(`AGENTFX_GPU_FX`, default all). The digital looks and grain from `AGENTFX_GPU_MIN_PX` (65536 px),
blur / drop shadow / glow from `AGENTFX_GPU_BLUR_MIN_PX` (512 K px: a lone glow there is about
break-even with the CPU on Data, where 1080p is 24 ms CPU vs 17 ms GPU with the transfer; Soong's
faster bus wins down to roughly 256 K px, 15 vs 6 ms at 1080p). A node's consecutive GPU effects run on one resident
frame (one upload, one readback); an effect that is not ported, or a matte, flushes. Frames do
not stay on the GPU across nodes: the parent composites its children with vello_cpu.

Conformance (`gpu::fx::tests`, `--test-threads 1`): on the GTX 1050 Ti and the RTX 5060 Ti the
digital looks are max 0 (rgbsplit, glitch) or 1 (vhs, grain: at most 62 of the 479 K channels of
a 403 x 297 fixture) on smooth, opaque and random-alpha sources at node scales 1 and 1.37; a HORIZON chain
(grain, rgbsplit, glitch, vhs, glow) PSNR 89.6 dB, max 2. Whole HORIZON frames at 12, 30 and 50 s,
`--gpu` vs CPU: max 1 (one channel 2), 0.015 % of channels differ, PSNR 86 to 87 dB. The CPU
output is bitwise unchanged by this work (frames at 12, 20, 30, 50 s compared).

### Results (HORIZON v1 1080p reel, `--hide swirl`, ms per frame)

`h1.ir.json` is `agentfx ir horizon.reel` of the workshop reel (`hbench/horizon.reel`; 13 glow,
1 blur, 4 grain, 3 rgbsplit, 3 vhs, 2 glitch, a swarm, echo, the fluid hidden). It is a lighter
variant than the one profiled above (whose numbers are not comparable). HORIZON v2
(`horizon-v2-t2.ir.json`) uses `kaleido` / `tunnel` from `feat/gen-warp` and does not load on
main. Fastest frame after the first of each 3-frame range, minimum over 3 rounds (Data) or 2
(Soong). Data: i7-2600K (4 cores, no FMA), GTX 1050 Ti, shared box (load 2 to 8). Soong:
i7-8700 (6 cores), RTX 5060 Ti 16 GB, Windows, Flint paused.

| t | Data CPU before | Data CPU after | Data `--gpu` before | Data `--gpu` after | Soong CPU before | Soong CPU after | Soong `--gpu` before | Soong `--gpu` after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 12 s | 318 | 168 | 319 | **82** | 139 | 71 | 136 | **37** |
| 30 s | 691 | 544 | 726 | **468** | 312 | 242 | 304 | **203** |
| 40 s | 183 | 156 | 174 | **122** | 85 | 76 | 83 | **55** |
| 50 s | 487 | 341 | 754 | **135** | 139 | 105 | 142 | **57** |
| mean | 420 | 302 | 493 | **202** | 169 | 123 | 166 | **88** |

"before" = `416d59a` (main after the profile pass), "after" = this branch. "`--gpu` before"
with blur/glow routed by hand: Data 412, Soong 160. The CPU-only gain comes from the vhs
sampling tables (no `sin` per pixel) and the smaller crop of the tiled glow (`#px`: 148 -> 38 ms).

Stages, mean of the four frames (ms per frame; `gpu.*` = upload into the staging buffer, wait
for the GPU, readback; under `--gpu` the effect stages keep only their CPU parts):

| stage | Data CPU before | Data CPU after | Data `--gpu` | Soong CPU before | Soong CPU after | Soong `--gpu` |
|---|---:|---:|---:|---:|---:|---:|
| fx.vhs | 105 | 48 | 0 | 34 | 14 | 0 |
| fx.glow | 73 | 43 | 16 | 39 | 24 | 9 |
| fx.rgbsplit | 39 | 20 | 0 | 15 | 9 | 0 |
| fx.grain | 24 | 24 | 0 | 8 | 8 | 0 |
| fx.glitch + fx.blur | 17 | 10 | 0 | 7 | 4 | 0 |
| draw.paint (dithered gradients) | 53 | 54 | 54 | 21 | 20 | 21 |
| vello.rasterize | 33 | 34 | 35 | 19 | 18 | 19 |
| time.echo | 20 | 19 | 22 | 13 | 12 | 12 |
| mblur.cloud (swarm) | 46 | 43 | 16 | 7 | 8 | 7 |
| gpu.upload / wait / readback | - | - | 19 / 28 / 4 | - | - | 5 / 6 / 2 |

What is left under `--gpu` is the vector side: dithered gradients, vello rasterization, the
echo's five extra subtree renders, the swarm splat; t = 30 s is dominated by `#kal` (a glow over
the kaleidoscope: echo + conic petals). On Data `mblur.cloud` also dropped (46 -> 16) although the
swarm has no GPU port; it was not reproduced in isolation (the shared box's load varies
between rounds), treat it as noise.

### Residency (one upload / readback per node)

`AGENTFX_GPU_RESIDENT=0` reads the frame back after every GPU effect. HORIZON's `#fx` (grain,
rgbsplit, vhs on 2048 x 1208) and `#tape` are the chains that profit:

| per frame | Data resident | Data not | Soong resident | Soong not |
|---|---:|---:|---:|---:|
| upload + readback MB | 18 to 32 each way | 42 to 51 | (same) | (same) |
| gpu.upload / wait / readback ms | 19 / 28 / 4 | 33 / 41 / 6 | 5 / 6 / 2 | 8 / 10 / 4 |
| frame mean ms | 202 | 226 | 88 | 98 |

A 1080p frame up + down costs 13 ms on Data (PCIe 2, the staging copy into write-combined
memory is the slow half) and 5 ms on Soong; per effect the GPU kernels are 1 to 3 ms. Standalone
(`gpu_bench`, upload + readback included) vhs is 3x the CPU on both machines, rgbsplit / glitch
1.1x (Data) to 1.9x (Soong), grain alone is slower than the CPU (it is 3 to 7 ms of hashing on the
CPU): its win is in a chain.

### Full 60 s at 1080p30 (1800 frames, x264)

Measured (`h1.ir.json` with the fluid, whole video, one process):

| machine | build | mode | wall | notes |
|---|---|---|---:|---|
| Data | after | CPU, 1 segment, NVENC | 1179 s (19.7 min) | box load 9 to 11 during the run |
| Data | after | `--gpu`, 1 segment, NVENC | 589 s (9.8 min) | load 0.4 to 9; 0 GPU fallbacks, 55 GB up / 55 GB down |
| Soong | before | CPU, `--segments 4`, x264 | 296 s | |
| Soong | after | `--gpu`, 1 segment, x264 | 201 s | |
| Soong | after | `--gpu`, `--segments 4`, x264 | 140 s | 2.1x the old CPU run |

From the frame means above (x 1800, plus the fluid's 26 s on Data, encode overlapped): Data
before about 13 min, after CPU about 9.5 min, after `--gpu` about 6.5 min on an idle box (the
measured runs are slower because the box was shared); Soong before about 5 min, after `--gpu`
about 2.6 min in one segment, and `--segments 4` overlaps the remaining CPU stages (vector
rasterization, gradients) of four frames.

### `--gpu=auto`

`auto` now uses only a discrete adapter (not llvmpipe, not an integrated or virtual GPU) and
runs a start-up self-test (`fx::self_test`, a few ms: glow on both blur paths bit-exact; vhs,
glitch, grain within 1 on a 96 x 64 fixture). A failed test prints one line and renders on the
CPU; no adapter at all stays quiet. Any GPU error during a render already replays the chain's
effects on the CPU (`failed_fell_back` in the stats).

Proposal: make `auto` the default of `agentfx-render` (and so of `agentfx render`). It is
faster on both machines measured (Data 1.5x, Soong 1.4x per frame on HORIZON over the new CPU
path), the output differs from the CPU by at most 1 / 255 in rare channels (blur / glow exactly
equal), and the guards make it safe on CPU-only, CI and container hosts (no adapter: quiet CPU
path; `AGENTFX_GPU=0` keeps the old behaviour and skips the probe). Not done in this branch
because it changes the default output bits of every GPU machine (regression baselines are CPU
renders) and adds the Vulkan device, pipelines and self-test (about 150 to 200 ms on Data) to every
`agentfx frame` preview. Until then:
`AGENTFX_GPU=auto` or `--gpu=auto`.

### NVENC on RTX 50

The NVENC probe encoded a 128 x 128 frame; on the RTX 5060 Ti (driver 610.88) both h264_nvenc
and hevc_nvenc refuse it ("Frame Dimension less than the minimum supported value"; h264 needs a
width of at least 145, 160 x 96 and 256 x 64 pass). The probe now uses 256 x 256, the encode
tests 160 x 64. The NVENC session cap was read from `/proc/driver/nvidia/version` only; on
Windows it falls back to `nvidia-smi --query-gpu=driver_version` (610.88 -> 8 sessions).

### Vector paths and dithered gradients on the GPU (investigation, not done)

- **vello_hybrid** (CPU path flattening + sparse-strip generation, GPU fine rasterization) is the
  natural successor of vello_cpu here: its `Scene` API mirrors `RenderContext` (fill_path,
  push_layer / clip / blend, set_paint, glyph_run), and it shares `vello_common`'s strip
  generation, so coverage should be close to vello_cpu's. Blocker today: the current
  `vello_hybrid` 0.2.0 depends on `vello_common` 0.2 and `wgpu` 29, the tree is on
  `vello_cpu` / `vello_common` 0.3 and `wgpu` 30; two wgpu versions cannot share buffers, so
  it needs the next vello_hybrid release (or the whole stack pinned back). Then the frame would
  stay on the GPU for nodes whose content and effects are all GPU (no readback between a
  layer's raster and its effects, the bigger win than the rasterization itself: `vello.rasterize`
  is 19 to 35 ms per frame here). Images (`draw_pixmap` of effect results, dithered gradient
  images, glyph masks) would have to become GPU textures; output equality with vello_cpu is
  unverified (u8 vs f32 fine pipelines) and every regression baseline would need a check.
- **vello** (the GPU compute renderer, 0.11, already on wgpu 30) renders a whole `vello::Scene`
  into a texture, but its rasterization (analytic area coverage, its own blend pipeline) is not
  vello_cpu's: not bitwise, and the API is different (scene encoding with peniko brushes).
  More work for a less exact result.
- **Dithered gradients** (`dither.rs`, `draw.paint`: 54 ms per frame on Data, 21 on Soong) are
  a good GPU kernel: per pixel a piecewise-linear premultiplied ramp in f32 plus Skia's 8 x 8
  ordered offset, rounded. Stops and the gradient transform go up as parameters, the image
  comes back (or stays on the GPU with vello_hybrid). The CPU f32 ramp and the conic `atan2`
  differ by ulps on the GPU; the +-0.49 LSB offset then rounds the other way in rare pixels:
  within 1 / 255, like the digital looks. Worth it for large rotating conic / radial gradients
  (they miss the gradient cache every frame); with vello_cpu still compositing, it costs a
  readback per gradient image.

## Recommendations for scene authors

- **Do not put a glow or blur on a layer that is scaled up a lot.** The radius scales with
  the layer's largest scale: `glow=20` on a layer with `sx=110` is a 2200 px glow (at 720p).
  Put the glow on an unscaled parent, or draw the stretched shape at its final size (a
  `rect w=1320 h=5` instead of a 12 px square scaled 110x).
- `vhs`, `rgbsplit`, `glitch`, `dither` and `ascii` process their layer's whole raster (frame
  plus margin) every frame: one such layer at the top is fine, stacking several is not.
  Floyd dither cannot be parallelized.
- Glows are cheap when the layer is small; a glow on a full-frame layer (`#grid`, `#kal`)
  costs a full-frame blur per frame, and an animated glow amount (`glow=audio(...)`) defeats
  the effect cache.
- `echo n=5` renders the subtree 6 times per frame; keep it on small subtrees.
- Large rotating conic or radial gradients are evaluated per pixel every frame (dithering);
  static or merely moving ones are cached.
- A fluid simulates every step from the start of the video to the frame: previews
  (`agentfx frame`) late in a scene with a fluid pay the whole simulation.
- With a discrete GPU, render with `--gpu` (or `--gpu=auto`): glows, blurs, the digital looks
  and grain run there, a node's effects on one resident frame. Put the digital looks of a
  layer next to each other (a matte or an unported effect such as `color` or `dither` between
  them costs a readback + upload).

## Further opportunities (not done)

- GPU: `color`, `dither` (ordered / blue noise; Floyd is serial), `ascii` and `paper` are not
  ported; matte, kaleido / tunnel-style warps and compositing children stay on the CPU, so a
  frame never stays on the GPU across nodes (see the vector investigation above).
- Anisotropic effect scale: a glow on a layer scaled `sx=110 sy=0.4` uses the largest scale
  in both axes; Chrome blurs in layer space. Matching that would be both faster and closer.
- Frame-level parallelism: one painter renders frames in order; `--segments N` splits the
  video into N segments rendered concurrently (each with its own caches and fluid catch-up).
  With most stages now parallel inside a frame the gain is smaller than before, but
  single-threaded parts remain (display-list encoding, the swarm, cache misses).
- The Reel Script compile of HORIZON takes about 12 s (`nodes_s` 9 s in `reelir.rs`), paid by
  every `agentfx frame` preview.
- A fluid catch-up cache across `agentfx frame` calls.
