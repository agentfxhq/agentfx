# Compare `baseline` -> `improved-2` (17 common cases)

| Metric | baseline | improved-2 | delta |
|---|---|---|---|
| cases | 17 | 17 | 0 |
| score | 79.04 | 81.24 | 2.2 |
| accept_rate | 0.353 | 0.471 | 0.118 |
| judge_accept | 0.353 | 0.529 | 0.176 |
| quality | 5.29 | 5.65 | 0.36 |
| checklist_pass | 0.882 | 0.882 | 0.0 |
| lint_clean | 1.0 | 0.941 | -0.059 |
| renders | 0.882 | 0.941 | 0.059 |
| duration_ok | 0.882 | 1.0 | 0.118 |
| text_ok | 0.941 | 0.941 | 0.0 |
| turns | 11.8 | 12.0 | 0.2 |
| lint_calls | 3.12 | 3.0 | -0.12 |
| first_lint_errors | 2.94 | 0.35 | -2.59 |
| lint_errors_seen | 3.47 | 1.35 | -2.12 |
| agent_cost_usd | 2.2 | 2.29 | 0.09 |
| judge_cost_usd | 0.49 | 0.48 | -0.01 |
| agent_wall_s | 255.86 | 98.28 | -157.58 |

| Case | score A | score B | delta | Q A | Q B | accept A | accept B |
|---|---|---|---|---|---|---|---|
| data-budget-donut | 64.5 | 83.3 | 18.8 | 4 | 5 | False | False |
| data-downloads-counter | 86.7 | 90.0 | 3.3 | 6 | 7 | False | True |
| data-q3-kpis | 76.7 | 80.0 | 3.3 | 5 | 5 | False | False |
| data-signups-bar | 83.3 | 86.7 | 3.4 | 5 | 6 | False | True |
| edit-add-vertical | 86.7 | 86.7 | 0.0 | 6 | 6 | True | True |
| edit-brand-colors-hold | 83.3 | 86.7 | 3.4 | 5 | 6 | True | True |
| edit-fix-broken | 83.3 | 83.3 | 0.0 | 5 | 5 | True | True |
| edit-title-bigger-slower | 86.7 | 86.7 | 0.0 | 6 | 6 | True | True |
| explainer-request-diagram | 71.3 | 77.3 | 6.0 | 5 | 5 | False | False |
| explainer-water-tiktok-captions | 61.8 | 63.8 | 2.0 | 5 | 5 | False | False |
| fx-fluid-ink-dream | 86.7 | 90.0 | 3.3 | 6 | 7 | False | True |
| fx-glitch-system-failure | 73.3 | 80.7 | 7.4 | 5 | 6 | False | False |
| kinetic-stop-scrolling | 65.3 | 59.3 | -6.0 | 5 | 5 | False | False |
| logo-reveal-svg | 86.7 | 79.2 | -7.5 | 6 | 6 | True | False |
| music-visualizer-night-drive | 86.7 | 90.0 | 3.3 | 6 | 7 | True | True |
| short-pushups | 83.3 | 83.3 | 0.0 | 5 | 5 | False | False |
| teaser-nordlicht | 77.3 | 74.0 | -3.3 | 5 | 4 | False | False |

