# Eval run `improved-2`

Model sonnet, judge sonnet, mode both, max turns 40, commit ee18d22, wall 46.6 min.

| Metric | Value |
|---|---|
| cases | 17 |
| score | 81.24 |
| accept_rate | 0.471 |
| judge_accept | 0.529 |
| quality | 5.65 |
| checklist_pass | 0.882 |
| lint_clean | 0.941 |
| renders | 0.941 |
| duration_ok | 1.0 |
| text_ok | 0.941 |
| turns | 12.0 |
| lint_calls | 3.0 |
| first_lint_errors | 0.35 |
| lint_errors_seen | 1.35 |
| agent_cost_usd | 2.29 |
| judge_cost_usd | 0.48 |
| agent_wall_s | 98.28 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 12, `E_VERB` 4, `E_PRESET` 2, `E_OVERFLOW` 1, `E_ID` 1, `E_EVENT_OUTSIDE` 1, `E_PAINT` 1, `E_KIND` 1

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| data-budget-donut | 83.3 | 5 | no | 4/4 | ok | ok | ok | 4/4 | 10 | 2 | 0 | 0.10 | The legend text is tiny compared with the frame, so it is hard to read on social or mobile.; The donut sits left of center with the legend squeezed beside it. The right third of the frame is empty and the layout looks un |
| data-downloads-counter | 90.0 | 7 | yes | 4/4 | ok | ok | ok | 1/1 | 10 | 2 | 0 | 0.10 | Layout is top-heavy while counting. The number sits above centre with the bottom half empty, then it shifts down at the end.; At 4.38s the 'Thank you!' text is faintly visible behind the confetti and number, so it overla |
| data-q3-kpis | 80.0 | 5 | no | 4/4 | ok | ok | ok | 2/3 | 12 | 3 | 0 | 0.17 | The layout is small and sits in the middle of the frame, with a lot of empty space above and below. It is hard to read on a big all-hands screen.; The +18% pill is tiny and low-contrast, and it is the only growth callout |
| data-signups-bar | 86.7 | 6 | yes | 5/5 | ok | ok | ok | 2/2 | 9 | 2 | 0 | 0.10 | The axis ticks and month labels are very small and low contrast, so they would be hard to read on a phone.; The June highlight is only a lighter tint of the same green, so it does not stand out much. There is no accent c |
| edit-add-vertical | 86.7 | 6 | yes | 3/3 | ok | ok | ok | 2/2 | 16 | 5 | 0 | 0.19 | The vertical layout puts all content in the upper-middle and leaves the bottom ~45% of the frame empty. The bottom is also where story UI overlaps.; The subline in the vertical version is small relative to the headline a |
| edit-brand-colors-hold | 86.7 | 6 | yes | 3/3 | ok | ok | ok | 2/2 | 12 | 3 | 0 | 0.11 | The frame at 3.00s is empty blue for a moment at the scene change, which looks like a small dead gap before the outro text fades in.; The zoom the agent added was not requested, and it makes the title drift slowly. The u |
| edit-fix-broken | 83.3 | 5 | yes | 4/4 | ok | ok | ok | 2/2 | 9 | 2 | 3 | 0.10 | The bottom half of the frame is empty and the composition is generic, with text only in the middle.; The tagline is small for a 1080x1920 vertical video and looks weak next to the huge title.; BEAN BOX wraps onto two lin |
| edit-title-bigger-slower | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 16 | 3 | 0 | 0.14 | The date is small next to the large title, and there is a lot of empty space around the text.; The slow scale drift is barely visible, and the final 4 seconds look static.; The title rise is hard to see in the frames. It |
| explainer-request-diagram | 77.3 | 5 | no | 4/5 | ok | ok | ok | 3/3 | 14 | 5 | 0 | 0.20 | The diagram is small. The nodes and their subtitles take up a modest part of a 1920 frame, and there is a large empty gap between the diagram and the caption. The text is hard to read on a phone.; The response path is no |
| explainer-water-tiktok-captions | 63.8 | 5 | no | 3/4 | ERR | ok | ok | 1/1 | 14 | 4 | 0 | 0.13 | Captions are too small for 'big bold' TikTok style. They fill only a small part of the frame, and the 120px size looks timid on 1080 wide.; The background is generic: plain gradient with dull translucent circles. There a |
| fx-fluid-ink-dream | 90.0 | 7 | yes | 3/3 | ok | ok | ok | 1/1 | 7 | 1 | 0 | 0.09 | The word is gone by about 2.6s and never comes back. The dissolve is quick, and the second half is only ink swirling with no payoff, such as a title or reform.; The ink fills almost the whole frame by the end. It turns m |
| fx-glitch-system-failure | 80.7 | 6 | judge | 3/3 | ok | FAIL | ok | 1/1 | None | 2 | 0 | 0.00 | Between glitch bursts the text is clean, so the TV-breakdown feel is weak. There are no visible scanlines, tracking bars or screen distortion.; The late glitch on SYSTEM smears so heavily that the word is nearly illegibl |
| kinetic-stop-scrolling | 59.3 | 5 | no | 1/5 | ok | ok | ok | 2/2 | 12 | 4 | 0 | 0.16 | STOP and SCROLLING seem to appear together at the start rather than slamming in one after another, so the sequencing is weak in the first scene.; The second half is a flat full-green screen with black text. The brief ask |
| logo-reveal-svg | 79.2 | 6 | no | 3/4 | ok | ok | ok | 0/0 | 13 | 4 | 1 | 0.19 | The expanding ring is centred on the screen, not on the mark. At 0.2–1.0s it floats off to the right of the mark and then overlaps the blurred wordmark, so it looks like a mistake.; The logo does not hold still at the en |
| music-visualizer-night-drive | 90.0 | 7 | yes | 4/4 | ok | ok | ok | 1/1 | 12 | 3 | 2 | 0.14 | The '128 BPM' subtitle is small and sits tight under the title. The user didn't ask for it, and it reads as technical clutter.; Nothing shows driving, such as a road, car or moving grid. The scene is a static sun, so the |
| short-pushups | 83.3 | 5 | no | 4/4 | ok | ok | ok | 4/4 | 11 | 3 | 0 | 0.17 | Text only, with no push-up visuals, icons or diagrams. Nothing shows hand, glute or range-of-motion form, so it feels like a slide deck.; The background is a flat green gradient, and the same layout repeats in every scen |
| teaser-nordlicht | 74.0 | 4 | no | 4/5 | ok | ok | ok | 3/3 | 15 | 3 | 0 | 0.19 | It reads as a text slideshow. It has four centered text cards with no visuals, no app UI, no logo mark and no aurora or 'Nordlicht' imagery, so it doesn't feel like a product teaser.; The frame is mostly empty navy. The  |
