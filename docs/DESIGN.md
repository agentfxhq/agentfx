# reel — a change-first HTML/CSS renderer for video

Working title. Experiment started 2026-10-25.

## Premise

Browsers render for an interactive screen: time is a wall clock, the future is
unknown, every frame is worth drawing. Video is the opposite: time is an input,
the whole timeline is known before the first frame, and most frames repeat the
one before (65 % of the frames of an 80 s product tour change by less than 0.05 gray levels, only
11 % change a lot).

reel treats a video as **one document plus a timeline** and plans all work
ahead of time. Pixels are produced only where and when something changes.

## Principles

0. **The IR is the centre.** Frontends (HTML mode, direct mode / Reel Script) compile
   to one IR (`IR.md`): a node tree with property tracks, resolved display lists, text,
   paths, effects. Everything after it (plan, raster, composite, encode) only sees the
   IR. HTML is one frontend, not the model.
1. **Time is an input.** Document = HTML + CSS + timeline (or IR). Output at frame n is
   a pure function `f(n)`. No clocks, no event loop, no async loading
   (fonts/images resolved before frame 0).
2. **Plan before painting.** The timeline is compiled into property tracks.
   Each track is classified by the stage it invalidates:
   - composite: `transform`, `opacity`
   - paint: colors, backgrounds, shadows, `filter`
   - layout: size, position in flow, font, text content
   - effect (IR): effect parameters, path trim, per-glyph tracks, motion blur — redrawn
     per frame where they change, with exact damage (principle 4).
3. **Layers come from the timeline, not heuristics.** A subtree whose animated
   properties over its whole life are composite-only becomes a layer. Its content is
   compiled once (a native display list). Pixels are cached **at every scale/offset a
   layer rests at** (exact raster, bit-identical to drawing it fresh; rests are most
   frames), while moving frames draw the vectors at the exact fractional position
   (text slides like Chrome re-rasterizes it). Everything static merges into as few
   layers as possible.
4. **Damage is computed, not detected — and quantized.** Dirty rect per frame = union
   of the before/after *ink* of every content node whose state changes at that frame.
   A transform/opacity change is committed only when it moves the subtree ≥ 1/16 px on
   screen or changes opacity ≥ 1/255, measured against the last *committed* value, and
   frames draw committed values (so damage and pixels agree). Ink comes from the
   evaluated content bounds: shadows, blur and glow grow it by their evaluated radius
   (3σ) and offsets, motion blur by the swept samples. An empty rect means a repeated
   frame; no pixel diffing anywhere.
5. **Escalate locally — with a containment test.** Paint changes re-paint one layer.
   Layout changes re-lay out the smallest containing subtree: the repaint root is the
   element itself if absolutely positioned (a `left/top` change is treated as a
   translate), else its parent, and it is valid only if the root's own box does not
   change in any state (checked per state with the native layout; otherwise the scene
   fails loudly or escalates further).
6. **The encoder gets the plan too.** Repeated frames are never re-encoded (VFR, with
   B-frames). Full-damage frames are cut points: segments render and encode in
   parallel and are concatenated with `-c copy`. Encoder work (preset, threads, colour
   conversion in Rust) is planned with the renderer, not after it: after M0 x264 was
   the largest cost.
7. **Chrome stays the reference — measured carefully.** The same HTML file plays in
   Chrome (Web Animations API seeking), so there is always a ground-truth frame.
   Fidelity target: SSIM ≥ 0.98 per frame, text included, measured **before encode**
   (lossless captures of both renderers; crf 18 + yuv420p alone drops a fast zoom frame
   to 0.83 against its own source). Chrome's reference has quirks (first-frame seek,
   its own filter for transformed images), so keep a "static Chrome" control (animations
   frozen as inline styles) to tell reel errors from Chrome artefacts.

## Authoring format (v0)

- HTML + CSS, CSS `@keyframes` + `animation-*` (durations, delays, fill modes,
  `cubic-bezier`, `steps`). Contract: `INPUT.md`.
- Escape hatch: a `tracks.json` for values CSS can't express well (typed text,
  counters): `{selector, property, keys: [{t, v, ease}]}`.
- Direct mode: the IR itself (`IR.md`), including what CSS can't express (springs,
  per-glyph tracks, mattes, path draw-on, motion blur, animatable blur/glow).
- A **track recorder** (a separate tool, not part of this repository) runs a browser
  animation in Chrome without painting and emits this HTML-mode input.

## Pipeline

```
frontends:  HTML/CSS + timeline  --css.rs + Blitz (Stylo/Taffy/Parley)-->  IR  <--  direct mode / Reel Script
            (geometry, layers, units, repaint states; native display lists per unit/state)
IR  -> schedule (all frames): committed states, signatures, ink, damage, cut points
       (planner v2: only nodes inside their activity intervals are re-evaluated;
        damage merged with an R-tree; AGENTFX_PLANNER=v1 = old path)
    -> segments in parallel:  per frame, per damaged rect:
         vello_cpu paint of the IR tree (vectors at exact positions; static content
         from rest-transform raster cache; effects/motion blur offscreen)
       -> I420/NV12 (BT.709) in Rust -> VAAPI (Quick Sync) or x264 (VFR, B-frames)
    -> concat segments (-c copy)
```

