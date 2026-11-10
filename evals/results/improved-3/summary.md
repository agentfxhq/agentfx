# Eval run `improved-3`

Model sonnet, judge sonnet, mode both, max turns 40, commit ee18d22+dirty, wall 3.5 min.

| Metric | Value |
|---|---|
| cases | 3 |
| score | 77.07 |
| accept_rate | 0.0 |
| judge_accept | 0.0 |
| quality | 5.67 |
| checklist_pass | 0.714 |
| lint_clean | 1.0 |
| renders | 1.0 |
| duration_ok | 1.0 |
| text_ok | 1.0 |
| turns | 12.0 |
| lint_calls | 2.67 |
| first_lint_errors | 1.67 |
| lint_errors_seen | 2.0 |
| agent_cost_usd | 0.61 |
| judge_cost_usd | 0.09 |
| agent_wall_s | 52.13 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 4, `E_KIND` 2

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| kinetic-stop-scrolling | 74.7 | 6 | no | 3/5 | ok | ok | ok | 2/2 | 12 | 2 | 2 | 0.18 | Pacing is slow for a punchy reel: the second word waits over 1s, and the last ~2s hold static with little new action.; SCROLLING and BUILDING are much smaller than STOP and START, so they don't fill the frame width and f |
| logo-reveal-svg | 79.2 | 6 | no | 3/4 | ok | ok | ok | 0/0 | 13 | 4 | 2 | 0.26 | The ring pulse is off-center and clipped by the frame edge (0.62s). It is centered on the left side, not on the mark, and looks like a bug.; The mark starts off-center at the left and the wordmark enters afterward, so th |
| teaser-nordlicht | 77.3 | 5 | no | 4/5 | ok | ok | ok | 3/3 | 11 | 2 | 1 | 0.17 | Text-only video with no visuals of the app or any imagery. It feels like a slideshow of title cards, not a product teaser.; The brand name doesn't appear until about 7.5s. The first 7s show only the tagline halves, so th |
