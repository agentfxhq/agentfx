# Eval run `full-31-fix`

Model sonnet, judge sonnet, mode both, max turns 40, commit 3e1b28f, wall 6.5 min.

| Metric | Value |
|---|---|
| cases | 6 |
| score | 80.87 |
| accept_rate | 0.667 |
| judge_accept | 0.667 |
| quality | 5.33 |
| checklist_pass | 0.917 |
| lint_clean | 1.0 |
| renders | 1.0 |
| duration_ok | 0.833 |
| text_ok | 1.0 |
| turns | 14.17 |
| lint_calls | 3.83 |
| first_lint_errors | 1.33 |
| lint_errors_seen | 2.33 |
| agent_cost_usd | 0.94 |
| judge_cost_usd | 0.17 |
| agent_wall_s | 50.78 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 5, `E_ID` 2, `E_CAPTION_OVERFLOW` 2, `E_TOPLEVEL` 2, `E_VERB` 1, `E_PRESET` 1, `E_EVENT_OUTSIDE` 1

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| data-downloads-counter | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 1/1 | 15 | 4 | 0 | 0.17 | The confetti is tiny and sparse. It reads as weak next to the huge number, and it's gone by about 5s.; The bottom third of the frame is empty and the content sits high, so the composition feels top-heavy.; The final 10,0 |
| edit-brand-colors-hold | 83.3 | 5 | yes | 3/3 | ok | ok | ok | 2/2 | 13 | 4 | 0 | 0.12 | The subtitle 'Q2 2027' is white instead of a brand colour, and it is small and weak next to the title.; The design is plain and generic: centred text on a flat colour, with no graphic elements.; The pulse added to the su |
| edit-fix-broken | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 9 | 4 | 3 | 0.10 | The bottom half of the frame is empty. All content sits in the upper-middle, so the composition feels top-heavy and sparse.; The tagline is small for a 9:16 video and has weaker contrast than the title.; The rise entranc |
| explainer-request-diagram | 77.3 | 5 | no | 4/5 | ok | ok | ok | 3/3 | 18 | 4 | 3 | 0.23 | The diagram is small. It takes up about the middle third of the frame, with a lot of empty space above and below, and the node labels and captions are small for 1080p.; The response path is not distinct. The return arrow |
| explainer-water-tiktok-captions | 64.5 | 4 | no | 3/4 | ok | ok | {'9:16': 10.7} | 1/1 | 19 | 4 | 0 | 0.20 | Captions are too small for the 'big bold' TikTok style. They fill only a small part of the frame, and the 110px size is visibly undersized.; The caption position is inconsistent. It jumps between left, centre and right a |
| fx-halftone-pop-art | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 1/1 | 11 | 3 | 2 | 0.13 | ART is dark charcoal on dark blue with a dotted texture. Contrast is low and it is much weaker than POP.; The first frame is a messy, blobby halftone and the text is hard to read. The resolve to clean dots takes about 1  |
