# Design layer: how to make a Reel Script video look designed

Read this after `SPEC.md`. It adds a design spec (the look), per-step detail components and the
taste rules. Everything here compiles with the same `reel` tools.

## 1. Pick a look (L1 lens) or write a design spec (L2)

One line after `video`:

```
design lens=editorial          // built-in lens (tokens + skins + motion)
design src=design.reel         // your own spec next to the .reel file
```

| Lens | Look | Use for |
|---|---|---|
| `swiss` | paper ground, black type, one red accent, 3 px rules, no radius, numbered step cards | serious explainers, process |
| `editorial` | newspaper graphics desk: serif headline, red kicker, steel-blue data, source lines | data stories |
| `keynote` | near-black, huge type, one blue accent, soft tiles, slow fades | product moments, one idea per frame |
| `flat` | deep night-blue, rounded type (Nunito), warm yellow accent, springy pops | friendly how-it-works |
| `blueprint` | cyanotype, white line work, mono labels, yellow accent, outline tags | technical pipelines, architecture |

Each lens lives in `design/lenses/<name>/` (`LENS.md` essay with numbers, negatives and motion
tendencies; `design.reel` tokens). A spec is a `theme`-style block (it extends the theme tokens):

```
design "My look" base=light|dark
  colors bg=#f2f0eb surface=#ffffff border=#111111 text=#111111 muted=#3d3d3d accent=#e3000f accent2=#111111 mint=#e3000f
  bg #f2f0eb                       // or linear(...) / radial(...)
  type display="Source Serif 4" body="Libre Franklin" base=36 ratio=1.333 display-weight=700 label-case=upper
  layout radius=0 radius-sm=0 safe=7%
  surface style=elevated|flat|outline|glass|tinted border=2
  elevation shadow="0 24 60 #00000040" | none
  icons style=tile|solid|outline|plain|ring
  scale components=1.1 diagram=1.35 chart=1.1
  skin node=tile|step|tag          // HTML/CSS skins for diagram nodes
  motion enter=out-cubic exit=in-quad move=inout-sine enter-dur=0.4s exit-dur=0.3s distance=28 stagger=133ms
  do "..." ; dont "..."
```

Fonts that exist: Inter, Source Serif 4, Libre Franklin, Nunito, JetBrains Mono, Space Grotesk, Fraunces,
Noto Sans Mono. `mint` is the highlight/active color, `accent2` the data/series color.
Lint checks the spec (`E_DESIGN_TOKEN`, `E_DESIGN_FONT`, `W_DESIGN_CONTRAST`).

## 2. Per-step detail (what makes a diagram look finished)

```
progress #prog n=4 label="Schritt"                 // segmented progress + "2 / 4" counter
diagram #d dir=auto gap=70
  node #a "Chat" sub="Wunsch" icon=chat n=01       // n= shows in the `step`/`tag` skins
  ...
panel #det kicker="Details"                        // detail panel; one pane shows at a time
  pane #p1 title="Chat"
    item "Hero-Text ändern" icon=chat
  pane #p2 title="Agent"
    item "index.astro" chip="bearbeitet" status=ok  // status=ok|run|wait|fail -> icon + chip color
chart #rev type=bar data=[..] labels=[..] highlight=4 note="x2 seit Q1" kicker="Umsatz 2025" title="Q4 verdoppelt Q1" source="Beispieldaten"
```

Verbs (inside `@anchor` lines):

| Verb | Does |
|---|---|
| `activate #a` / `deactivate #a` | accent state on a node/card/stat (the step being spoken) |
| `done #a` / `undone #a` | check badge pops on the node, ends its active state |
| `advance #prog to=2` | fills progress segments up to 2, counts |
| `page #det to=p2` | swaps the detail panel to pane `p2` (blur swap) |

Typical step beat: `@Agent done #a ; activate #b ; advance #prog to=2 ; page #det to=p2`.

