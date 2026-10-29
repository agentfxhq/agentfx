#!/usr/bin/env python3
"""agentfx critique: a VLM critic against the L0 rubric (design/PRINCIPLES.md), via the Anthropic API.

usage:
  agentfx critique <sheet.png|video.mp4|frames...> [--rubric all|visual|motion] [--model haiku|sonnet|opus|fable]
                [--brief "what the video should say"] [--prev findings.json] [--tag label] [-o findings.json]

- A PNG is critiqued as a contact sheet as is. An MP4 is sampled: 12 evenly spaced frames become a
  labelled contact sheet, and for the motion rubric an 8-frame strip at 0.1 s steps around the largest
  visual change (motion read from stills: what moves, in which order, what holds).
- Output: JSON {scores, overall, findings[{principle, severity, where, problem, fix}], tells, verdict},
  validated by the API (structured output). With --prev it also reports which earlier findings are fixed
  (critic loop rounds 2 and 3).
- Every call's token usage and cost is appended to $AGENTFX_CACHE/api-usage.jsonl, else cache/api-usage.jsonl
  next to this checkout (never the key).
- The key is read from ANTHROPIC_API_KEY (never printed or logged).
"""
import base64, io, json, os, subprocess, sys, tempfile, time
from pathlib import Path

HOME = Path(__file__).resolve().parent.parent
MODELS = {"haiku": "claude-haiku-5-5", "sonnet": "claude-sonnet-5-5", "opus": "claude-opus-5-5", "fable": "claude-fable-5-1"}
# $ per million tokens: input, output (cache write 1.25x input, cache read 0.1x input)
PRICE = {"claude-haiku-5-5": (0.10, 0.50), "claude-sonnet-5-5": (2.0, 10.0), "claude-opus-5-5": (4.0, 20.0), "claude-fable-5-1": (10.0, 50.0)}
USAGE_LOG = Path((os.environ.get("AGENTFX_CACHE") or os.environ.get("REEL_CACHE")) or HOME / "cache") / "api-usage.jsonl"


def load_key():
    if not os.environ.get("ANTHROPIC_API_KEY"):
        sys.exit("critique: set ANTHROPIC_API_KEY")


def log_usage(model, usage, tag, kind="critique"):
    pin, pout = PRICE.get(model, (4.0, 20.0))
    u = {k: getattr(usage, k, 0) or 0 for k in ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]}
    cost = (u["input_tokens"] * pin + u["output_tokens"] * pout + u["cache_creation_input_tokens"] * pin * 1.25 + u["cache_read_input_tokens"] * pin * 0.1) / 1e6
    USAGE_LOG.parent.mkdir(parents=True, exist_ok=True)
    with open(USAGE_LOG, "a") as f:
        f.write(json.dumps({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "kind": kind, "tag": tag, "model": model, **u, "cost_usd": round(cost, 6)}) + "\n")
    return cost


