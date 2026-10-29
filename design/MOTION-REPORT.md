# Motion tokens v0: what the recordings say (2026-10-26)

`agentfx motion mine` (`tools/motion_mine.py`) over recorder output. The recorder (`../recorder`, run, not
edited) seeks each composition frame by frame and writes composite tracks (translate / scale / rotate /
opacity) with fitted easings (`model.json`) and neutral element rects (`probe.json`, via
`lower.mjs --plan-only`). The miner samples every track per frame, cuts it into moves, classifies each
move, fits its normalized progress against 36 named curves (CSS/GSAP families, Material 3, springs by
damping ratio) and aggregates. Raw data (every move with its fit) comes from `agentfx motion mine --json`.

## Sources

| Source | Kind | Frames | Moves |
|---|---|---:|---:|
| An in-house product tour (Remotion) | Remotion | 2333 | 132 |
| HyperFrames `figma-elementor` example | GSAP | 210 | 8 |
| 14 HyperFrames registry blocks (`npx hyperframes add`): ai-answer-rank, bar-chart-race, code-diff, data-chart, flowchart, hw-pipeline, instagram-follow, logo-outro, lower-third-bild, lt-accent-underline, lt-kicker-name, lt-mask-reveal, lt-stack-bars, macos-notification | GSAP | 135 to 450 each | 152 |

292 moves in total, 257 (88 %) fit a named curve with RMSE < 0.06 of the progress.

## Durations

| Move | n | p25 s | median s | p75 s |
|---|---:|---:|---:|---:|
| enter | 116 | 0.30 | **0.40** | 0.47 |
| exit | 60 | 0.20 | **0.30** | 0.43 |
| move (re-position, camera) | 90 | 0.20 | **0.47** | 1.37 |
| emphasis (pulse, nudge) | 26 | 0.23 | **0.27** | 0.37 |
| enter, translate | 30 | 0.30 | 0.43 | 0.47 |
| enter, opacity | 79 | 0.30 | 0.40 | 0.47 |
| exit, translate | 25 | 0.23 | 0.27 | 0.43 |
| move, translate | 68 | 0.20 | 0.63 | 1.70 |
| move, scale (zoom) | 19 | 0.40 | 0.43 | 0.43 |

- **Exits are shorter than entrances**: per element with both, exit/enter = **0.73** median (p25 0.67,
  p75 1.0). HyperFrames' own guide says 40 to 50 % shorter; the recordings are less extreme.
- **"Larger elements move slower" is not in this data.** By element area: small (< 1 % of the frame)
  0.43 s, medium 0.43 s, large (>= 10 %) **0.33 s**. Large blocks here are panels and cards that fade in,
  small ones include the cursor and chart bars that travel far. Kept as a rubric question, not a token.
- By role: text 0.37 s, blocks 0.33 s, small parts 0.60 s, full-stage elements 0.47 s.

## Distances

- Enter travel: **28 px** median (p25 14, p75 40) on a 1080p frame. S1 used 40 px for `rise` and 90 px
  for `left/right`; both were above the p75.
- Enter scale: from **0.90** (p25 0.85). S1's `pop` starts at 0.6.
- Exits travel further (median 120 px) than entrances: things leave the frame, they arrive in place.

## Easing families

| Move | n fitted | out (decelerate) | inout | in (accelerate) | linear | spring/back |
|---|---:|---:|---:|---:|---:|---:|
| enter | 109 | **51 %** | 22 % | 0 | 12 % | 10 % spring + 12 % out-back |
| exit | 53 | 13 % | 28 % | **30 %** (+6 % M3 accelerate) | 23 % | 0 |
| move | 71 | 1 % | **75 %** | 3 % | 20 % | 1 % |
| emphasis | 24 | 33 % | 12 % | 0 | 54 % | 0 |

Most frequent named curves: enter `out-cubic` (31), `inout-cubic` (14, mostly the product tour: Remotion's
`interpolate` with `Easing.inOut`), `out-back` (13), `linear` (13, opacity fades); exit `linear` (12),
`inout-cubic` (9), `in-quad` (8); move `inout-sine` (25), `inout-quad` (18), `linear` (14).

The rule "enter decelerating, exit accelerating, moves ease both ways" holds as a majority, not a law.
Per source the house style shows: data-chart enters everything with `out-cubic` (16 of 16);
ai-answer-rank and bar-chart-race pop with `out-back`; lower thirds use springs (damping ratio 0.6 to 1.0);
our product tour eases enters both ways (a Remotion habit that reads softer).

## Choreography

- **Stagger**: start-to-start of consecutive enters within 0.5 s is bimodal: **item staggers 67 to 165 ms**
  (median 133 ms; list rows, bars, letters) and **beat spacing 0.4 to 0.5 s** (the next idea).
- **Overlap**: when consecutive enters overlap, the next one starts at **19 %** of the previous one's
  duration (median; p75 59 %). A quarter of consecutive pairs start together. HyperFrames' guide says
  30 to 50 %.

## Tokens (design/motion-tokens.json)

M3-style named durations and easings, plus roles that the story verbs use by default (S1 defaults in
brackets):

| Token | Value | Used by |
|---|---|---|
| `duration.short1..short4` | 0.10, 0.167, 0.267, 0.30 s | emphasis = short3, exit = short4 |
| `duration.medium1..medium4` | 0.40, 0.467, 0.633, 0.80 s | enter = medium1 [0.5], move = medium3 [0.7] |
| `duration.long1, long2` | 1.0, 1.4 s | counters, draw-on |
| `easing.enter` | `out-cubic` | `enter`/`in=` fade, rise, drop, left, right, zoom, blur [out-cubic] |
| `easing.enter-pop` | `out-back` | available as `$enter-pop` |
| `easing.exit` | `in-quad` | `exit`/`out=` [cubic(0.32, 0, 0.67, 0)] |
| `easing.move` | `inout-sine` | `move` [inout-cubic] |
| `easing.emphasized-decelerate/-accelerate` | M3 curves | available as `$emphasized-decelerate` |
| `roles.enter.distance_px` | 28 | rise/drop travel (left/right 2.5x) [40 / 90] |
| `roles.stagger.item / beat` | 0.133 s / 0.43 s | `$stagger`, `$beat` |

A design spec overrides them per lens: `motion enter=out-expo exit=in-quart enter-dur=0.45s exit-dur=0.25s
move-dur=0.7s distance=40 stagger=50ms`.

## Limits

- Only composite tracks are mined. Clip-path wipes, width/height growth and SVG stroke draws are discrete
  switches in the recording, so lower-third-bild and lt-stack-bars contributed 0 moves, and wipe/draw
  timings are not measured yet.
- The product tour contributes 45 % of the moves; it is one video with one author's habits.
- The role guess (text / small / block / stage) is a heuristic from the union DOM and the probe rect.
- 35 moves (12 %) did not fit any named curve below RMSE 0.06 (multi-segment GSAP timelines, interrupted
  tweens); they count in the durations, not in the easing families.
