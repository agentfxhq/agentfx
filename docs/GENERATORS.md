# Generators

A new layer type, `gen`, draws procedural imagery: fractals, noise fields, tilings, simulations.
A generator layer behaves like any other layer: it can be positioned, masked, animated, blended
and run through every look and time effect.

Every generator has:

- a CPU reference and a GPU (wgpu) path that agree within 1/255 per channel;
- animatable numeric parameters (`anim #g zoom 0s:1 8s:400`);
- a `seed=` where randomness is involved, so renders are deterministic;
- an example in `examples/features/`, a catalog entry and a section in `SPEC.md`.

## Backlog

| Generator | Look | Status |
|---|---|---|
| Julia set | breathing, liquid fractal; animate `c` | done (`kind=julia`) |
| Mandelbrot | infinite zoom; f32 GPU limit about 2e4x, CPU f64 | done (`kind=mandelbrot`) |
| Kaleidoscope (effect) | N-fold mirror symmetry of any layer | planned |
| Tunnel warp (effect) | polar remap, fly down a tube | planned |
| Mandelbulb, KIFS | raymarched 3D fractals | planned |
| Fractal flames | glowing silk, IFS with variations | planned |
| Strange attractors | Lorenz, Clifford, de Jong; 2D density and 3D point clouds | planned |
| Reaction-diffusion | Gray-Scott coral, zebra, spots | planned |
| Domain-warped noise | marble, oil, liquid neon | done (`kind=noise`) |
| Flow fields | particles streaking through a curl-noise field | done (`kind=flow`, CPU) |
| Truchet, Voronoi | tiled arcs, cells | planned |
| L-systems | growing plants and curves as drawable paths | planned |
| Cellular automata | Life, elementary rules, Lenia-like | planned |
| Penrose tilings | P2/P3 by deflation, as paths | planned |

## How to add a generator

A kind is a pixel function `sample(u, v) -> straight RGBA` over unit space (origin at the rect
centre, v from -1 at the top to 1 at the bottom, u scaled like v). The painter maps device pixels
into unit space through the layer transform, supersamples (`aa`), premultiplies and dithers.

1. **Declare it**: one `Kind { name, doc, params, gpu, gpu_zoom_max, time }` entry in `KINDS` in
   `crates/ir/src/generator.rs`. The parameter list (name, default, min, max, doc) drives IR
   validation, Reel Script parsing, the `gen.<param>` tracks, lint and the catalog. Parameter
   names must not collide with existing channels (`x y rot scale angle offset hue ...`, see
   `Ch::parse` in `crates/script/src/ir.rs`): a colliding name parses as that channel instead.
   Set `time: true` when the image moves with t itself (noise flow, simulations): the planner
   then redraws it every frame.
2. **Implement it**: `crates/render/src/generator/<kind>.rs` with a unit struct implementing
   `Generator` (`sample`, and `wgsl` when there is a kernel). Read parameters as `g.p[i]` in
   declaration order (name the indices as consts, see `julia.rs`). Put centre / zoom / rotation
   into `view` (an affine from unit space to your space, composed on the host in f64): `sample`
   then gets your coordinates and needs no transcendental view math, which keeps CPU and GPU
   identical. In `g.f32` mode compute in f32 like the kernel (the `Real` trait makes one generic
   function serve both; use `Real::fma` where the kernel uses WGSL `fma`, because drivers fuse
   `a * b + c` on their own and chaotic iterations amplify that last bit). Use `g.seed` for
   randomness and `g.t` for time.
3. **GPU kernel** (optional): `crates/render/src/generator/<kind>.wgsl` defining
   `fn sample(u: f32, v: f32) -> vec4<f32>`, parameters via `gp(i)`; return it from `wgsl()` as
   `concat!(include_str!("common.wgsl"), include_str!("<kind>.wgsl"))`. Helpers in
   `generator/common.wgsl` (palette, escape-time, view mapping) and `gpu/common.wgsl` (hash,
   noise). Without a kernel set `gpu: false`: the kind always runs on the CPU.
4. **Register it**: one line in `lookup()` in `crates/render/src/generator/mod.rs` and a
   `pub mod <kind>;` there.
5. **Vector or grid kinds** (L-systems, Penrose, cellular automata): keep `sample` trivial and
   override `cpu(&self, g, out)` to rasterize your geometry into the premultiplied RGBA8 buffer
   (`g.w x g.h`; unit -> pixel is the inverse of `g.m`, e.g. with tiny-skia, already a
   dependency). `gpu: false`, no `.wgsl`.
6. **Tests**: `registry_matches_declarations`, `deterministic` and `gpu_matches_cpu` in
   `generator/mod.rs` cover every kind once it is declared and registered; add a few
   parameter cases of yours to the `cases` table in `gpu_matches_cpu` (they must stay within
   1/255). Run `cargo test --release -p agentfx-render generator`.
7. **Docs and example**: a row in the kinds list of `SPEC.md` "Generators" and of `IR.md`
   "Generator", a catalog item (`crates/script/src/catalog.rs`, then regenerate the SPEC table
   with `agentfx catalog --md`), a row in `skill/agentfx/animation-kinds.md`, an example in
   `examples/features/`, and mark it done above.
