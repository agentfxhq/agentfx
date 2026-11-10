# AgentFX agent evals

How good is the video an AI agent makes with AgentFX on the **first try**, without a human
asking for changes? This harness measures it on realistic requests, so changes to the spec, lint,
skill and MCP tools can be judged by whether they raise that number.

## What a run does

For each case in `cases/*.yaml`:

1. A throwaway directory gets the AgentFX skill (`.claude/skills/agentfx`), the case's files
   (a logo, a track, a screenshot or an existing `film.reel` for edit cases), and a headless
   Claude Code session (`claude -p`, model `sonnet` by default) with the AgentFX MCP server and
   the `agentfx` CLI. The prompt is the user's request, phrased the way a non-expert writes it;
   a short system note says the session is non-interactive and that the result goes to
   `./film.reel` and `./out/*.mp4`.
2. The transcript (stream-json) gives turns, tool calls, lint calls, the lint errors the agent
   ran into, renders, frames and sheets looked at, cost and tokens.
3. The scorer then checks the result on its own:
   - **lint**: `agentfx lint --measure --format F` for every requested format, no errors;
   - **render**: the agent delivered an MP4 of the final script in every requested format
     (complete, right aspect). A case without a delivered video is still judged from a native
     contact sheet (`agentfx sheet`), but loses most render points and cannot be accepted;
   - **duration**: within +-15 % of a stated length, else 3 to 25 s;
   - **copy**: the must-have text is in the source (word by word, kinetic type splits lines);
   - **judge**: a vision model (`claude -p`, sonnet) reads a 12-frame contact sheet per format
     (from the delivered MP4), the source and, for edits, the script before the edit. It checks
     the case's must-have list, gives a quality score 1 to 10 and says whether a non-expert
     would accept the video as the first draft.

Score per case, 0 to 100: lint clean 12, delivered render 10 (4 if it only compiles), duration 8,
copy 10, checklist 30 x pass rate, quality 30 x (q - 1) / 9. **Accepted** = the judge would accept
it AND the user got a lint-clean rendered video: that is the first-try rate this harness exists for.

## Running it

```bash
cargo build --release                      # the harness uses target/release/agentfx (or $AGENTFX_BIN)
python3 evals/run.py list                  # the cases
python3 evals/run.py run --run-id mytest   # all cases; --cases 'fx-*,edit-*' or --tags data,edit for a subset
python3 evals/run.py score mytest          # re-score kept workdirs (new scorer or judge), no agent cost
python3 evals/run.py summary mytest        # rewrite results/mytest/summary.md
python3 evals/run.py compare baseline mytest
python3 evals/trace.py <work>/<run>/<case>/_transcript.jsonl   # what the agent did, step by step
```

Options: `--model` (agent, default sonnet), `--judge-model` (default sonnet), `--max-turns 40`,
`--timeout 900` (s per agent session), `--mode both|cli|mcp` (which interface the agent gets),
`--parallel 1`, `--max-load 10` (wait while the 1-min load is above it), `--skip-done` (resume a run),
`--work DIR` (workdirs with scripts, renders and transcripts; default `$AGENTFX_EVAL_WORK` or
`<tmp>/agentfx-evals`). Agent and judge use the local `claude` login; no API key is needed.

Shared machines: the harness puts its own `agentfx` first on the agent's PATH. That wrapper runs
`render`, `sheet` and `storyboard` under `flock $AGENTFX_HEAVY_LOCK` (default
`/tmp/claude-1000/agentfx-heavy.lock`), so only one heavy job runs at a time, and it kills what an
agent left running in its workdir when the session ends. Wall times therefore include waiting for
the lock and for the load to drop; compare costs, turns and scores, not seconds, across days.

Results: `results/<run-id>/cases/<case>.json` (all metrics, lint, judge checklist and problems),
`summary.md`, `summary.json`, `meta.json`. Media stay in the workdir and are not committed.

## Cases

31 requests in 9 groups: social clips and kinetic type (4), product teasers with brand colours and
a screenshot (4), explainers with captions and a diagram (3), data and KPI animation (4), logo
reveals (one from a provided SVG) (2), music-reactive clips with a provided track (2), effects
(fluid ink, glitch, shatter, halftone, infinite zoom, VHS) (6), vertical shorts (2) and edits of
an existing script (bigger and slower title, add a vertical version, brand colours and a longer
ending, fix a broken script) (4). Each case: `prompt`, `formats`, `duration_s` (null when the
user did not say), `files`, `must_text`, `must_haves`, `tags`.

## Results

First round (2026-11-10, agent and judge `sonnet`, 17 of the 31 cases: every group is covered;
the box was shared and overloaded, so the coordinator capped evals at one case at a time and the
full set did not fit). `baseline` = the product at `1dcf82d` (skill, spec, MCP, binary before this
work); `improved-1` = lint + spec + skill + MCP fixes, 10 cases; `improved-2` = plus the second
skill pass, all 17. Full tables: `results/<run>/summary.md`, `results/compare-*.md`.

