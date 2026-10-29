---
name: agentfx
description: Make motion graphics and videos with AgentFX. Write a Reel Script (.reel), lint it, look at frames and contact sheets, then render MP4, GIF, WebP or alpha video natively (no browser). Quick mode renders straight away and is the default; director mode deliberates first (brief, three directions with style frames, dramaturgy on the music grid, storyboard, hero variants, then the full render), with an optional local browser desk. Use when the user asks for a video, animation, motion graphics, social clip, explainer, product teaser, kinetic typography, captions, an effect like fluid ink, physics, shatter, glitch, VHS, halftone, slit-scan or infinite zoom, or says "director mode", "storyboard first", "show me directions".
---

# AgentFX

AgentFX turns a short text script into video. You write **Reel Script** (`.reel`), the `agentfx`
CLI lints it, renders single frames and contact sheets so you can look, and renders the video
with its native renderer (`agentfx-render`, CPU or Vulkan GPU, no browser). Same script, same
frames: renders are deterministic.

Sources of truth, in this order:
1. `agentfx catalog --md`: every element kind, verb, effect and its status (implemented, partial,
   planned). Never offer or write something the catalog does not list as implemented.
2. The spec: `docs/SPEC.md` in the AgentFX repository, or `agentfx_spec` over MCP.
3. Examples: `examples/features/*.reel` (one effect each), `examples/showcase.reel`,
   `examples/hello.reel`.

If the AgentFX MCP server is registered, prefer its tools (`agentfx_spec`, `agentfx_catalog`,
`agentfx_lint`, `agentfx_frame`, `agentfx_sheet`, `agentfx_render`); otherwise use the CLI.

## Projects directory

Keep each film in its own folder under a projects directory: by default `./agentfx-projects/`
in the current working directory, or what the user names (`$AGENTFX_PROJECTS`, `--root` for the
desk). One folder per film, named by a slug (lowercase `a-z`, `0-9`, `-`, at most 48
characters, never renamed). Inside: the `.reel` sources, assets, `stills/` (small jpg stills worth
keeping) and `out/` (renders, previews; not worth committing).

## Quick mode (default)

Ask nothing more than you need. If the request is clear, go:

1. Read the catalog (and the spec section for what you use). Pick the closest example as a start.
2. Write the `.reel`. Copy, numbers, names and logos come from the user; never invent claims.
   Mark any copy you propose as proposed.
3. `agentfx lint film.reel --measure --all-formats` (add `--no-tts` when there is no voice-over).
   Fix every error and warning; lint hints say how.
4. Look before you render: `agentfx sheet film.reel --events -o out/sheet.png` and
   `agentfx frame film.reel --at <anchor|seconds> -o out/frame.png` for the moments that matter.
   Open the PNGs and check overflow, clipping, contrast, alignment and pacing. Fix and repeat.
5. `nice -n 19 agentfx render film.reel -o out/film.mp4` (`--format 9:16` for vertical; `.gif`,
   `.webp`, `.mov` or `.webm` outputs by extension; `--alpha` for a transparent background).
6. Check the result (a strip of frames: `ffmpeg -i out/film.mp4 -vf fps=2,scale=320:-1,tile=9x1
   out/strip.png`), then hand over the path with one line on what you made.

Offer director mode in one line only when the brief is open-ended or the user wants to steer:

> Render it straight away, or shall we decide look, arc and hero moment together first (about five short rounds)?

"Straight away", "quick", silence, or a complete brief: stay in quick mode.

## Director mode (optional)

You make the film together with a human director. The human chooses creative vision and a few
details; you do setup, writing, rendering and checking. Every decision is a **gate**: show
something concrete, propose a default, wait for the choice. State lives in `direction.json` in
the project folder (schema: `schema.md`).

The stages below are a default recipe. Skip, reorder or revisit when the film calls for it (a 6 s
loop needs no dramaturgy table) and say in one line what you skip and why. Ask at most 2 or 3
questions per turn, numbered, each with a proposed default the human can accept with one word.

**Stage 1, brief (five commitments).** Audience and outcome; required content (exact copy,
claims, logos, CTA); delivery envelope (length, aspects, sound: music, voice, none); taste anchor
(a reference and what to borrow, or "show me directions"); priority and locks (the moment that
must land, what is fixed). Turn fixed points into locks: `must` (hard) or `prefer` (soft, may be
broken with a stated reason). Gate: "Brief rev N ok?"

