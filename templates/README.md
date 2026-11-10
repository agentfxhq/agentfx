# AgentFX templates

Ready-made Reel Script videos with a handful of parameters. Each template is a directory with
`template.reel` (Reel Script with `{{param}}` placeholders), `manifest.json` (the params),
`preview.webp` (rendered from the defaults) and a one-paragraph `README.md`. The contract and the
template language are in [`docs/PLATFORM.md`](../docs/PLATFORM.md#templates-p3-writes-p1-and-p2-read).

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

All templates render in 16:9, 9:16 and 1:1 (`agentfx render --format 9:16`). They need an
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
