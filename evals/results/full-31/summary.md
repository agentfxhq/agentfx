# Eval run `full-31`

Model sonnet, judge sonnet, mode both, max turns 40, commit d8b3ac0, wall 160.3 min.

| Metric | Value |
|---|---|
| cases | 31 |
| score | 72.11 |
| accept_rate | 0.452 |
| judge_accept | 0.484 |
| quality | 5.88 |
| checklist_pass | 0.897 |
| lint_clean | 0.935 |
| renders | 0.806 |
| duration_ok | 1.0 |
| text_ok | 0.968 |
| turns | 13.0 |
| lint_calls | 2.74 |
| first_lint_errors | 0.52 |
| lint_errors_seen | 1.48 |
| agent_cost_usd | 6.99 |
| judge_cost_usd | 0.73 |
| agent_wall_s | 290.23 |

Lint errors the agents ran into (code: count over all lint calls): `E_OUT_OF_FRAME` 28, `E_EVENT_OUTSIDE` 3, `E_PHYSICS` 3, `E_CAMERA` 2, `E_ANCHOR` 2, `E_ICON` 2, `E_OVERFLOW` 1, `E_PRESET` 1, `E_ID` 1, `E_TOPLEVEL` 1, `E_CAPTION_OVERFLOW` 1, `E_VERB` 1

| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| data-budget-donut | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 4/4 | 21 | 4 | 0 | 0.21 | The chart and legend are small for the 1080p frame, with empty space at the sides and bottom.; The two greens (Rent and Salaries) are similar, and the legend dots are tiny, so matching legend to slice is hard.; The centr |
| data-downloads-counter | 30.0 | None | no | 0/0 | ok | FAIL | ok | 1/1 | 1 | 2 | 0 | 0.18 |  |
| data-q3-kpis | 83.3 | 6 | yes | 4/4 | ok | ok | ok | 2/3 | 25 | 3 | 0 | 0.24 | Revenue 1.2M and churn 2.1% appear statically instead of counting up, so the 'numbers should count up' request is only partly met.; Cards are crowded in the top ~55% of the frame, with large empty space below and tight g |
| data-signups-bar | 77.3 | 5 | no | 4/5 | ok | ok | ok | 2/2 | 1 | 2 | 0 | 0.18 | June highlight is weak. It is a pale, desaturated green, with no contrasting accent colour. It looks washed out rather than emphasised.; The bar-grow animation is glitchy. At 0.25s Jan shows 87 and Feb shows 52, so the b |
| edit-add-vertical | 86.7 | 6 | yes | 3/3 | ok | ok | ok | 2/2 | 25 | 7 | 0 | 0.30 | In 9:16 the text block sits in the upper-middle and the bottom 40% of the frame is empty. The composition feels top-heavy and unused.; In 9:16 the subline runs almost edge to edge. Its side margins are tight and fall ins |
| edit-brand-colors-hold | 30.0 | None | no | 0/0 | ok | FAIL | ok | 2/2 | 10 | 1 | 0 | 0.10 |  |
| edit-fix-broken | 30.0 | None | no | 0/0 | ok | FAIL | ok | 2/2 | 12 | 3 | 3 | 0.12 |  |
| edit-title-bigger-slower | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 16 | 2 | 0 | 0.16 | Title side margins are tight (~10% each side), so it feels slightly crowded.; The title entrance looks only slightly slower; it's mostly settled by ~0.9s, and the rise motion is barely visible.; The date is small next to |
| explainer-request-diagram | 30.0 | None | no | 0/0 | ok | FAIL | ok | 3/3 | 13 | 4 | 0 | 0.17 |  |
| explainer-solar-captions | 83.3 | 5 | no | 4/4 | ok | ok | ok | 2/2 | 22 | 2 | 6 | 0.34 | Step 1 layout is weak: the sun and panel are unconnected, with no visible rays, and the panel is a blank blue rectangle with no cell grid. The sun looks dropped in next to it rather than shining on it.; Step 2 is only lo |
| explainer-water-tiktok-captions | 30.0 | None | no | 0/0 | ok | FAIL | ok | 1/1 | 23 | 6 | 0 | 0.24 |  |
| fx-fluid-ink-dream | 90.0 | 7 | yes | 3/3 | ok | ok | ok | 1/1 | 15 | 1 | 0 | 0.15 | The ink floods nearly the whole frame by the end. It reads as a muddy blue mass and never settles on a clear final image or resolve.; The dissolve is quick, so the word is gone by about 3s. The ink spends the remaining ~ |
| fx-glitch-system-failure | 64.7 | 6 | no | 2/3 | ERR | ok | ok | 1/1 | 1 | 2 | 0 | 0.23 | There is no broken-TV environment. The black background has no static, scanlines, roll bars or screen flicker, and the glitch is confined to the text.; The red ghost copy is offset to the right and shows as a dark red sl |
| fx-halftone-pop-art | 30.0 | None | no | 0/0 | ok | FAIL | ok | 1/1 | 1 | 3 | 0 | 0.19 |  |
| fx-infinite-zoom | 90.0 | 7 | yes | 3/3 | ok | ok | ok | 1/1 | 2 | 1 | 0 | 0.24 | Text contrast drops over bright yellow/pink/cyan areas; the shadow helps but the white text is less crisp there.; The start has large black empty areas, so the fractal looks sparse and less striking.; Large smooth gradie |
| fx-shatter-breaking-news | 74.7 | 6 | judge | 3/3 | ERR | ok | ok | 1/1 | 19 | 3 | 3 | 0.19 | The last ~1.5s is a static pile of shards. It reads as dead time with no resolution or ending beat.; Shards are pushed to the frame edges and clipped at the left and right. At 2.71s some also overlap the LIVE tag.; Shard |
| fx-vhs-retro-rewind | 73.3 | 5 | no | 2/3 | ok | ok | ok | 1/1 | 19 | 2 | 0 | 0.17 | The title scene is clean and digital, with little VHS texture. The user asked for a VHS look, and the main branding frames lose it.; The white RETRO sits on the bright sun, so its contrast is weak. The sun also crowds RE |
| kinetic-countdown | 86.7 | 6 | no | 4/4 | ok | ok | ok | 4/4 | 14 | 4 | 2 | 0.21 | The LAUNCH finale is weak. The text is small and sits in a thin band with a lot of empty purple above and below. No confetti or flash shows in the sampled frames, and the frame looks static for about 2s.; The yellow ring |
| kinetic-quote | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 2/2 | 24 | 1 | 0 | 0.27 | Layout shifts as words are added: the line re-centers and jumps, and the ultimate row looks left-aligned at 4.6–5.4s, which breaks the calm feel.; The author credit is tiny and sits very close under the quote. The hierar |
| kinetic-stop-scrolling | 71.3 | 5 | no | 3/5 | ok | ok | ok | 2/2 | 24 | 6 | 0 | 0.28 | Text is smaller than 'big and bold' implies. SCROLLING and BUILDING are small, and the text block takes up only about a third of the tall frame, leaving large empty black space above and below.; Low energy. The words sit |
| logo-reveal-bakery | 90.0 | 7 | yes | 4/4 | ok | ok | ok | 1/1 | 1 | 1 | 0 | 0.30 | The glow circle is cropped at the top and bottom of the frame and reads as a large flat disc. Its hard edge looks a bit clumsy.; The loaf mark is fairly small and generic next to the very large wordmark. The lockup is sl |
| logo-reveal-svg | 79.2 | 6 | yes | 3/4 | ok | ok | ok | 0/0 | 1 | 1 | 0 | 0.19 | Reveal is a plain stroke-draw then fill. It's clean but generic, with no glow, light sweep or accent to make it 'cool'.; The first ~0.8s is nearly empty, with just a faint thin outline. It looks slow and weak.; The wordm |
| music-beat-words | 83.3 | 5 | yes | 4/4 | ok | ok | ok | 4/4 | 21 | 2 | 0 | 0.22 | The design is plain: a flat dark background with one word, and nothing else moves except a small pulse.; The words fill only about 60–70% of the frame width and sit in the middle of a mostly empty tall frame. The type co |
| music-visualizer-night-drive | 90.0 | 7 | yes | 4/4 | ok | ok | ok | 1/1 | 20 | 1 | 0 | 0.21 | Bars are thin and low-profile, and they sit on the horizon line. The bottom half of the frame is a large empty black block.; The '128 BPM' subtitle is tiny, floats in the empty ground area, and adds little.; There is no  |
| promo-summer-sale-multi | 68.3 | 5 | no | 2/4 | ok | ok | ok | 3/3 | 1 | 4 | 0 | 0.25 | Layout is top-heavy in both formats. In 9:16 the text block sits in the upper half with roughly 40% of the frame empty at the bottom. In 1:1 the bottom third is empty.; '-30%' is buried inside a sentence in a mid-sized p |
| short-pushups | 83.3 | 5 | no | 4/4 | ok | ok | ok | 4/4 | 1 | 6 | 0 | 0.33 | There are no visuals at all. It is plain text on black, with no push-up demo, icons or illustration, so it looks generic and unengaging.; Frames are mostly static, with the same layout repeated for each fix, so motion fe |
| short-quote-story | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 1/1 | 17 | 3 | 0 | 0.22 | Text sits in the upper third while the lower half is filled with circles and squares; the layout feels unbalanced and the middle is empty.; The pink circle overlaps the right end of 'Small steps' and slightly muddies it  |
| social-sleep-tips | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 3/3 | 16 | 2 | 0 | 0.21 | The bottom 40% of every frame is empty, so the layout is top-heavy and the frame is poorly used.; There is a big gap between the huge number and the tip text, so they don't read as one unit.; The scenes are identical tex |
| teaser-beanbox-cards | 86.7 | 6 | yes | 4/4 | ok | ok | ok | 4/4 | 15 | 3 | 2 | 0.19 | The layout is top-heavy. The bottom ~15% of the frame is empty, and the empty space is uneven.; The three pills are identical, full-width orange blocks. There are no icons or imagery, and no coffee or bean visual, so the |
| teaser-dashboard-shot | 79.2 | 6 | no | 3/4 | ok | ok | ok | 2/2 | 1 | 1 | 0 | 0.40 | Dashboard is too small in the frame at rest (about 1300px on a 1920 canvas, small top-heavy placement); UI details are illegible until the zoom.; The headline disappears before the zoom begins, so the zoom has no copy an |
| teaser-nordlicht | 80.7 | 6 | no | 4/5 | ok | ok | ok | 3/3 | 21 | 2 | 0 | 0.27 | Purely typographic with no app UI, imagery or logo, so it feels like a generic slideshow rather than a product teaser; The tagline is split across two separate scenes and never appears together; 'Plan less.' fades to an  |
