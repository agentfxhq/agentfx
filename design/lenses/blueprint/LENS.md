# Technical Blueprint

**Origin.** The blueprint is the cyanotype copy of an engineering drawing: white lines on Prussian blue, the reprographic copy used by engineers from the 1870s on. The look keeps the conventions of the drawing office: a construction grid, line weights by meaning, part numbers, dimension lines with arrowheads, detail callouts, and a title block in the corner.

**Why it works for explainer video.** A blueprint says "this is how it is built", which is exactly what a technical explainer promises. Its conventions are a diagram grammar: a process becomes numbered parts, progress becomes a dimension line, a closer look becomes "Detail A". Charts read as measured drawings: hatched bars, mono ticks, a comparison drawn as a dimension. The grid gives the eye a ruler for judging sizes.

## Rules in numbers (1920x1080)
- Type: Space Grotesk for titles (84 px / 600 / -0.02em) and readable labels (34 to 36 px / 400 to 500); JetBrains Mono caps with +0.12em for every part number, measurement, chip and axis tick (26 to 32 px). Floor 26 px. Ratio 1.333 from a 36 px base.
- Grid: 40 px minor lines at 7 % white, 200 px major lines at 13 % white, a 2 px sheet border inset 40 px.
- Line weights: object outlines 2 px, the active object 3 px, dimension, extension and leader lines 1.5 px, upcoming objects 2 px dashed. Radius 0.
- Margins: 7 % safe area inside the sheet border.
- Color: ground #0e3d73, white #ffffff (10.9:1), pale #b7d3f2 (7.0:1), faint #9cc3ea (5.9:1), cyan #7fe3ff for line work and data (7.4:1), amber #ffd23f as the red pencil (7.5:1). Amber only marks the active part, its callout and the highlighted value.

## Never
1. Filled solid shapes; use outlines and hatching.
2. Rounded corners, shadows, glows, gradients.
3. Proportional type for part numbers or measurements.
4. A dimension line without arrowheads or extension lines.
5. Any hue beyond white, cyan and one amber.
6. Hiding the grid; it is the ruler.
7. Decorative "tech" noise: hexagons, circuit traces, fake code.
8. Things popping or bouncing in.

## Motion tendencies
Everything is drawn, the way a pen would. Outlines draw on as strokes over 800 ms inout-cubic, clockwise from a corner; parts follow 90 ms apart in part-number order. Dimension lines extend from their center to both arrowheads (500 ms out-cubic), then their label fades in over 200 ms. Leader lines run from the part to the callout, and the detail box draws on after them. Hatching fills a bar as it grows from the baseline (700 ms out-cubic). Exits are fast 300 ms in-quad fades. No springs, no scale pops.

Adapted from HyperFrames (Apache-2.0): references/look.md (a look as the rules of one medium; a banned-defaults list).
