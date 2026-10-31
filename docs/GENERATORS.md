# Generators (planned)

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
| Julia set | breathing, liquid fractal; animate `c` | in progress |
| Mandelbrot | infinite zoom; f32 GPU limit about 1e4x | in progress |
| Kaleidoscope (effect) | N-fold mirror symmetry of any layer | planned |
| Tunnel warp (effect) | polar remap, fly down a tube | planned |
| Mandelbulb, KIFS | raymarched 3D fractals | planned |
| Fractal flames | glowing silk, IFS with variations | planned |
| Strange attractors | Lorenz, Clifford, de Jong; 2D density and 3D point clouds | planned |
| Reaction-diffusion | Gray-Scott coral, zebra, spots | planned |
| Domain-warped noise | marble, oil, liquid neon | planned |
| Flow fields | particles streaking through a curl-noise field | planned |
| Truchet, Voronoi | tiled arcs, cells | planned |
| L-systems | growing plants and curves as drawable paths | planned |
| Cellular automata | Life, elementary rules, Lenia-like | planned |
| Penrose tilings | P2/P3 by deflation, as paths | planned |
