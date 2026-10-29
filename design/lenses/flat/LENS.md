# Flat Vector Explainer

**Origin.** The flat vector explainer grew out of 2010s motion design for YouTube science channels and product explainers: illustration built in vector tools from circles, capsules and rounded rectangles, solid fills, no outlines, on a deep night-sky ground. Its medium is the narrated animated short.

**Why it works for explainer video.** Flat shapes read instantly and survive compression. The dark, saturated ground makes every object a light source, so hierarchy comes from color temperature: cool cyan for what is done, warm yellow for what is happening now, muted indigo for what is still to come. A diagram becomes a row of icon discs a child could follow; a chart becomes chunky rounded bars standing on a ground line, with a speech bubble saying the point. Rounded type keeps technical content friendly.

## Rules in numbers (1920x1080)
- Type: Nunito only (JetBrains Mono for code). Title 88 px / 900 / -0.01em. Labels 34 to 40 px / 800. Body 34 px / 600 to 700. Hero number 150 px / 900. Floor 26 px. Ratio 1.333 from 38 px.
- Shapes: radius 36 px on cards, 18 px on small parts, full capsules for chips and progress. Icon discs 116 px (136 px active) with 60 px icons at 2.4 px stroke.
- Margins: 7 % safe area; illustration may bleed past it, text never.
- Depth: a hard offset shadow (0 14 to 16 px 0, #0f1336, no blur) lifts only the active object.
- Color: ground #141a46 with radial lift #26307a, cards #252d6e, text #ffffff (16.6:1), muted #b9bfe8 (9.2:1), yellow #ffc84a for now (10.8:1), cyan #4fd1e8 for done and data (9.2:1), green #3ddc97 for check badges, coral #ff6f61 in illustrations. Yellow appears on one thing per frame.

## Never
1. Outlines on filled shapes, or hairlines of any kind.
2. Gradients on objects, blurred shadows, glass, glows.
3. Pure black or grey grounds.
4. Sharp corners (below 14 px) on anything but tiny details.
5. Thin, light or condensed type.
6. Photographs or realistic textures mixed with the vectors.
7. Two warm highlights competing in one frame.
8. Linear, robotic motion with no follow-through.

## Motion tendencies
Objects have weight and spring. Discs and cards pop in on spring(260,18): scale from 0.6 with a slight overshoot that settles in about 600 ms, 70 ms apart in reading order. Moves use spring(170,22), so a card that slides also settles. Emphasis is a squash-and-stretch pulse on spring(320,14), once, never a loop. Bars grow from the ground line with overshoot and settle; the speech bubble pops in last from its tail. Exits are quick: 300 ms in-back, a small anticipation then gone.

Adapted from HyperFrames (Apache-2.0): references/look.md (a look as the rules of one medium; a banned-defaults list).