RUBRIC = """You are a harsh motion-design director reviewing an explainer video (1080p, 16:9 or 9:16) frame by frame.
You see a contact sheet (frames in time order, each labelled with its time and scene) and possibly a motion strip
(consecutive frames 0.1 s apart). Judge it as a viewer sees it once, at speed, while a narrator talks.

Rubric (L0 principles of the house design layer):
Visual
V1 one focal point per beat: the element the narration names is the one that changes; everything else holds.
V2 hierarchy: clear size/weight/contrast steps (>= 1.5x) between levels.
V3 type at 1080p: reading text >= 28 px, display 96-200 px; too many words is cut, never shrunk.
V4 text density: keywords on screen, not sentences; one idea per frame.
V5 contrast: text clearly readable on its background.
V6 safe area: nothing important near the edges.
V7 scale fill: the hero of each frame fills >= 40 % of the frame; no small elements floating in a big empty frame.
V8 consistency: the same role looks the same in every scene.
V9 color discipline: roles, one rationed accent.
V10 depth: background, mid content, foreground accents; a coherent elevation/texture.
V11 grid and alignment: shared edges, a deliberate composition (not everything centered by default).
V12 per-step detail: a growing diagram shows state (done checks, active highlight, progress, a detail panel).
Motion (from strips and from what changes between sheet frames)
M1 enter decelerating, exit accelerating, moves ease both ways.
M2 exits shorter than entrances.
M3 staggers readable (60-165 ms), M4 one orchestrated move per beat (not many unrelated moves at once).
M5 overlap rather than strict sequence; M6 small travel on arrival; M7 landed titles hold still.
M8 motion follows the narration and reading direction; M9 holds long enough to read; M10 no decorative loops.
Tells of a generated video (name any you see): small things floating in a dark void; everything centered;
three cards in a row; a KPI grid; glow/particles/purple-blue gradients as decoration; glass cards everywhere;
every element fading and sliding up the same way; a diagram dropped on screen at once.

Scores are integers 1-5 per dimension (5 = a professional motion designer would ship it), overall 1-10.
Be specific: every finding names the frame time, the principle, the problem, and an exact fix
(e.g. "scale the diagram up 1.4x so the nodes span 80 % of the width"). Severity P0 breaks the read or
looks broken, P1 weakens the video, P2 polish. At most 8 findings, most important first."""

SCHEMA = {
    "type": "object",
    "properties": {
        "scores": {
            "type": "object",
            "properties": {k: {"type": "integer"} for k in ["focal", "hierarchy", "type_legibility", "density", "scale_fill", "consistency", "color", "depth", "composition", "detail", "motion"]},
            "required": ["focal", "hierarchy", "type_legibility", "density", "scale_fill", "consistency", "color", "depth", "composition", "detail", "motion"],
            "additionalProperties": False,
        },
        "overall": {"type": "integer"},
        "findings": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {"principle": {"type": "string"}, "severity": {"type": "string", "enum": ["P0", "P1", "P2"]}, "where": {"type": "string"}, "problem": {"type": "string"}, "fix": {"type": "string"}},
                "required": ["principle", "severity", "where", "problem", "fix"],
                "additionalProperties": False,
            },
        },
        "tells": {"type": "array", "items": {"type": "string"}},
        "fixed_since_prev": {"type": "array", "items": {"type": "string"}},
        "verdict": {"type": "string"},
    },
    "required": ["scores", "overall", "findings", "tells", "fixed_since_prev", "verdict"],
    "additionalProperties": False,
}


def ffmpeg():
    return (os.environ.get("AGENTFX_FFMPEG") or os.environ.get("REEL_FFMPEG") or "ffmpeg")


def probe_dur(mp4):
    out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", mp4], capture_output=True, text=True).stdout.strip()
    return float(out or 0)


def grab(mp4, t, w=640):
    r = subprocess.run([ffmpeg(), "-v", "error", "-ss", f"{t:.3f}", "-i", mp4, "-frames:v", "1", "-vf", f"scale={w}:-2", "-f", "image2pipe", "-vcodec", "png", "-"], capture_output=True)
    from PIL import Image
    return Image.open(io.BytesIO(r.stdout)).convert("RGB")


