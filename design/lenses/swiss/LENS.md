# Swiss International

**Origin.** The International Typographic Style grew out of Zurich and Basel in the 1950s: Müller-Brockmann's concert posters, Ruder's grids, Neue Grafik. Its medium is the printed poster and the transit sign, read at distance and in passing. Type is the image, the grid is the argument.

**Why it works for explainer video.** A 1080p frame watched for four seconds behaves like a poster seen from a tram. Swiss hierarchy is built from size and weight alone, so it survives compression, small screens and a viewer who looks away. A diagram becomes a row of ruled boxes on a column grid; a chart becomes black bars with the one that matters in red.

## Rules in numbers (1920x1080)
- Type: Inter only (JetBrains Mono for code). Display 108 px / 800 / tracking -0.045em / line height 0.95. Hero numerals 200 to 240 px. Labels 28 px / 700 / caps / +0.06em. Body 38 to 44 px / 450. Floor 26 px.
- Scale ratio 1.414 from a 40 px base.
- Margins: 7 % safe area (134 px sides, 76 px top and bottom). 12-column grid, 40 px gutter. Everything flush left on a column edge.
- Rules: 3 px black for structure, 2 px for table rows. No corner radius anywhere.
- Color: warm paper #f2f0eb, ink #111111 (16.6:1), muted #3d3d3d (9.5:1), one red #e3000f (4.3:1, used at 28 px bold and up). Red appears on exactly one element per frame: the active step, the highlighted bar, the kicker that names it.

## Never
1. Rounded corners, pills or circles as containers.
2. Drop shadows, glows, gradients, glass.
3. Centered titles or symmetrical layouts.
4. A second accent color, or red on more than one idea.
5. Gridlines and legends on charts; label the bars directly.
6. Icons as decoration; an icon must name the thing next to it.
7. Light weights for headlines, or tracking-out on large type.
8. Springs, overshoot, bounce.

## Motion tendencies
Motion is mechanical: things wipe, slide or cut, they never pop. Elements enter with a 450 ms out-expo slide of 40 to 80 px along the grid axis, or a hard-edged wipe revealing them left to right. Exits take 250 ms in-quart, or are hard cuts on the beat. Moves between grid positions use 600 ms inout-quart. Stagger rows and columns 50 ms apart, in reading order. Rules draw on at constant speed (linear, 300 ms) before the content they separate arrives. The red highlight changes state with a cut, not a fade, so the eye jumps with it. Numbers do not count up; they appear complete, because a poster does not animate its figures.

Adapted from HyperFrames (Apache-2.0): references/look.md (a look as the rules of one medium; a banned-defaults list).
