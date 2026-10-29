# L0 principles: visual and motion rules for explainer video

Versioned: v0 (2026-10-26). The base layer of the design layer (`../../research/design-layer/CONCEPT.md`,
L0 to L4). Every rule says where it is enforced: **lint** (measurable, `agentfx lint`, codes below),
**critic** (taste, `agentfx critique` asks a vision model against this list), **token** (a default in
`design/motion-tokens.json` or the design spec, so the right thing happens without writing it).

Video is not a web page. A frame is seen once, at speed, often on a phone, while a voice talks. So:
bigger type, fewer words, one idea per frame, full-frame composition, motion that leads the eye.

## Visual

| # | Principle | Number | Where |
|---|---|---|---|
| V1 | **One focal point per beat.** The element the narration names is the one that changes (enters, activates, pulses); everything else holds. | at most 2 emphasized targets per anchor | lint `W_FOCAL`; critic |
| V2 | **Hierarchy by contrast steps.** Each level differs by at least 1.5x in size or a clear weight/color step. | type ratio 1.125 to 1.618 | lint `W_DESIGN_RATIO`; token `type ratio=` |
| V3 | **Type minimums at 1080p.** Reading text >= 28 px (2.6 % of the short side), body 36 to 44 px, labels >= 22 px, display 96 to 200 px. Too many words: cut words, never shrink type. | 28 / 22 px | lint `W_READ_SIZE`, `W_MIN_FONT` (measured, incl. zoom), `W_DESIGN_BODY` |
| V4 | **Text density and reading time.** On-screen words are keywords; the voice carries sentences. A scene shows at most about 1.5 words per second of its length (half the 3 words/s reading rate, because the viewer also listens); one text at most 14 words; display lines under 45 characters. | 3 wps x 0.5; 14 words | lint `W_TEXT_DENSITY`, `W_WORDY`, `I_LINE_LENGTH` |
| V5 | **Contrast.** Text >= 4.5:1 on its effective background (large display >= 3:1); design roles: text >= 7:1, muted >= 4.5:1, accent used as text >= 3:1. | | lint `W_CONTRAST` (static and measured), `W_DESIGN_CONTRAST` |
| V6 | **Safe area.** Content inside 5 to 7 % margins; captions keep their band. | `layout safe=6%` | lint `W_SAFE_AREA`, `E_OUT_OF_FRAME` |
| V7 | **Scale fill.** The hero of a shot fills at least 40 % of the frame's width or height; reading text is not tiny while the frame stays half empty. Fewer, larger elements beat many small ones. (The S1 diagram problem.) | >= 40 %; median text >= 30 px when content covers < 50 % | lint `W_SCALE_FILL` (measured, on the scene's most built-up sample); token `scale diagram=1.35` |
| V8 | **Consistency of treatment per role.** The same role (a heading, a node, a chart) looks the same in every scene; styling belongs in the design spec, not on elements. | | lint `I_ROLE_TREATMENT`; critic |
| V9 | **Color discipline.** Roles, not swatches (bg, surface, text, muted, accent); one accent, rationed to what matters (60/30/10). | | design spec `colors`; critic |
| V10 | **Depth in layers.** Background texture or gradient, mid content, foreground accents; elevation from one shadow recipe. | | design spec `surface`, `elevation`; critic |
| V11 | **Grid and alignment.** Shared alignment edges across scenes; a 12-column or modular grid; asymmetry with balance or a strong center. | | design spec `layout`, `grid`; critic |
| V12 | **Per-step detail.** A diagram that builds shows state: done steps carry a check, the active step a highlight and a detail panel, progress is visible. | | components `done`, `panel`, `progress`; critic |

## Motion

| # | Principle | Number (measured, `MOTION-REPORT.md`) | Where |
|---|---|---|---|
| M1 | **Enter decelerating, exit accelerating, moves ease both ways.** | enter 51 % out-family; exit 30 % in-family; move 75 % inout | token `easing.enter/exit/move` |
| M2 | **Exits are shorter than entrances.** | exit/enter 0.73; enter 0.40 s, exit 0.30 s | token; lint `W_EXIT_SLOW` |
| M3 | **Stagger in the readable range.** Items 60 to 165 ms apart, beats about 0.43 s. | item 133 ms | token `$stagger`, `$beat`; lint `W_STAGGER` |
| M4 | **One orchestrated move per beat.** Group what moves together and move the group; do not start four unrelated moves at one word. | at most 3 moving targets per anchor | lint `W_BEAT_MOVES`; critic |
| M5 | **Overlap, not sequence.** The next element starts while the previous is still settling. | next starts at 19 % (median) of the previous | critic |
| M6 | **Small travel on arrival.** Things arrive in place from close by and leave further. | enter 28 px, scale from 0.90; exits about 120 px | token `roles.enter.distance_px` |
| M7 | **Landed titles hold still.** No breathing, floating or creeping on text that is being read. | | critic |
| M8 | **Motion follows the narration.** A move starts on the word that names its element; reading direction left to right, top to bottom. | | anchors (`@word`); critic |
| M9 | **Holds long enough to read.** 0.5 s + 0.3 s per word for a line on screen; the last line at least 2 s. | | lint `W_TEXT_DENSITY`; critic |
| M10 | **Restraint over loops.** No pulsing, glowing or bouncing as decoration; emphasis is an event, once. | | critic |
| M11 | **Something always moves, and it is big enough to read.** No dead air under narration (> 1 s without motion), motion not confined to a tiny element (>= 8 % of the frame at some point in every span), the film ends on a move rather than a long hold. | 1 s dead air; 8 % area; 2 s end hold | lint `W_COLD_OPEN`, `W_SMALL_MOTION`, `W_DEAD_AIR`, `W_STATIC_END` (motion-energy map, `agentfx lint --motion`) |

## What lint cannot see (the critic's rubric)

The VLM critic (`agentfx critique`, `tools/critique.py`) scores a contact sheet or sampled frames against
V1, V2, V7 to V12 and M4 to M10 and names the defaults that make a video look generated:

- small things floating in a dark void; everything centered; three cards in a row; a KPI grid;
- glow, particles, purple-blue gradients as decoration; glass cards everywhere;
- every element fading and sliding up the same way; titles that keep moving after they land;
- a diagram dropped on screen at once instead of growing with the narration.

Findings are JSON (`{principle, severity, where, problem, fix}`), so an agent can apply them.

## Sources

- `../../research/design-layer/CONCEPT.md` (visual language basics, animation principles).
- Measured numbers: `design/MOTION-REPORT.md` (an in-house product tour + HyperFrames recordings).
- HyperFrames skill references (Apache-2.0, `hyperframes` skill: `references/spec.md`, `motion.md`,
  `review.md`): the 28 px reading floor, the 40 % hero fill, "one hero move per shot", exit/enter ratio
  and stagger ranges were cross-checked against them. Structure adapted, wording ours.
