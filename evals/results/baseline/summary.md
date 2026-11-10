# Eval run `baseline`

Model sonnet, judge sonnet, mode both, max turns 40, commit ee18d22, wall 26.3 min.

| Metric | Value |
|---|---|
| cases | 17 |
| score | 79.04 |
| accept_rate | 0.353 |
| judge_accept | 0.353 |
| quality | 5.29 |
| checklist_pass | 0.882 |
| lint_clean | 1.0 |
| renders | 0.882 |
| duration_ok | 0.882 |
| text_ok | 0.941 |
| turns | 11.8 |
| lint_calls | 3.12 |
| first_lint_errors | 2.94 |
| lint_errors_seen | 3.47 |
| agent_cost_usd | 2.2 |
| judge_cost_usd | 0.49 |
| agent_wall_s | 255.86 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 44, `E_ID` 3, `E_PAINT` 3, `E_VERB` 3, `E_PRESET` 2, `E_EVENT_OUTSIDE` 2, `E_VALUE` 2

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| data-budget-donut | 64.5 | 4 | no | 3/4 | ok | ok | {'16:9': 0.5} | 4/4 | 12 | 3 | 0 | 0.12 | The chart does not animate. It is fully drawn from the first frame, and only the title fades in.; The video is only about 0.5s long, which is far too short to read or watch.; The legend text is tiny and hard to read at 1 |
| data-downloads-counter | 86.7 | 6 | no | 4/4 | ok | ok | ok | 1/1 | 11 | 2 | 0 | 0.15 | The final number shows "10.000" with a period, which reads as a decimal (10.0) to English-speaking LinkedIn viewers. During the count it shows 2073 and 9991 with no separators, so the formatting is inconsistent. It shoul |
| data-q3-kpis | 76.7 | 5 | no | 4/4 | ok | ok | ok | 1/3 | 11 | 2 | 0 | 0.14 | The cards are small and sit in the middle of a mostly empty 1080p frame. The numbers are too small to read from the back of an all-hands room.; Revenue reads '$1200K' instead of '$1.2M', and customers reads '4300' instea |
| data-signups-bar | 83.3 | 5 | no | 5/5 | ok | ok | ok | 2/2 | 11 | 2 | 6 | 0.10 | The chart fills only about 45% of the frame width and is small below a big title. Axis labels, month labels and value labels are tiny and hard to read on a phone, and the frame has large empty margins.; The June highligh |
| edit-add-vertical | 86.7 | 6 | yes | 3/3 | ok | ok | ok | 2/2 | 14 | 4 | 0 | 0.15 | Vertical layout leaves the bottom ~50% of the frame empty; the content sits in the upper-middle, so the frame is poorly used.; Subline is small in vertical (about 44px on 1080 width), so it is hard to read on a phone.; V |
| edit-brand-colors-hold | 83.3 | 5 | yes | 3/3 | ok | ok | ok | 2/2 | 6 | 2 | 1 | 0.07 | The 'Q2 2027' subtitle is small and plain off-white. It has weak hierarchy and doesn't use the brand yellow.; The design is generic: flat blue with centered text and no brand element or accent.; The outro is a static tex |
| edit-fix-broken | 83.3 | 5 | yes | 4/4 | ok | ok | ok | 2/2 | 10 | 3 | 6 | 0.11 | The title splits onto two lines (BEAN / BOX) and the tagline is small relative to the 1080 wide frame, so the hierarchy is weak.; The top and bottom of the frame are empty. All content sits in the middle band, and the ti |
| edit-title-bigger-slower | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 16 | 3 | 1 | 0.15 | The title wraps to two lines, because nowrap was removed. A single line may have been expected.; The date is tiny next to the big title, so it is hard to read and the hierarchy gap is too large.; The motion stops after a |
| explainer-request-diagram | 71.3 | 5 | no | 4/5 | ok | FAIL | ok | 3/3 | None | 4 | 0 | 0.00 | Step captions overlap at ~10.6s (3 and 4 collide into garbled text).; Diagram is small in the frame, with huge empty areas above and below. Node text and subtitles are tiny.; Step captions at the bottom are very small an |
| explainer-water-tiktok-captions | 61.8 | 5 | no | 3/4 | ok | FAIL | {'9:16': 17.87} | 1/1 | 13 | 3 | 0 | 0.17 | Captions show 2–3 words at a time on two lines rather than true word-by-word, and they're only moderately sized for a TikTok look.; The visuals are very bare: only a flat blue circle on a dark gradient, with no water ima |
| fx-fluid-ink-dream | 86.7 | 6 | no | 3/3 | ok | ok | ok | 1/1 | 8 | 1 | 0 | 0.10 | The ink spreads over the whole frame and ends as a muddy grey-blue mass, with no payoff or clean end state. The word is lost early and nothing resolves.; Visible banding or striped artifacts in the ink near 3.2–3.8s look |
| fx-glitch-system-failure | 73.3 | 5 | no | 2/3 | ok | ok | ok | 1/1 | None | 8 | 0 | 0.00 | The broken-TV look is weak. The background is almost pure black with no visible scanlines, static, tracking bars or screen distortion.; The subtitle 'ERR 0x0000F4 // SIGNAL LOST' is tiny, dim and low-contrast, and is oft |
| kinetic-stop-scrolling | 65.3 | 5 | no | 2/5 | ok | ok | ok | 2/2 | 15 | 3 | 0 | 0.23 | The second words (SCROLLING., BUILDING.) are much smaller than the first words. This breaks the 'big bold words' brief and makes the hierarchy uneven.; The text sits in a middle band with empty black or green space above |
| logo-reveal-svg | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 0/0 | 14 | 4 | 3 | 0.17 | The reveal is over by about 1.5s and the last ~3s are a static hold. The sweep highlight is barely visible, so the middle feels empty.; The expanding ring is off-centre relative to the final lockup. At 1.04s it is croppe |
| music-visualizer-night-drive | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 1/1 | 11 | 2 | 33 | 0.14 | The title sits very close to the sun and the sun's top edge nearly touches it. The layout feels slightly crowded in the middle and empty on the sides.; Nothing says 'drive': there is no road, car, motion or parallax. The |
| short-pushups | 83.3 | 5 | no | 4/4 | ok | ok | ok | 4/4 | 13 | 5 | 0 | 0.24 | The bottom 40–50% of the frame is empty in the hook and outro. Content clusters in the upper half and feels unbalanced.; The diagrams are abstract, just a dot, a line and a bar. They don't show a person or a push-up, so  |
| teaser-nordlicht | 77.3 | 5 | no | 4/5 | ok | ok | ok | 3/3 | 12 | 2 | 0 | 0.16 | There is no product imagery, UI, icon or logo mark. It is only text on a gradient, so it doesn't show what Nordlicht is and feels like a slideshow of title cards.; The glow is a soft blurry blob and doesn't evoke an auro |
