# direction.json (schema reel-director/1)

(The schema names `reel-director/1` and `reel-project/1` are kept for compatibility.)

One file per film project, next to the film's `.reel`. The agent writes it; the desk only reads it
(exceptions: the desk writes the stage-0 skeleton when the human creates a project, and brief
fields the human edits in the desk). Decision cards live next to it in `cards.jsonl` (below).
Paths are relative to the project dir. Times are seconds. `project.json` (below) sits next to it.

```jsonc
{
  "schema": "reel-director/1",
  "project": { "id": "spring-launch", "title": "...", "source": "film.reel", "mode": "deliberate|quick",
               "created": "ISO time", "operator": "name", "notes": "..." },
  "stage": "mode|brief|directions|dramaturgy|storyboard|variants|render",   // where the gate is now
  "revision": 4,             // global, +1 on every change to something the human has seen
  "feedback_cursor": 0,      // feedback.jsonl lines already handled by the agent

  "brief": {
    "rev": 2,
    "audience":  { "value": "...", "source": "answered|default-accepted|open" },
    "outcome":   { "value": "...", "source": "..." },
    "content":   { "must": ["exact copy, claims, assets"], "proposed": ["agent copy, not yet supplied"], "source": "..." },
    "envelope":  { "length_s": 45.0, "aspect": ["16:9", "9:16"], "fps": 30, "sound": "...", "source": "..." },
    "taste":     { "value": "...", "references": [{ "src": "url or path", "borrow": "...", "ignore": "..." }], "source": "..." },
    "priority":  { "value": "the moment that must land", "beat": "b02", "source": "..." },
    "open":      ["decisions still provisional"],
    "dialogue":  [{ "turn": 1, "by": "agent|operator", "text": "..." }]
  },

  "locks": [
    { "id": "L1", "text": "AT Aero only", "kind": "must|prefer", "scope": "project|beat|variant", "beat": "b07" }
  ],

  "directions": [
    { "id": "A", "name": "...", "idea": "one line", "palette": ["#hex"], "type": "...", "texture": "...",
      "motion": "...", "music": "...", "frame": "stills/style-a.jpg", "source": "style-a.reel",
      "status": "explore|chosen|rejected", "conflicts": ["L6 prefer: ..."], "rejected_note": "why" }
  ],
  "chosen_direction": { "base": "A", "mix": [{ "attribute": "palette|type|texture|motion|music", "from": "C", "detail": "..." }],
                        "rev": 1, "note": "one line" },

  "dramaturgy": {
    "rev": 1, "shape": "...", "hero": "b02",
    "arc": [{ "part": "hook|tension|turn|payoff|cta|...", "beats": ["b01"] }],
    "music": { "src": "...", "bpm": 128, "bar_s": 1.875, "beat_s": 0.46875,
               "notation": "bar:beat, bars from 0, beats from 1",
               "hits": [{ "at": "4:1", "t": 7.5, "what": "..." }] }
  },

  "beats": [   // in film order; ids are stable across reorders
    { "id": "b02", "title": "Drop", "t0": 7.5, "dur": 3.75, "bars": "4-5", "energy": [1.0, 0.6], "hero": true,
      "intent": "what the viewer understands or feels", "still": "stills/board-b02.jpg", "still_at": 7.9,
      "motion_note": "one line",
      "kinds": [{ "id": "shatter", "label": "...", "syntax": "Reel Script syntax", "status": "implemented|partial|planned" }],
      "kind": "shatter or null", "status": "explore|chosen|approved" }
  ],
  "storyboard": { "rev": 2, "contact_sheet": "stills/board.jpg or null", "changes": [{ "rev": 2, "beat": "b07", "what": "..." }] },

  "variants": [
    { "id": "v-b", "beat": "b02", "parent": "v-a", "parents": ["v-a", "v-c"],   // parents: only for mixes
      "axis": "motion treatment", "value": "Ink dissolve", "diff": "one sentence: what actually changed",
      "source": "hero-b.reel", "clip": "out/variants/hero-b.mp4", "still": "stills/hero-b.jpg",
      "clip_t0": 5.9, "dur": 4.5, "cue": 1.6, "cue_label": "drop",
      "status": "explore|chosen|rejected", "rejected_note": "why" }
  ],

  "notes": [
    { "id": "n1", "rev": 1, "stage": "storyboard", "beat": "b07", "variant": null, "t": 43.3,
      "clock": "film|clip", "by": "operator", "text": "...", "status": "open|done", "resolved_rev": 2 }
  ],

  "approvals": [ { "target": "brief|direction|dramaturgy|storyboard|variant|render", "rev": 2, "by": "operator", "at": "ISO" } ],
  "checks":    [ { "target": "hero-a.reel", "tool": "agentfx lint", "result": "0 errors, 2 warnings" } ],

  "render": { "status": "not-started|rendering|done", "planned": "...",
              "outputs": [{ "label": "16:9 master", "path": "out/renders/film-16x9.mp4" }] },

  "history": [ { "rev": 3, "at": "ISO", "by": "agent|operator", "action": "...", "detail": "..." } ]
}
```

