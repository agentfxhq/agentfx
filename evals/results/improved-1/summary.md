# Eval run `improved-1`

Model sonnet, judge sonnet, mode both, max turns 40, commit ddbdbbb+dirty, wall 82.2 min.

| Metric | Value |
|---|---|
| cases | 10 |
| score | 82.99 |
| accept_rate | 0.5 |
| judge_accept | 0.6 |
| quality | 5.5 |
| checklist_pass | 0.975 |
| lint_clean | 1.0 |
| renders | 0.9 |
| duration_ok | 1.0 |
| text_ok | 0.9 |
| turns | 12.44 |
| lint_calls | 4.2 |
| first_lint_errors | 0.3 |
| lint_errors_seen | 15.3 |
| agent_cost_usd | 1.31 |
| judge_cost_usd | 0.28 |
| agent_wall_s | 373.62 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 143, `E_OVERFLOW` 4, `E_CAMERA` 2, `E_PRESET` 1, `E_ID` 1, `E_EVENT_OUTSIDE` 1, `E_VERB` 1

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| data-budget-donut | 83.3 | 5 | no | 4/4 | ok | ok | ok | 4/4 | 5 | 5 | 0 | 0.17 | The legend text is tiny for a 1080p video and hard to read. The percentages are not on the slices.; The composition is unbalanced. The pie sits left of centre and the legend is small and crowded next to it. The right hal |
| data-downloads-counter | 80.7 | 6 | judge | 4/4 | ok | FAIL | ok | 1/1 | None | 6 | 0 | 0.00 | The confetti is thin, tiny and brief, so the celebration feels underwhelming for a 10k milestone. It fades out by about 6s.; 'Thank you, and counting!' wraps onto two lines. The 'and' is highlighted in mint, which looks  |
| data-q3-kpis | 76.7 | 5 | no | 4/4 | ok | ok | ok | 1/3 | 13 | 5 | 0 | 0.14 | The stat cards are small and sit in the middle of a 1080p frame. The numbers, labels and especially the +18% delta are too small for an all-hands screen.; The +18% growth is a tiny pill and is the key message, so it has  |
| data-signups-bar | 86.7 | 6 | yes | 5/5 | ok | ok | ok | 2/2 | 16 | 5 | 0 | 0.14 | The title is small and sits in the top-left corner. For a headline it has weak hierarchy and would be hard to read on a phone.; The axis labels, month labels and value labels are tiny, and the axis ticks are very faint.; |
| edit-add-vertical | 83.3 | 5 | yes | 3/3 | ok | ok | ok | 2/2 | 18 | 5 | 0 | 0.21 | Vertical layout puts all content in the upper-middle and leaves the bottom ~45% of the frame empty. Story UI overlays the top and bottom, so the layout feels unbalanced.; The headline wraps with 'THE' alone on the middle |
| edit-brand-colors-hold | 86.7 | 6 | yes | 3/3 | ok | ok | ok | 2/2 | 11 | 2 | 0 | 0.10 | The 3.00s frame is blank blue, so there is a visible empty beat between the scenes.; The text is the same size and weight throughout, and the subtitle 'Q2 2027' is small and the same yellow as the title, so the hierarchy |
| edit-fix-broken | 83.3 | 5 | yes | 4/4 | ok | ok | ok | 2/2 | 11 | 3 | 3 | 0.10 | The layout is generic: the text sits in the middle and the bottom 40% of the frame is empty, with no imagery or coffee cue.; The tagline is small for a vertical reel, at about 60px.; The title wraps to two lines, which i |
| edit-title-bigger-slower | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 9 | 2 | 0 | 0.10 | The date is small next to the large title, so the hierarchy gap is big and it is hard to read on mobile.; After about 2s the frame is static apart from a barely visible 4% scale drift, so the last 5s feel dead.; The layo |
| explainer-request-diagram | 86.7 | 6 | no | 5/5 | ok | ok | ok | 3/3 | 15 | 4 | 0 | 0.20 | The diagram is small. The nodes and their subtitles are tiny for a 1080p frame, and the middle of the frame is mostly empty, with a large gap between the diagram and the caption.; The response path is not a distinct retu |
| explainer-water-tiktok-captions | 75.8 | 5 | no | 3/4 | ok | ok | ok | 1/1 | 14 | 5 | 0 | 0.16 | Captions are small for a 1080-wide vertical frame and show 2–4 words at once, so they are not the big bold word-by-word style requested.; Frame is mostly empty dark blue gradient with no water, glass, or imagery. It look |
