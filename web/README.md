# AgentFX web playground (P2)

Two modes in one app (Vite + TypeScript + Preact, CodeMirror 6):

- **Templates** (`/play?t=<id>`): gallery from `GET /templates`, a form generated from the manifest
  params (text, longtext, colour, number, enum, audio/image upload, data grid with CSV paste), live
  preview, format switch (videos) or multi-size picker with per-size thumbnails (stills), render with
  progress and download (one file, or a list plus a zip), batch from a pasted spreadsheet. English and
  German (`?lang=de`, else the browser language; template texts from the manifest `i18n` block).
- **Studio** (`/studio[?t=<id>]`): code editor with a Reel Script highlighter, lint squiggles and quick
  fixes, live preview, and a light timeline: scenes as blocks (drag the edge = `dur=`), `@` events as
  flags, element rows with verb chips, one sub-row per animated channel with keyframe diamonds joined by
  their ease curves. Drag keys/events/scene edges to retime (frame snap, Shift = 0.1 s, playhead
  magnet), double-click a channel lane to add a key, click to edit value and easing in the inspector.
  Edits go JSON -> edit -> canonical text and are patched into the author's text (comments survive),
  as editor transactions, so one undo history covers typing and timeline edits.

## Run

```sh
cd web && npm install
python3 dev/mock_api.py                                  # mock API on 127.0.0.1:8793 (CLI + fixtures)
AGENTFX_API=http://127.0.0.1:8793 npx vite --port 8794   # http://127.0.0.1:8794/play
```

`AGENTFX_API` defaults to `http://127.0.0.1:8780` (the real service). The mock uses the repo's
`templates/` when it has manifests, else `dev/fixtures/templates/` (or `--templates DIR`), runs the CLI
with `--tts none`, and takes the shared heavy-job lock for render/frame/sheet (`MOCK_FLOCK=` disables).

```sh
npm test                 # unit tests (timeline model, JSON <-> timeline <-> edits <-> JSON through the CLI, CSV, i18n)
npm run typecheck
python3 e2e/e2e.py       # browser e2e (needs mock + dev server); on the shared box: flock /tmp/claude-1000/agentfx-heavy.lock ...
python3 e2e/shots.py     # screenshots -> docs/screenshots/
npm run build            # static build in dist/ (VITE_API_BASE to point at another API origin)
```

## Embed

```html
<iframe src="https://play.agentfx.dev/embed/glitch-text?lang=de&theme=light"
        style="width:100%;height:640px;border:0" allow="clipboard-write"></iframe>
<script>
  addEventListener('message', (e) => {
    if (e.data?.type === 'agentfx:resize') document.querySelector('iframe').style.height = e.data.height + 'px';
  });
</script>
```

`/play?t=<id>&embed=1` is the same. Embed mode hides the header and gallery, keeps preview, form and
render, and links "Made with AgentFX" to the full playground.

## Live preview

The preview is the compiler's HTML backend (`POST /preview`, an API addition, see docs/PLATFORM.md) in
a sandboxed iframe, driven through `window.__reel` by postMessage: instant scrubbing and looping
playback, double-buffered so edits do not flash. It draws layout, text, shapes, gradients, keyframes
with every ease (springs and GSAP curves sampled to CSS `linear()`), enter/exit verbs, charts and
captions. It does not draw the native-only effects (fluid, gen, glitch/temporal glitch, rgbsplit, vhs,
halftone, kaleido, tunnel, glass, motion blur, time effects, visualizers, 3D...). When the script uses
any of them (or "Exact" is on in Studio, or the template is a still), pausing fetches the exact native
frame (`POST /frame`) and fades it over the live preview.