## Rules

- **Stale approvals.** An approval names a target and the rev the human saw. When the target's
  `rev` moves past it, the approval is stale (the desk shows a banner, `desk.py status` lists it).
  Ask again; do not carry approvals forward silently.
- **Approval is not lint.** `approvals` hold human decisions only; `checks` hold tool results.
- **Nothing is deleted.** Rejected directions and variants keep their files and reason. Notes are
  closed with `status: done` and `resolved_rev`.
- **Variant times.** `cue` is seconds into the clip; `clip_t0` is where the clip starts in the film,
  so `t_film = clip_t0 + t_clip`. Notes from the desk carry both.
- **Beat ids** never change on reorder; `t0` is recomputed, ids are not renumbered.

## A new project's direction.json (stage 0)

`desk.py new` and the desk's "New project" write the same skeleton: `stage: "mode"`, `revision: 0`,
`mode: "deliberate"`, `project.source: null`, every section present but empty, every `rev` 0.
`brief.outcome.value` holds the one-line goal (`source: open`), `brief.envelope` holds the chosen
`length_s` and `aspect` (`source: answered`). A section with `rev: 0` has nothing to approve; the
desk shows an empty state for it.

# cards.jsonl (v3: decision cards)

The agent asks the human through cards in `<project>/cards.jsonl`: one JSON object per line,
**append-only**. A card is updated by appending a newer line with the same `id`; the latest line
wins (`v` counts the versions). Post them with the MCP tool `post_card`, `desk.py post <slug>
card.json`, or `POST /api/p/<slug>/cards` from localhost; never edit the file by hand.

```jsonc
{
  "id": "kind-b04",          // optional on post (c1, c2, ... assigned); letters, digits, . _ -
  "v": 1,                    // set by the server: +1 on every new line of this id
  "type": "choice|compare|note|approve|question",
  "title": "b04 The dive: animation kind",
  "body": "one or two short paragraphs: what to decide and why it matters",
  "stage": "storyboard",     // optional: which stage it belongs to (the rail counts open cards per stage)
  "beat": "b04",             // optional: ties the card (and its notes) to a beat
  "media": { "still": "stills/board-b04.jpg", "clip": "out/previews/b04.mp4",
             "cue": 1.6, "dur": 4.5, "clip_t0": 15.0, "clock": "clip|film", "label": "..." },
  "options": [               // choice and compare: 2 to 4
    { "id": "dive", "label": "Camera dive", "detail": "one line", "still": "...", "clip": "...",
      "audio": "out/cards/sketch-a.mp3",                 // choice: a player in the tile (music sketches)
      "cue": 1.6, "dur": 4.5, "clip_t0": 15.0,          // compare: clip required; cue aligns the clips
      "status": "implemented|partial|planned",           // planned: shown, never pickable
      "attrs": { "palette": "violet", "motion": "..." } } // optional, shown under the option
  ],
  "default": "dive",         // choice/compare: an option id; question: the proposed answer text (required)
  "attributes": ["palette", "type", "motion"],          // choice only: the human may mix per attribute
  "target": "storyboard", "rev": 2, "item": "L3",       // approve only: target is brief|direction|
                             // dramaturgy|storyboard|music|variant|render|lock|item
  "placeholder": "...",      // question only: hint in the text field
  "status": "open|answered|withdrawn",
  "answer": { ... },         // set by the desk when answered (below)
  "created": "ISO", "updated": "ISO", "by": "agent"
}
```

