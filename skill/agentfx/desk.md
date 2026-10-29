# The director's desk (v3: card stream + director MCP)

A steering surface for all AgentFX film projects, served by `director/desk.py` in the AgentFX
repository (Python 3 stdlib, no build; files `index.html`, `desk.css`, `desk.js`). The
agent authors the interaction: it posts **decision cards** into a stream, the human answers them.
The six stages are a recipe in `SKILL.md`, not the navigation; their v2 pages stay reachable as
panels from the state rail.

```bash
python3 <agentfx>/director/desk.py serve [ROOT] [--port 8771]   # ROOT default ./agentfx-projects
# binds 127.0.0.1 only; --host adds another address (never a public one: the desk has no auth)
desk.py list                    # projects: slug, status, stage, pending actions
desk.py inbox [--json]          # unread actions across ALL projects, each tagged "project": <slug>
desk.py new "Name" --client C --goal G [--aspect 9:16 ...] [--length 30]
desk.py status   <slug|dir>     # stage, stale approvals, unread feedback
desk.py feedback <slug|dir>     # unread lines of one project; --all for every line
desk.py ack      <slug|dir>     # mark them read (moves direction.json feedback_cursor)
desk.py cards    <slug|dir>     # open cards (--all: every card, latest version)
desk.py post     <slug|dir> card.json   # post a card ("-" reads stdin)
```
All commands take `--root DIR` (or `$AGENTFX_PROJECTS`). Start `serve` in the background and remember
the PID; stop it with `kill <pid>`. Open `http://127.0.0.1:8771/`.

## The director MCP server (preferred for agents)

`director/mcp.py` is a stdio MCP server (stdlib only; the protocol is implemented by hand, so no
package is needed). It shares its state code with `desk.py`, so the desk
and the agent write the same files the same way (with file locks). Register it once; this is a
snippet to paste, nothing changes your Claude Code config by itself:

```bash
claude mcp add agentfx-director -- python3 /path/to/agentfx/director/mcp.py --root /path/to/agentfx-projects
# or with the environment variable:
claude mcp add agentfx-director -e AGENTFX_PROJECTS=/path/to/agentfx-projects -- python3 /path/to/agentfx/director/mcp.py
```

| tool | what |
|---|---|
| `list_projects()` | slug, name, status, stage, badge, unread feedback, open cards, agent line |
| `get_state(slug)` | direction.json, project.json, stale approvals, open cards, unread feedback |
| `post_card(slug, card)` | post a card or a new version (same `id`); returns the stored card |
| `withdraw_card(slug, id, note?)` | the card leaves the stream (history: withdrawn) |
| `read_feedback(slug, all?)` | unread lines past the cursor (card answers, picks, notes, brief edits) |
| `ack(slug, upto?)` | move `feedback_cursor` (all, or up to seq `upto`) |
| `render_preview(slug, source, at \| t0+t1, out?, format?)` | a still or a short clip of a `.reel`, returns its project path |
| `import_media(slug, src, name?)` | copy an audio/video/image file (under your home or temp directory) into `out/cards/` so a card can show it, e.g. a music track |
| `set_agent_state(slug, state, note)` | `working` / `waiting` / `idle` and one line for the card |

**render_preview.** A still is `agentfx frame <source> --at <at>s` (jpg out is converted to 1280 px).
A clip renders the whole source with `agentfx render` (there is no range render yet) and cuts
`t0..t1` with ffmpeg at 1280 px, at most 20 s: keep clip sources short (one beat, a hero `.reel`).
Default output `out/previews/<stem>-<t>.jpg|mp4`. It runs locally with `nice -n 19`; the answer
includes the machine's available memory, so check it before starting several renders.

## Pages and URLs

| URL | What |
|---|---|
| `/` | Projects home: cards with poster, hover preview, stage pipeline, badge (**N open cards** first), filter, search, sort, new project |
| `/p/<slug>/` | the project: the **stream** of open cards (newest first), answered cards in a collapsed history, an aside (agent line, brief at a glance, locks) |
| `/p/<slug>/<stage>` | the stream with that stage open as a **panel**: `mode`, `brief`, `directions`, `dramaturgy`, `storyboard`, `variants`, `render` |

