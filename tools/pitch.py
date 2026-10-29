#!/usr/bin/env python3
"""agentfx design pitch: divergent art directions as HTML style frames, then one contact sheet (L4).

usage:
  agentfx design pitch <brief.md|"brief text"> [--n 4] [--lenses swiss,editorial,flat,keynote] [--out dir]
                    [--generate [--model sonnet|haiku|opus]] [--render]
  agentfx design pitch --render <dir>          (render the frame-*.html in <dir> as one contact sheet)

1. Prompt pack: <out>/direction-<k>/PROMPT.md per direction = brief + a lens (essay, tokens, negatives)
   + one constraint card (forces divergence) + the frame contract (1920x1080, Blitz-safe CSS, fonts, L0
   rules). An agent (or --generate, via the Anthropic API) writes <out>/direction-<k>/frame.html.
2. --render: every frame is converted into one scene of a generated .reel (`html` block, rendered
   natively by Blitz, no browser) and `agentfx sheet` renders all of them as one contact sheet:
   <out>/pitch-sheet.png. Pick one (human or `agentfx critique`), record it with `agentfx picks add`, then
   `agentfx design harvest <out>/direction-<k>/frame.html -o design.reel`.
"""
import json, os, re, subprocess, sys, threading, time
from pathlib import Path

HOME = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HOME / "tools"))

CARDS = [
    "One accent color only, used on the single element the frame is about.",
    "Type is the image: the key word or number is the largest object in the frame.",
    "Everything sits on a 12-column grid with a visible structure (rules or columns).",
    "No boxes: no cards, no tiles, no rounded rectangles; structure by type, rules and space.",
    "Data as landscape: the data shape fills the full frame width.",
    "Asymmetric: the hero sits on the left third, the right side carries the detail.",
    "Material, not effects: one physical medium (paper, ink, blueprint, glass) with its own texture rules.",
    "Big numerals: every step and value is a numeral of at least 120 px.",
]

CONTRACT = """## Frame contract (renderer: Blitz, a native HTML/CSS engine; no browser, no JavaScript)
- One complete HTML page, exactly 1920x1080: `html, body {margin:0; width:1920px; height:1080px; overflow:hidden}`.
- Allowed: flexbox, CSS grid, absolute positioning, borders, border-radius, box-shadow, linear/radial gradients,
  inline SVG (stroke icons, lines, arrows), letter-spacing, text-transform, font-variant-numeric.
- Not allowed: JavaScript, external URLs or web fonts, <img> with remote src, CSS filters except one blur(),
  backdrop-filter, mix-blend-mode, clip-path, @keyframes, transitions, vw/vh units.
- Fonts (only these exist): Inter, Source Serif 4, Libre Franklin, Nunito, JetBrains Mono, Space Grotesk, Fraunces,
  Noto Sans Mono. At most two families plus one mono.
- Video rules (L0): reading text >= 28 px, labels >= 22 px, display 96 to 200 px; keywords, not sentences;
  the hero fills >= 40 % of the frame; one focal point; 6 to 7 % safe margins; text contrast >= 4.5:1.
- No em-dashes in any text.
- Put the design tokens as CSS custom properties on :root (colors as roles: --bg --surface --text --muted --accent ...).
"""


def lens_text(name):
    d = HOME / "design/lenses" / name
    if not d.exists():
        return ""
    essay = (d / "LENS.md").read_text() if (d / "LENS.md").exists() else ""
    tok = (d / "design.reel").read_text() if (d / "design.reel").exists() else ""
    return f"## Lens: {name}\n\n{essay}\n\n### Tokens (Reel Script design spec)\n```\n{tok}```\n"


def prompt_for(brief, lens, card, k):
    return f"""# Pitch direction {k}: write one HTML style frame

You are an art director pitching a look for an explainer video. Draw the single most telling still of the
video described in the brief, as HTML/CSS, in the lens below, obeying the constraint card. Make it look
designed and finished, not like a web page: video type sizes, full-frame composition, per-step detail
(done checks, the active step highlighted, a detail panel) when the brief is a process.

## Brief
{brief}

## Constraint card
{card}

{lens_text(lens)}
{CONTRACT}
Reply with only the HTML document, nothing else.
"""


def frames_to_reel(frames, out_reel):
    """style-frame pages -> one .reel, one scene per frame (html blocks, native)"""
    lines = ['video "pitch" size=1920x1080 fps=30 captions=off', "theme studio-light", "  layout safe=0%", "  lint min-contrast=1", ""]
    for i, f in enumerate(frames):
        html = Path(f).read_text()
        styles = "\n".join(re.findall(r"<style[^>]*>(.*?)</style>", html, re.S | re.I))
        body = re.search(r"<body[^>]*>(.*)</body>", html, re.S | re.I)
        body = body.group(1) if body else html
        body = re.sub(r"<script.*?</script>", "", body, flags=re.S | re.I)
        css = re.sub(r"(^|[\s,{}])html\s*,\s*body(?=[\s,{])", r"\1.sfroot", styles)
        css = re.sub(r"(^|[\s,{}])(html|body|:root)(?=[\s,{:.\[])", r"\1.sfroot", css)
        block = f'<style>{css}</style><div class="sfroot" style="position:relative;width:1920px;height:1080px;overflow:hidden">{body}</div>'
        block = "\n".join("    " + l for l in block.splitlines() if l.strip())
        lines += [f"scene f{i + 1} dur=1s nocaptions", f"  html #frame{i + 1} x=0 y=0 w=1920 h=1080 |", block, ""]
    Path(out_reel).write_text("\n".join(lines) + "\n")


