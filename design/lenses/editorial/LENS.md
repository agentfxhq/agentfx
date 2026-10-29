# Editorial Infographic

**Origin.** This is the look of a newspaper graphics desk: the explanatory charts and step diagrams that run beside a story in a financial daily or a big-city paper. Its medium is the news page, where a graphic has to be read by someone who did not ask for it in under ten seconds.

**Why it works for explainer video.** Editorial graphics are built around a sentence. A red kicker says what kind of thing this is, a serif headline states the finding, a one-line dek gives the unit. The chart proves it with direct labels and one annotation, so narration and frame say the same thing. The serif lends print authority; the sans keeps numbers legible at 1080p. Diagrams become annotated timelines, not boxes and arrows.

## Rules in numbers (1920x1080)
- Type: Source Serif 4 for headlines (90 px / 600 / -0.015em), Libre Franklin for labels, body and numbers (body 34 to 36 px / 400, labels 26 to 28 px / 700 caps +0.08em), JetBrains Mono only for code. Headline number 150 px serif. Floor 26 px.
- Ratio 1.25 from a 34 px base: editorial scales are gentle; the headline does the shouting.
- Margins: 7 % safe area, 6-column grid, 40 px gutter.
- Rules: 3 px ink under headers and on the baseline, 2 px hairlines (#d9d2c5) for gridlines and table rows. Corner radius 3 to 4 px on chips only.
- Color: paper #faf7f2, ink #1a1a1a (16.3:1), muted #57534e (7.1:1), source grey #6f6a62 (5.0:1), steel blue #2f5d8a for the data series (6.4:1), vermilion #b8321f for the story point and the kicker (5.6:1). Vermilion covers one bar, one step, one annotation per frame.

## Never
1. A legend where a direct label fits.
2. 3D, perspective or shadowed bars.
3. More than two hues in one chart.
4. Gridlines at odd values or heavier than the data.
5. Centered headlines or text set over the data.
6. Unsourced numbers: every chart carries a source line.
7. Truncated value axes on bar charts.
8. Decorative icons; the illustration is the data.

## Motion tendencies
Editorial motion explains in order. The headline is already there, or arrives with a 600 ms out-cubic fade and 24 px rise; then the graphic builds. Gridlines and the baseline draw on left to right (500 ms inout-cubic). Bars grow from the baseline 80 ms apart (700 ms out-quart) while their values count up with them. Annotation leader lines draw on (400 ms inout-cubic), the note text fades in only after the line lands. Exits are quiet: 300 ms in-cubic fades. No springs: a chart that wobbles looks unsure of its numbers.

Adapted from HyperFrames (Apache-2.0): references/look.md (a look as the rules of one medium; a banned-defaults list).