| type | the human sees | answer (in the card's `answer` and the feedback line) |
|---|---|---|
| `choice` | 2-4 tiles (still or hover clip, label, detail), keys 1-4, "accept proposed"; with `attributes` a per-attribute mixer | `pick: id` (+ `accepted_default`) or `mix: [{attribute, from}]`; optional `text` |
| `compare` | the clips in sync (shared scrub, align to cue, solo), a range lane, notes at a time or range | `pick` or `branch` (+ `text`) or `none` (+ `text`); `notes: [...]` |
| `note` | one clip (or a still) with the range lane | `notes: [...]` or `none: true` |
| `approve` | what and which rev, "Approve" or "Request changes" | `approve: true` / `approve: false` + `text`; `rev` |
| `question` | the proposed answer with one-click accept, or own text | `text` (+ `accepted_default`) |

A note is `{text, t | t0 + t1, t_film | t0_film + t1_film, clip, beat, clock}`: times are seconds in
the clip (`clip` is the compare option id); film times are added when the clip has `clip_t0`; `beat`
comes from the card, the human's selected beat, or the film time.

**Answers.** The desk appends one `feedback.jsonl` line and an answered version of the card:
```json
{"seq": 7, "action": "answer", "card": "cmp-hero", "v": 1, "card_type": "compare", "title": "...",
 "stage": "variants", "beat": "b02", "pick": "v-b",
 "notes": [{"text": "tighten by half a beat", "t0": 1.14, "t1": 2.6, "t0_film": 7.04, "t1_film": 8.5, "clip": "v-b", "beat": "b02"}],
 "context": {"range": {"t0": 1.14, "t1": 2.6, "clip": "v-b", "src": "out/variants/hero-b.mp4"}}}
```
`v` is the version the human saw; if the agent posted a newer version in between, the desk refuses
the answer and shows the new one. `context` is whatever the human had selected when answering: a
`beat`, a `point` in a still (`{src, x, y}`, 0..1 from the top left) and/or a `range` in a player.
Legacy actions (`pick`, `note`, ...) carry `context` the same way.

**Rules.** An answered card stays answered (re-posting the same id keeps the answer unless you set
`status: "open"` to ask again). Withdraw cards that no longer apply (`withdraw_card`) instead of
leaving them open. Cards never replace `direction.json`: you still record the decision there.

# project.json (schema reel-project/1)

The card the desk's home grid, switcher and palette show. The agent owns it (like
`direction.json`); the desk writes it only on create and on archive/unarchive.

```jsonc
{
  "schema": "reel-project/1",
  "slug": "spring-launch",           // = folder name, never changes
  "name": "Spring launch teaser",
  "client": "Acme Studio",
  "goal": "one line: what the viewer should do or feel",
  "created": "ISO", "updated": "ISO", // updated on every change (the desk sorts by it)
  "status": "active|paused|done|archived",
  "status_before_archive": "active",  // set by the desk while archived, restored on unarchive
  "stage": "variants",               // mirror of direction.json "stage" (direction.json wins)
  "poster": "stills/style-a.jpg",    // card image, 16:10 crop
  "preview": "out/preview.mp4",      // optional 3-6 s muted 640 px clip, plays on hover
  "tags": ["promo"],
  "delivery": "45 s, 16:9 and 9:16, 30 fps, music only",
  "agent": { "state": "working|waiting|idle", "note": "one line for the card", "at": "ISO" }
}
```

The desk computes the rest per card (`desk.py list`, `GET /api/projects`): `pipeline` (one state per
stage: `done`, `current`, `waiting` = the human's turn, `stale` = approval stale, `todo`), `pending`
(unread feedback lines), `open_cards` and `badge`: `cards` when the human has open cards (not
archived), else `pending` when the agent has unread lines, else `working` when `agent.state` is
`working` or `render.status` is `rendering`, else `waiting` for active projects.