**Stage 2, directions.** Three genuinely different directions (concept and material, not three
tints): name, one-line idea, palette (hex), type, texture, motion character, music feel. Render
one style frame each with the same copy (`style-<id>.reel`, 2 s, `agentfx frame ... --at 1.6s`),
lint and look at each before showing. Gate: pick one, mix attributes ("palette of B, type of A"),
or ask for a new direction. Keep rejected ones with the reason.

**Stage 3, dramaturgy.** Shape (hook, tension, turn, payoff, CTA, or the film's own), a beat list
with stable ids (`b01`...), intent, start and duration (durations sum exactly to the length; show
the sum), energy per beat as start and end values 0..1 (one peak is the hero), and a music plan:
bpm, bar and beat length, hit points as `bar:beat` with seconds. Cut on the grid.

**Music: bring your own track.** AgentFX does not compose music. Ask the user for a track they
have the rights to (or agree on silence or voice-over only). Put it in the project's `assets/`,
take bpm and downbeat from the user or from `audio()` analysis (see the spec's audio section:
beat anchors, `audio()` drives, visualizers), and align the beat list to its bars.

**Stage 4, storyboard.** Per beat: one still (`agentfx frame <film>.reel --at <t>s`), one line of
motion note, and a menu of 2 to 4 animation kinds from `animation-kinds.md` (verified against the
catalog). Planned features may be listed only as `planned` and cannot be chosen.

**Stage 5, hero variants.** Three short clips (3 to 5 s) of the hero beat that differ on ONE named
axis (motion treatment, pacing, composition, material...). Everything else stays identical,
including the cue time; generate them from one source by swapping only the varied block. Record
`parent`, `axis`, `value`, `diff`, `clip`, `still`, `clip_t0`, `dur`, `cue`. Look at a strip of
frames before showing them. Gate: pick, combine (a child variant with both parents), note at a
timestamp, or "none of these".

**Stage 6, first full render.** Only now render the whole film in every format of the envelope,
make a contact sheet (`agentfx sheet <film>.reel --events`), and continue with the review loop:
notes carry revision, time and beat id; fix, re-render, show again.

**State rules.** `revision` goes up whenever you change something the human approved; each
section has its own `rev`, and an approval is for one rev (it goes stale when the section moves
on). Human approvals are separate from lint results: a clean lint never approves anything.
Rejected directions and variants are kept with the reason; notes are marked done, not deleted.
Append every step to `history`.

## The desk (optional, local)

The AgentFX repository ships a small local web page for director mode: `director/desk.py`
(Python stdlib, no build) and a director MCP server, `director/mcp.py`. The agent posts
**decision cards** (choice, compare, note, approve, question), the human answers them in the
browser, and each answer lands as a JSON line in the project's `feedback.jsonl`.

```bash
python3 <agentfx>/director/desk.py serve ./agentfx-projects --port 8771   # binds 127.0.0.1 only
python3 <agentfx>/director/desk.py new "Spring launch" --client "Acme" --goal "one line" --aspect 16:9 --length 30
```

Start `serve` in the background, note the PID, and `kill <pid>` when done. The desk has no
authentication: keep it on 127.0.0.1. Loop: `read_feedback(slug)`, act on each line in order and
update `direction.json`, `ack(slug)`, post the next card(s) with a proposed default, set the agent
line (`set_agent_state`). Details, card format, API and keys: `desk.md`.

Chat works just as well; the desk is a convenience, not a requirement.

## Rules

- One render at a time, with `nice -n 19`. Check free memory (`MemAvailable` in `/proc/meminfo`)
  before long or parallel renders; renders of big formats can need several GB.
- Look at frames before you claim something works. Numbers from lint are not a visual check.
- Never break a `must` lock silently. If a request conflicts with one, say so and offer a way out.
- Do not show more than three alternatives at once; name what differs.
- Never invent facts, numbers, client names or logos. Use only media the user has rights to.
- No em-dashes in copy, UI text or messages.
- GPU effects (`--gpu`) need Vulkan; without it everything renders on the CPU. 3D is stylised:
  `scene3d` with simple shapes, extruded text, OBJ meshes and screens (toon, outlines, glow,
  fog, ground shadows), not photoreal. A moving 3D camera re-renders every frame (about 0.2 s
  per 1080p frame); a still 3D scene costs nothing after its first frame.