Header: breadcrumb, project switcher, command palette (Ctrl/Cmd+K), Outbox (everything sent, read or
not). Under the title the **state rail**: Stream, then Brief, Directions, Dramaturgy, Storyboard,
Hero variants, Render, each with its state (done, in progress, your turn, approval stale, to do) and
the number of open cards tagged with that stage. A rail item opens the v2 page in a panel (Esc or
"Stream" closes it). The dramaturgy panel shows a **beat board on the bar grid** (bar ruler, beats
sized by duration with their stills, energy curve, hits). The brief stays click-to-edit.

**Selection as context.** Clicking a beat (dramaturgy board, beat chips, storyboard), a point in a
still (storyboard stills, style frames, card stills; a marker shows it) or marking a range in any
player sets a floating "Context" chip. It goes with the next card answer or desk action as
`context: {beat?, point?: {src, x, y}, range?: {t0, t1, clip, src}}`, then clears. `X` clears it.

**Keys** (`?` shows them). Stream: `J`/`K` next or previous open card, `1`-`4` select option or clip,
`Enter` send the selection or accept the proposed default, `N` write a note (approve: request
changes), `Space` play, `I`/`O` range in/out, `Shift+1`..`6` open a stage panel, `X` clear context.
Panel: `[` `]` stage, `J`/`K` variants or directions, `1`-`3` pick, `Space`, `N`, `Esc`. `Shift+O`
outbox, `G H` home, `/` search, `C` new project. Dark and light, phone width, reduced motion.

## API

| Method, path | What |
|---|---|
| `GET /api/projects` | `{projects: [card + open_cards], version, single, root, stages}` |
| `GET /api/version?p=<slug>` | `{home, project}` change signatures (polled every 2 s; cards included) |
| `GET /api/inbox` | `{pending: [line + project], count}` |
| `GET /api/p/<slug>/state` | `{card, state, stale, feedback (last 40), feedback_unread, cards, ...}` |
| `GET /api/p/<slug>/cards[?status=open][&history=1]` | latest version of every card (history: every line) |
| `POST /api/p/<slug>/cards` | **agent, localhost only** (a request to any other bound address is refused): `{card}` or the card itself |
| `POST /api/p/<slug>/cards/<id>/withdraw` | **agent, localhost only**: `{note?}` |
| `POST /api/p/<slug>/cards/<id>/answer` | the desk: `{v, pick \| mix \| branch \| notes \| approve \| text \| accept \| none, context?}`; 409 if the card moved on or is not open |
| `GET /files/<slug>/<path>` | a file under the project dir, with HTTP Range |
| `POST /api/projects` | create a project |
| `POST /api/p/<slug>/feedback` | one legacy action (table below), optional `context` |
| `POST /api/p/<slug>/archive` / `brief` | archive toggle / one brief field edit |

## feedback.jsonl

One JSON object per line per project; the server adds `seq`, `ts`, `seen_revision`, `stage`, `by`.

| action | payload | meaning |
|---|---|---|
| `answer` | `card`, `v`, `card_type`, `title`, `beat?`, the answer fields (schema.md, cards.jsonl), `context?` | the human answered a card |
| `pick` | `target: {type: direction, id}` / `{type: variant, id, beat}` / `{type: kind, beat, kind}` | choose this (from a panel) |
| `mix` | directions: `attributes: {...}`; variants: `variants: [ids], text` | make a child from parts |
| `note` | `text`, `beat`, `variant`, `t` or `t0`+`t1`, `t_film`, `clock` | a remark at a time or range |
| `new-direction` | `target`, `text` | none of these |
| `approve` | `target`, `rev` | approve that revision |
| `reorder` | `order: [beat ids]` | new beat order |
| `branch` | `parent`, `beat`, `text` | a child of one variant |
| `edit` | `target: brief`, `field`, `value`, `rev` | the desk already wrote the brief field |
| `new-project` / `archive` / `unarchive` | | project lifecycle |

## Agent loop with cards

1. `read_feedback(slug)` (or `desk.py inbox`). Act on every line in order: an `answer` names its
   card; record the decision in `direction.json` as before (pick, mix, approval with the rev, notes
   with times and beat), render what changed.
2. `ack(slug)`.
3. Post the next decision as a card: one decision per card, a proposed `default` whenever you
   have one, media the human can judge (render with `render_preview` first). Withdraw cards that
   no longer apply. `set_agent_state(slug, "waiting", "one line")` when it is the human's turn,
   `"working"` before a long render.
4. One or two lines in chat. A pick that conflicts with a `must` lock is not executed: say why.

Not built: auth (localhost only by default), push into a running chat session (the agent polls
`read_feedback`), drawing on frames, voice notes.