`node ... skin=tile|step|tag` (or `skin node=...` in the spec) draws the node from an HTML/CSS skin
(`design/skins/node/*.html`) with the spec's tokens; `activate` and `done` work the same.

## 3. Rules lint now checks (L0, `design/PRINCIPLES.md`)

- `W_SCALE_FILL`: the hero fills >= 40 % of the frame; no small elements in a big empty frame.
- `W_FOCAL`: at most 2 emphasized targets per anchor (one focal point per beat).
- `W_BEAT_MOVES`: at most 3 moving targets per anchor (group them or stagger).
- `W_TEXT_DENSITY`, `W_WORDY`: keywords on screen, not sentences (about 1.5 words per second of scene).
- `W_READ_SIZE`: reading text >= 28 px; cut words, never shrink type.
- `W_EXIT_SLOW`, `W_STAGGER`: exits shorter than entrances; staggers 30 to 500 ms (items ~133 ms).
- `I_ROLE_TREATMENT`: same role, same styling across scenes (put styling in the spec).
- Taste lint (`agentfx lint --motion`, from the motion-energy map): `W_COLD_OPEN` (motion on frame 0), `W_SMALL_MOTION`
  (a span whose motion stays under 8 % of the frame: make the mover bigger or move the camera), `W_DEAD_AIR`
  (nothing moves for > 1 s under narration: anchor a beat to a word), `W_STATIC_END` (the film ends still or on a
  small element). `agentfx energy <file>` prints the energy strip; `agentfx sheet --energy` adds a bar per thumbnail.

Motion defaults are measured from well-made videos (`design/MOTION-REPORT.md`): enter 0.4 s out-cubic
with 28 px travel, exit 0.3 s in-quad, moves 0.63 s inout. You rarely need `dur=`/`ease=`.

## 4. Optional workflow steps (only when your task says so)

**Style frames first, then harvest (arm C).** Draw 2 to 3 still frames of the video as HTML/CSS, each in a
different direction. Frame contract (the renderer is Blitz, a native HTML/CSS engine, no browser):

- one complete page, exactly 1920x1080: `html, body {margin:0; width:1920px; height:1080px; overflow:hidden}`;
- allowed: flexbox, CSS grid, absolute positioning, borders, border-radius, box-shadow, linear/radial
  gradients, inline SVG, letter-spacing, text-transform, font-variant-numeric, CSS custom properties;
- not allowed: JavaScript, external URLs or web fonts, remote images, filters except one `blur()`,
  backdrop-filter, mix-blend-mode, clip-path, `@keyframes`, transitions, `vw/vh`;
- fonts: only the families listed in section 1; reading text >= 28 px; 6 to 7 % margins.

Examples: `design/lenses/<lens>/frame-diagram.html` and `frame-data.html` for every lens. Render them as one sheet,
pick one, harvest its look into a spec, then build the video with `design src=design.reel`:

```
tools/agent-design.sh frames <dir-with-frame-*.html>     # -> <dir>/pitch-sheet.png (look at it)
tools/agent-design.sh harvest <frame.html> -o design.reel  # computed styles -> design spec
```

Edit the harvested spec if needed (it never invents values; add `skin`, `scale`, `motion` lines).
`layout margin=7%` and `safe=7%` are the same token. A chart's `h=` is the plot area; `title=`/`kicker=`/
`source=` add their lines above and below it. Charts follow the spec: with `surface style=flat|outline` or
`elevation shadow=none` bars are solid, without gradient or glow. Counters group thousands from 10 000 on
(`12.480`; `sep=none` or `sep=,` on `counter`/`stat`).

**Critic loop (arm D).** After a sheet, ask the vision critic, fix its P0/P1 findings, re-check:

```
tools/agent-design.sh critique out-sheet.png --brief "..." [--prev critique-1.json] -o critique-2.json
```

It returns JSON findings (`principle, severity, where, problem, fix`) and 1 to 5 scores against the rubric.
2 to 3 rounds; stop when no P0 is left.