def grid(images, labels, cols, cell_w):
    from PIL import Image, ImageDraw
    h = images[0].height * cell_w // images[0].width
    rows = (len(images) + cols - 1) // cols
    pad, lab = 8, 26
    sheet = Image.new("RGB", (cols * (cell_w + pad) + pad, rows * (h + lab + pad) + pad), (18, 18, 18))
    d = ImageDraw.Draw(sheet)
    for i, (im, l) in enumerate(zip(images, labels)):
        x = pad + (i % cols) * (cell_w + pad)
        y = pad + (i // cols) * (h + lab + pad)
        sheet.paste(im.resize((cell_w, h)), (x, y))
        d.text((x + 2, y + h + 5), l, fill=(230, 230, 230))
    return sheet


def png_b64(im):
    b = io.BytesIO()
    im.save(b, "PNG", optimize=True)
    return base64.standard_b64encode(b.getvalue()).decode()


def video_inputs(mp4, rubric):
    dur = probe_dur(mp4)
    ts = [(k + 0.5) / 12 * dur for k in range(12)]
    portrait = grab(mp4, 0.1, 160).height > grab(mp4, 0.1, 160).width
    w = 360 if portrait else 560
    frames = [grab(mp4, t, w) for t in ts]
    out = [("contact sheet, 12 frames evenly over the video", grid(frames, [f"{t:.1f}s" for t in ts], 6 if portrait else 4, w))]
    if rubric in ("all", "motion"):
        # motion strip: 8 frames at 0.1 s around the largest change between two sample frames
        from PIL import ImageChops, ImageStat
        diffs = [sum(ImageStat.Stat(ImageChops.difference(a, b)).mean) for a, b in zip(frames, frames[1:])]
        k = max(range(len(diffs)), key=lambda i: diffs[i])
        # find the onset of motion between ts[k] and ts[k+1]
        t0 = ts[k] + (ts[k + 1] - ts[k]) * 0.25
        strip_t = [t0 + 0.1 * i for i in range(8)]
        sw = 300 if portrait else 420
        strip = [grab(mp4, t, sw) for t in strip_t]
        out.append(("motion strip: 8 consecutive frames 0.1 s apart", grid(strip, [f"{t:.2f}s" for t in strip_t], 8 if portrait else 4, sw)))
    return out, dur


def main():
    a = sys.argv[1:]

    def opt(k, d=None):
        if k in a:
            i = a.index(k)
            v = a[i + 1]
            del a[i:i + 2]
            return v
        return d

    rubric = opt("--rubric", "all")
    model = MODELS.get(opt("--model", "haiku"), opt("--model", "claude-haiku-5-5"))
    brief = opt("--brief", "")
    prev = opt("--prev")
    tag = opt("--tag", "")
    out = opt("-o")
    effort = opt("--effort", "medium")
    files = [x for x in a if not x.startswith("--")]
    if not files:
        print(__doc__)
        sys.exit(2)
    images = []
    meta = {}
    for f in files:
        if f.endswith(".mp4"):
            iv, dur = video_inputs(f, rubric)
            images += iv
            meta["duration_s"] = round(dur, 2)
        else:
            from PIL import Image
            im = Image.open(f).convert("RGB")
            if im.width > 2400:
                im = im.resize((2400, im.height * 2400 // im.width))
            images.append((f"contact sheet {Path(f).name}", im))
    load_key()
    import anthropic

    client = anthropic.Anthropic()
    content = []
    for label, im in images:
        content.append({"type": "text", "text": label})
        content.append({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png_b64(im)}})
    ask = f"Rubric focus: {rubric}. "
    if brief:
        ask += f"The brief the video was made for: {brief}\n"
    if prev:
        ask += "Findings from the previous round (say which are fixed in fixed_since_prev, by their problem text):\n" + Path(prev).read_text()[:6000]
    ask += "\nReturn the JSON review."
    content.append({"type": "text", "text": ask})
    t0 = time.time()
    resp = client.messages.create(
        model=model,
        max_tokens=8000,
        system=[{"type": "text", "text": RUBRIC, "cache_control": {"type": "ephemeral"}}],
        messages=[{"role": "user", "content": content}],
        output_config={"effort": effort, "format": {"type": "json_schema", "schema": SCHEMA}},
    )
    cost = log_usage(model, resp.usage, tag)
    if resp.stop_reason == "refusal":
        print(json.dumps({"error": "refusal"}))
        sys.exit(1)
    text = next(b.text for b in resp.content if b.type == "text")
    data = json.loads(text)
    sc = data["scores"]
    data["meta"] = {**meta, "model": model, "rubric": rubric, "inputs": files, "cost_usd": round(cost, 5), "wall_s": round(time.time() - t0, 1),
                    "mean_score": round(sum(sc.values()) / len(sc), 2)}
    s = json.dumps(data, indent=1, ensure_ascii=False)
    if out:
        Path(out).write_text(s)
    print(s)


if __name__ == "__main__":
    main()
