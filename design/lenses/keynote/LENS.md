# Keynote Minimal

**Origin.** The product-launch keynote as staged by the large consumer-tech companies since the 2000s: a dark auditorium, a screen the size of a building, one sentence or one number per slide, and a presenter who waits for it to land. Its medium is projection to a seated crowd.

**Why it works for explainer video.** Explainer narration is a presenter's voice, so the keynote's division of labor fits: the voice carries the reasoning, the frame carries one idea at a scale nobody can miss. Near-black grounds make 1080p compression invisible and let type and a single blue glow. Diagrams are reduced to soft tiles in a row with one lit; charts lose axes and gridlines and keep only proportion, the values and the one bar that matters. When only one thing is blue, nothing else needs to be small.

## Rules in numbers (1920x1080)
- Type: Inter only. Title 72 to 96 px / 700 / -0.035em / line height 1.02. Hero number 220 to 240 px / 700 / -0.05em. Labels 28 to 36 px / 500 to 600, sentence case, no tracking. Floor 26 px. Ratio 1.5 from 40 px.
- Margins: 7 % safe area, but content sits in the middle 75 %; at least 15 % of the frame is empty around the subject.
- Surfaces: tiles #1c1c1e, radius 28 px (14 px for small parts), no borders except the 4 px blue ring on the active tile. No rules.
- Color: ground #0b0b0c with a faint radial lift (#1d1d20 at 50 % 35 %), text #f5f5f7 (18.1:1), grey #a1a1a6 (7.7:1), quiet grey #86868b (5.4:1), blue #2997ff (6.5:1). Blue marks one thing per frame. Chart bars are #48484a.

## Never
1. Bullet lists or more than one sentence on screen.
2. Borders, rules, table lines, dividers.
3. A second accent color, or blue used for decoration.
4. Small type to fit more in; split the frame instead.
5. Busy backgrounds, photos behind text, patterns.
6. Axis lines, tick labels and legends on charts.
7. Anything moving while the viewer reads.
8. Bounce, overshoot or snappy cuts.

## Motion tendencies
Slow, confident, then still. Titles fade in over 900 ms out-cubic with no more than 16 px of rise. Once landed they hold dead still for the full read, often 3 to 5 s. Items in a row arrive 120 ms apart. Moves between states take 1100 ms inout-cubic: the lit tile hands its ring to the next tile, the progress line extends at the same speed. Bars rise together over 900 ms out-quart; the highlight turns blue only after they settle. Exits are 500 ms inout-sine fades to black.

Adapted from HyperFrames (Apache-2.0): references/look.md (a look as the rules of one medium; a banned-defaults list); references/motion.md (a landed title holds dead still).
