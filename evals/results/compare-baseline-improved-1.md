# Compare `baseline` -> `improved-1` (10 common cases)

| Metric | baseline | improved-1 | delta |
|---|---|---|---|
| cases | 10 | 10 | 0 |
| score | 78.43 | 82.99 | 4.56 |
| accept_rate | 0.4 | 0.5 | 0.1 |
| judge_accept | 0.4 | 0.6 | 0.2 |
| quality | 5.2 | 5.5 | 0.3 |
| checklist_pass | 0.925 | 0.975 | 0.05 |
| lint_clean | 1.0 | 1.0 | 0.0 |
| renders | 0.8 | 0.9 | 0.1 |
| duration_ok | 0.8 | 1.0 | 0.2 |
| text_ok | 0.9 | 0.9 | 0.0 |
| turns | 11.56 | 12.44 | 0.88 |
| lint_calls | 2.8 | 4.2 | 1.4 |
| first_lint_errors | 1.4 | 0.3 | -1.1 |
| lint_errors_seen | 1.4 | 15.3 | 13.9 |
| agent_cost_usd | 1.17 | 1.31 | 0.14 |
| judge_cost_usd | 0.28 | 0.28 | 0.0 |
| agent_wall_s | 323.01 | 373.62 | 50.61 |

| Case | score A | score B | delta | Q A | Q B | accept A | accept B |
|---|---|---|---|---|---|---|---|
| data-budget-donut | 64.5 | 83.3 | 18.8 | 4 | 5 | False | False |
| data-downloads-counter | 86.7 | 80.7 | -6.0 | 6 | 6 | False | False |
| data-q3-kpis | 76.7 | 76.7 | 0.0 | 5 | 5 | False | False |
| data-signups-bar | 83.3 | 86.7 | 3.4 | 5 | 6 | False | True |
| edit-add-vertical | 86.7 | 83.3 | -3.4 | 6 | 5 | True | True |
| edit-brand-colors-hold | 83.3 | 86.7 | 3.4 | 5 | 6 | True | True |
| edit-fix-broken | 83.3 | 83.3 | 0.0 | 5 | 5 | True | True |
| edit-title-bigger-slower | 86.7 | 86.7 | 0.0 | 6 | 6 | True | True |
| explainer-request-diagram | 71.3 | 86.7 | 15.4 | 5 | 6 | False | False |
| explainer-water-tiktok-captions | 61.8 | 75.8 | 14.0 | 5 | 5 | False | False |

