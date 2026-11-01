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

With the default routing `--gpu` changes nothing on HORIZON. Routing blur and glow too:

```sh
AGENTFX_GPU_FX=riso,halftone,rays,shadow,blur,glow,drop-shadow agentfx-render render ... --gpu
```

| 1080p, ms per frame (after the changes, back to back, load about 14) | CPU | `--gpu` + blur/glow routed |
|---|---:|---:|
| t = 12 s | 990 | 930 |
| t = 30 s | 2200 | 2570 |
| t = 50 s | 1270 | 1350 |
| fx.glow, sum of the 3 frames | 850 | 500 |

The 1050 Ti takes about 40 % off the glow time (upload and readback included), but at
this scale that is within the run-to-run noise of the CPU looks; the frame total does not
move. Before the changes the glow rasters were too large for the GPU anyway (they fell
back to the CPU), so `--gpu` was never going to fix HORIZON.

Would a faster GPU (an RTX 5060 Ti) help much? Not with the current split: after the
changes about 75 % of a frame is CPU work that has no GPU port (vhs / rgbsplit / glitch /
grain looks, dithered gradients, vello rasterization, the swarm splat, the fluid
simulation), and the ported part (glow, blur) already costs only a few hundred ms per frame
on the 1050 Ti, upload and readback included. A faster CPU with more cores (the box has 4 cores
from 2011 and was shared with compile jobs) moves the total more. A new GPU starts to
matter once the digital looks are ported to WGSL and frames stay resident across effects.

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
- Scenes dominated by large glows / blurs (and few digital looks) can try `--gpu` with
  `AGENTFX_GPU_FX=riso,halftone,rays,shadow,blur,glow,drop-shadow`.

## Further opportunities (not done)

- Port vhs / rgbsplit / glitch / grain to the GPU chain (they sit on the full-frame top
  layer, after the glows: one upload, one readback per frame).
- Route blur / glow to the GPU by default when `--gpu` is on and the buffer is large
  (about 40 % of the glow time on the 1050 Ti; needs a pass over the routing tests).
- Anisotropic effect scale: a glow on a layer scaled `sx=110 sy=0.4` uses the largest scale
  in both axes; Chrome blurs in layer space. Matching that would be both faster and closer.
- Frame-level parallelism: one painter renders frames in order; `--segments N` splits the
  video into N segments rendered concurrently (each with its own caches and fluid catch-up).
  With most stages now parallel inside a frame the gain is smaller than before, but
  single-threaded parts remain (display-list encoding, the swarm, cache misses).
- The Reel Script compile of HORIZON takes about 12 s (`nodes_s` 9 s in `reelir.rs`), paid by
  every `agentfx frame` preview.
- A fluid catch-up cache across `agentfx frame` calls.