def render(dirp):
    """each frame is its own document (styles must not leak between frames): one .reel per frame,
    rendered natively (`agentfx frame`), then one labelled contact sheet"""
    dirp = Path(dirp)
    frames = sorted(dirp.glob("direction-*/frame.html")) or sorted(dirp.glob("frame*.html"))
    if not frames:
        print(f"no frames in {dirp}", file=sys.stderr)
        sys.exit(1)
    import shutil
    cands = [os.environ.get("AGENTFX_BIN") or os.environ.get("REEL_BIN"), str(HOME.parent.parent / "bin" / "agentfx"),
             shutil.which("agentfx"), str(HOME / "target/release/agentfx")]
    local = next((Path(c) for c in cands if c and Path(c).is_file()), None)
    if local is None:
        sys.exit("agentfx binary not found (set AGENTFX_BIN or put agentfx on PATH)")
    pngs = []
    for i, f in enumerate(frames):
        wd = dirp / f"render-{i + 1}"
        wd.mkdir(exist_ok=True)
        reel = wd / "frame.reel"
        frames_to_reel([f], reel)
        png = wd / "out-frame.png"
        subprocess.run([str(local), "frame", str(reel), "--at", "0.5", "--no-tts", "-o", str(png)], check=False, capture_output=True)
        pngs.append((png, f))
    from PIL import Image, ImageDraw
    cw, ch = 640, 360
    cols = min(4, len(pngs))
    rows = (len(pngs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (cw + 10) + 10, rows * (ch + 34) + 10), (18, 18, 18))
    d = ImageDraw.Draw(sheet)
    for k, (png, f) in enumerate(pngs):
        x, y = 10 + (k % cols) * (cw + 10), 10 + (k // cols) * (ch + 34)
        if png.exists():
            sheet.paste(Image.open(png).convert("RGB").resize((cw, ch)), (x, y))
        else:
            d.text((x + 10, y + 10), "render failed", fill=(255, 80, 80))
        label = f.parent.name if f.name == "frame.html" else f.stem
        meta = f.parent / "meta.json"
        if meta.exists():
            m = json.loads(meta.read_text())
            label += f" | {m.get('lens')} | {m.get('card', '')[:60]}"
        d.text((x + 2, y + ch + 8), label, fill=(230, 230, 230))
    sheet.save(dirp / "pitch-sheet.png")
    print(json.dumps({"sheet": str(dirp / "pitch-sheet.png"), "frames": [str(f) for _, f in pngs]}))


def generate(prompts, model):
    import critique  # key loading + usage log

    critique.load_key()
    import anthropic

    client = anthropic.Anthropic()
    mid = critique.MODELS.get(model, model)

    def one(path, prompt):
        r = client.messages.create(model=mid, max_tokens=16000, messages=[{"role": "user", "content": prompt}], output_config={"effort": "medium"})
        critique.log_usage(mid, r.usage, f"pitch:{path.parent.name}", kind="pitch")
        txt = "".join(b.text for b in r.content if b.type == "text")
        m = re.search(r"<!doctype html.*</html>", txt, re.S | re.I) or re.search(r"<html.*</html>", txt, re.S | re.I)
        path.write_text(m.group(0) if m else txt)

    th = [threading.Thread(target=one, args=(p, pr)) for p, pr in prompts]
    for t in th:
        t.start()
    for t in th:
        t.join()


def main():
    a = sys.argv[1:]

    def opt(k, d=None):
        if k in a:
            i = a.index(k)
            v = a[i + 1]
            del a[i:i + 2]
            return v
        return d

    if a and a[0] == "--render":
        render(a[1])
        return
    n = int(opt("--n", "4"))
    lenses = (opt("--lenses") or "swiss,editorial,flat,keynote").split(",")
    model = opt("--model", "sonnet")
    gen = "--generate" in a
    rend = "--render" in a
    a = [x for x in a if not x.startswith("--")]
    if not a:
        print(__doc__)
        sys.exit(2)
    brief = Path(a[0]).read_text() if Path(a[0]).exists() else a[0]
    slug = re.sub(r"[^a-z0-9]+", "-", brief.lower())[:32].strip("-")
    out = Path(opt("--out") or (HOME / "out/pitch" / slug))
    out.mkdir(parents=True, exist_ok=True)
    prompts = []
    for k in range(n):
        lens = lenses[k % len(lenses)]
        card = CARDS[(k * 3) % len(CARDS)]
        d = out / f"direction-{k + 1}"
        d.mkdir(exist_ok=True)
        p = prompt_for(brief, lens, card, k + 1)
        (d / "PROMPT.md").write_text(p)
        (d / "meta.json").write_text(json.dumps({"lens": lens, "card": card}))
        prompts.append((d / "frame.html", p))
    print(json.dumps({"out": str(out), "directions": [{"dir": str(p.parent), "lens": json.loads((p.parent / 'meta.json').read_text())["lens"]} for p, _ in prompts]}, indent=1))
    if gen:
        generate(prompts, model)
    if rend or gen:
        render(out)


if __name__ == "__main__":
    main()