Chrome is used only by `ref/render-ref.mjs` (reference frames for SSIM).

M2 frontend (default): `css.rs` (tracks; inline animations, individual transform
properties, discrete holds) -> `plan2.rs` (change frames -> Blitz probe paints with item
attribution -> layers/groups/regions/units -> IR). The M1 planner (`html.rs`) remains as
`--planner m1`.

## Milestones

- **M0, architecture proof (stack-independent).**
  - Test scenes in HTML + CSS keyframes, modeled on a product-update screencast.
  - Chrome reference renderer.
  - Timeline compiler with the damage schedule.
  - Rust compositor + encoder.
  - Layers are rasterized by Chrome, once each.
  - Measures speed and SSIM end to end.
- **M1, native raster** (done 2026-10-25): Blitz replaces Chrome in
  the render path (layout + paint per unit/state into IR display lists), vello_cpu
  paints damaged rects, native text/path/effects/motion blur in the IR, VFR with
  B-frames, parallel segments.
- **M2, recorded scenes natively** (done 2026-10-26): the *probe
  planner* (`compiler/src/plan2.rs`) replaces the M1 layer planner as the default HTML
  frontend. Every frame where a non-composite value changes is a change frame; Blitz
  lays out and paints the whole document there once (composites neutral), each
  display-list item attributed to its element. From these probes (no pixels) come the
  IR: layers (composite tracks, one node per individual transform property), groups
  (effects around layers), regions (local escalation: LCA of the elements whose paint
  changed), paint order (Blitz's stacking contexts, merged over all probes) and
  per-unit `Switch` states. A recorded product tour (2333 frames, 345 change frames,
  852 elements) compiles in ~1.3 s. Containment is checked by construction: damage is
  the before/after ink of exactly the units whose paint changed.
- **M3, encoder + fidelity** (done 2026-10-26): hardware H.264 via Intel
  Quick Sync / VAAPI (NV12 from reel's own AVX2 converter, `sg render` fallback, per-encoder
  B-frame tail rule), BT.709 output, Skia text gamma, fractional line tops, dithered
  gradients, M2 review fixes. VFR, skip frames and parallel segments carried over from
  M1/M2; damage/motion hints to the encoder are not done (no API in VAAPI/x264 CLI).
- **M4, track recorder:**
  - Phase 1: HyperFrames projects (HTML + GSAP, Apache-2.0; same input shape as reel).
  - Phase 2: React-driven compositions (React per frame).

  The recorder runs the composition once in Chrome without screenshots, records
  the per-frame style/DOM changes as tracks, and reel renders natively. Each
  project is validated automatically: sample frames are diffed against Chrome,
  and layers or segments below threshold fall back to Chrome capture.

## M5: native JavaScript

Rule: **JS drives the DOM, reel draws the pixels.**

- **Engine:** embedded, with a fully virtual clock.
  - Start with QuickJS (rquickjs): small, deterministic, fine for timeline logic.
  - Switch to V8 (deno_core) only if per-frame JS gets too slow, e.g. for React.
- **Determinism:**
  - `Date.now`, `performance.now` and `requestAnimationFrame` are tied to frame time.
  - `Math.random` is seeded.
  - No network; fonts and images are preloaded.
- **DOM shim over reel's DOM:**
  - the APIs GSAP and typical scripts need: `element.style`, `classList`, `textContent`/`innerHTML`, attributes, `getComputedStyle`, `getBoundingClientRect` (served from our layout), `querySelector*`.
  - Grow it by test corpus (GSAP first), never speculatively.
- **Change-first still holds:**
  - Every style or DOM write is recorded per property.
  - The first pass runs JS for all frames without painting, which costs milliseconds.
  - Those recorded mutations become the timeline, and planning works as before.
- **React:** a custom React reconciler that targets the renderer's DOM directly, instead of emulating react-dom's full browser surface (planned, not started).

## M6: native video

- **Decode:** sequential, never per-frame seeking.
  - Start by piping raw frames from ffmpeg.
  - Later, VAAPI hardware decode (needs membership in the render group).
- **Layers:** a `<video>` is a layer whose content changes every frame. Damage is its rect, and transforms and opacity composite as usual.
- **Smart passthrough:** segments where a video plays full-frame, unscaled and with nothing over it are not decoded and re-encoded. Packets are stream-copied, as editing programs do.
- **Audio:** all audio comes from the timeline, mixed by ffmpeg with `filter_complex`.

## Non-goals (for now)

Interactivity, 3D transforms, full CSS coverage. (JS and `<video>` were
non-goals for v0; they are now planned as M5/M6, see below.) Unsupported features must fail loudly
(or fall back to Chrome for that layer), never render wrong silently.
