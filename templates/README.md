# AgentFX templates

Ready-made Reel Script sources with a handful of parameters. Each template is a directory with
`template.reel` (Reel Script with `{{param}}` placeholders), `manifest.json` (the params),
a preview rendered from the defaults and a one-paragraph `README.md`. The contract and the
template language are in [`docs/PLATFORM.md`](../docs/PLATFORM.md#templates-p3-writes-p1-and-p2-read);
`render_template.py` is the single implementation of substitution (stdlib only).

## Videos

| Template | What it makes | Search intent |
|---|---|---|
| [music-visualizer](music-visualizer/) | Audio-reactive spectrum, radial ring around a record, or waveform, with title and artist, for any uploaded song | music visualizer, audio visualizer |
| [lyric-video](lyric-video/) | Lyrics line by line on the song's beat grid (or per-line times), karaoke sweep or word rise | lyric video maker |
| [glitch-text](glitch-text/) | A word that boots up through a corrupted signal: block glitch, RGB split, datamosh bursts | glitch effect |
| [vhs-intro](vhs-intro/) | Retro tape intro: snow, tracking lock, glowing title over a striped sun, deck display | vhs effect |
| [animated-text](animated-text/) | Kinetic typography for one headline: rise, focus, slide or cinematic | animated text generator, kinetic typography |
| [infinite-zoom](infinite-zoom/) | Seamless infinite zoom through neon frames, rings or portals | infinite zoom |
| [halftone-poster](halftone-poster/) | Poster printed as CMYK or one-ink halftone dots that resolve from coarse to fine, optional photo | halftone effect |
| [logo-reveal](logo-reveal/) | Uploaded logo assembles from shards or wipes on under a light sweep; text logo fallback | logo animation |

All video templates render in 16:9, 9:16 and 1:1 (`agentfx render --format 9:16`). They need an
`agentfx` build that has the distort pack and Text v2 (commit 27d2a14 or later); older installs
reject `dblur` with `E_CHANNEL`.

```bash
python3 templates/render_template.py --list
python3 templates/render_template.py glitch-text --set text="HELLO" --set style=mono -o out/glitch.reel --copy-assets out/
python3 templates/render_template.py lyric-video --set lyrics=@my-lyrics.txt --set bpm=96 --set offset=1.2 -o out/lyrics.reel --copy-assets out/
agentfx render out/glitch.reel --format 9:16 -o out/glitch.mp4
python3 templates/test_templates.py      # unit tests + lint of every template with default and edge inputs
```

`audio` and `image` params take the name of a file next to the instantiated source (the service
maps uploads to such names; `--copy-assets` copies the template's own default files). Default
audio licence: [LICENSE-audio.md](LICENSE-audio.md) (CC0).

## Still graphics

Still templates (`"kind": "still"`) render one exact frame per output size with `agentfx frame`:
the same engine as the videos, so text is exact, brand colours are tokens, and one design covers
every channel. Each template draws into a page of design units (short side 1080) that is scaled
to the output pixels, so print sizes (A4/A5/A3 at 300 dpi) stay crisp vectors.

| Template | What it makes | Sizes | Search intent |
|---|---|---|---|
| [flyer](flyer/) | Event or sale flyer: photo, serif headline, label, subline, date and place | A4, A5, A3, Instagram post, portrait, story | flyer erstellen, flyer maker, poster maker |
| [social-set](social-set/) | One message with logo and call to action for every social channel | Instagram post, portrait, story, LinkedIn, X, OG image, Facebook | social media post, og image |
| [collage](collage/) | 2 to 9 photos as grid, mosaic with a hero, or polaroids; optional title | Instagram post, portrait, story, Facebook, X, Pinterest, A4 landscape | collage erstellen, photo collage |
| [chart](chart/) | Bar, line, area, donut or KPI tiles from a table | Instagram post, portrait, story, LinkedIn, X, OG image, Full HD | diagramm erstellen, chart maker |
| [youtube-thumbnail](youtube-thumbnail/) | Huge outlined text, cut-out subject, arrow / circle / burst, badge | 1280x720, Full HD | youtube thumbnail maker |
| [banner](banner/) | Display ads and social covers from one message | 728x90, 300x250, 160x600, 300x600, 970x250, 320x100, LinkedIn / X / Facebook covers, YouTube channel art | banner erstellen, banner maker |
| [mockup](mockup/) | Screenshot on a 3D phone or a laptop, artwork on a t-shirt | Instagram post, portrait, story, X, OG image, Full HD | mockup generator, t-shirt mockup |
| [menu](menu/) | Typeset restaurant menu from a table (section, dish, description, price) | A4, A5, A3, Instagram portrait, story, 16:9 menu board | speisekarte erstellen, menu maker |
| [certificate](certificate/) | Certificate with name, course, date, signature and seal; one per table row | A4 / Letter landscape, A4 portrait, LinkedIn, Instagram | certificate generator, urkunde |

The animated sibling of `chart` is the video template [chart-animated](chart-animated/).

```bash
python3 templates/render_template.py flyer --size a4-flyer --set headline="Night Market" -o out/flyer.reel --copy-assets out/
agentfx frame out/flyer.reel --at 0s -o out/flyer-a4.png
python3 templates/render_template.py menu --size story --set rows=@menu.csv -o out/menu.reel --copy-assets out/
python3 templates/render_template.py certificate --size a4-landscape \
  --set 'recipients=[{"name": "Ada"}, {"name": "Grace"}]' --set index=2 -o out/cert-2.reel --copy-assets out/
python3 templates/test_still_templates.py          # unit tests, every size with edge inputs, lint
AGENTFX_STILL_RENDER=1 python3 templates/test_still_templates.py   # plus one frame per size
```

Tables (`data` params) take a JSON array of objects or CSV text (header row; `,` `;` or tab);
`--set rows=@file.csv` reads a file. `image` params name a PNG next to the instantiated source
(the service maps uploads; `--copy-assets` copies the template's default images). The default
images are drawn by code (`_stills/make_placeholders.py`, `_stills/screen-*.reel`), so they carry
no third-party rights.