| Metric (17 cases) | baseline | improved-2 |
|---|---|---|
| score (0-100) | 79.0 | 81.2 |
| accepted on the first try | 6 / 17 (35 %) | 8 / 17 (47 %) |
| judge would accept | 35 % | 53 % |
| quality (1-10) | 5.29 | 5.65 |
| delivered a rendered video | 88 % | 94 % |
| length right | 88 % | 100 % |
| lint errors at the first lint call (mean) | 2.94 | 0.35 |
| lint errors seen per case (mean) | 3.5 | 1.4 |
| turns (mean) | 11.8 | 12.0 |
| agent cost, 17 cases | $2.20 | $2.29 |

On the 10 cases of the first round alone: score 78.4 -> 83.0 (`improved-1`) -> 82.5 (`improved-2`),
accepted 4 -> 5 -> 6 of 10, delivered video 8 -> 9 -> 10 of 10.

A third skill pass (`improved-3`: "motion then a short hold", one look per brief) re-ran the three
cases that dropped in `improved-2`: kinetic-stop-scrolling 59.3 -> 74.7 (baseline 65.3),
teaser-nordlicht 74.0 -> 77.3 (baseline 77.3), logo-reveal-svg 79.2 -> 79.2 (baseline 86.7). That
is diminishing returns for skill wording; the remaining gaps are design quality and the renderer
issues below, so the iteration stopped there.

Read it with care: one run per case, and single cases move by +-10 between runs of the same
product (judge and agent variance). The robust signals are the hard ones (delivered video, length,
lint errors) and the accept rate over all cases, not any single case's delta.

### What the failures were and what changed

| Cluster | Evidence (baseline) | Fix |
|---|---|---|
| MCP/CLI friction: no video delivered | agents backgrounded `agentfx render` (Claude Code moves long commands to the background), then waited with `pgrep -f "agentfx render"`, which matches itself and other users' renders; the session ended and the render was killed (diagram, water captions, glitch: 3 cases, 15 min each) | SKILL: render once, foreground, long timeout or `agentfx_render`; never background or poll; MCP `agentfx_render` says it blocks |
| Lint missing a check | a chart scene without `dur=` is 0.5 s long; lint was clean (donut case shipped a 0.5 s video) | `W_SCENE_SHORT` + SPEC timing note + skill checklist |
| Spec / defaults unclear | counters print `10.000`, charts `2,1` (German defaults) in English videos; "1.2M" became "1200K" | `W_NUMBER_FORMAT` (counter/stat >= 10 000 without `sep=`, decimal chart data without `locale=`), SPEC counter note, skill |
| Bad fix hint / lint noise | `image src=logo.svg` said "use a PNG"; particles emitted below the frame gave 140 `E_OUT_OF_FRAME` errors with internal ids in one call | SVG hint points to `svg src=`; particle dots skipped by the static frame check |
| Skill guidance gap | small heroes in a big frame, static last seconds, edits handed over without a look, unrequested formats rendered, render cost unknown (an animated noise background made an 8 s counter render at 0.3 fps; the agent then ran `pkill -f agentfx-render`) | first-draft checklist (length, format, fill, type sizes, motion then a short hold, numbers, assets, component scale, social captions), render-cost estimate, no killing renders by name |
| Tool bug (scorer and `tools/critique.py`) | AgentFX writes variable frame rate MP4s; `ffmpeg -ss t -i` returns the next change, so a held title showed as an empty frame and the judge failed a correct video | `critique.grab` fills holds with `fps=` and trims; skill tells agents the same |

### Top remaining issues (not fixed here)

- **Renderer speed of per-frame looks.** `temporalglitch`, `vhs`, live grain and full-frame `gen`
  run at 0.3 to 3 fps at 1080p on the CPU; a 6 s glitch title took over 10 minutes. Agents trim
  the effect to deliver in time. A cost estimate in lint (frames x per-frame cost) would help.
- **Return links in a row diagram.** `link #b -> #a curve dashed` between nodes on one line is
  drawn over the forward link (no visible curve or dash); the "way back" is invisible.
- **Decorative bleed is an error.** Big translucent circles placed partly off frame (a common
  background device) are `E_OUT_OF_FRAME` errors; agents then ship with errors or drop the device.
  A `bleed`/decoration flag or a warning for shapes without text would fit better.
- **Components are slide-sized.** `stat`, `card`, `node` and chart legends are small in a 1080p
  frame by default; judges keep flagging "too small for a phone" even after the skill note.
- **Captions** ("big bold word by word") are often 2 to 4 words at a moderate size.
- **VFR MP4.** Correct in players, but tools that seek with `-ss` (and some editors) show the
  wrong frame for long holds; a `--mode cfr` default for delivery could be safer.

Cost and time: about $0.13 agent + $0.03 judge per case with sonnet (a full 31-case run is about
$5). Wall time is 1 to 3 minutes per case on an idle box and 10 to 30 minutes when the heavy-job
lock is contended; the first round took about 5 hours of wall time on the overloaded machine.
