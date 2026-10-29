#!/usr/bin/env python3
"""TTS with word timings for Reel Script. Engine per request ("engine" field, from `--tts` / $AGENTFX_TTS):

  piper (default)  local, offline: the `piper` CLI (piper-tts, a separate program run as a subprocess).
                   Default voice `en_US-ljspeech-medium` (trained from scratch on the public-domain
                   LJ Speech dataset). Voices are looked up as <name>.onnx(+.json) in $AGENTFX_PIPER_VOICES,
                   then the `piper-voices` folder next to the TTS cache, and downloaded from the
                   rhasspy/piper-voices repository on Hugging Face on first use (AGENTFX_PIPER_DOWNLOAD=0 disables).
                   Word times are spread over the real audio by word length and snapped to pauses.
  edge (opt-in)    Microsoft Edge's online read-aloud voices via the third-party `edge-tts` package.
                   It uses an undocumented service without published terms for third-party use; it is
                   never the default, check whether you may use it before you do.

stdin : {"engine": "piper", "voice": "...", "rate": "+6%", "cache": "cache/tts", "items": [{"key": "intro-0", "text": "spoken text"}]}
stdout: {"intro-0": {"mp3": "cache/tts/<hash>.mp3", "dur": 4.51, "words": [["Hello", 0.02, 0.31], ...]}, ...}

Each sentence is cached by sha256(engine|voice|rate|text) (edge: sha256(voice|rate|text)). Audio is cut to [first word - 30 ms, last word end + 80 ms]
and word times are shifted to match. Exit status 3 means the engine is not installed.
"""
import array, asyncio, hashlib, json, os, re, shutil, subprocess, sys, time, urllib.request, wave
from pathlib import Path

FFMPEG = (os.environ.get("AGENTFX_FFMPEG") or os.environ.get("REEL_FFMPEG") or "ffmpeg")
FFPROBE = (os.environ.get("AGENTFX_FFPROBE") or os.environ.get("REEL_FFPROBE") or "ffprobe")
PRE, POST = 0.03, 0.08
DEFAULT_VOICE = {"piper": "en_US-ljspeech-medium", "edge": "de-DE-SeraphinaMultilingualNeural"}
PIPER_VOICE = re.compile(r"^([a-z]{2,3})_([A-Z]{2})-([A-Za-z0-9_]+)-(x_low|low|medium|high)$")
HF = "https://huggingface.co/rhasspy/piper-voices/resolve/main"


class Missing(Exception):
    """The engine (or its voice) is not available on this machine."""


def probe(p):
    return float(subprocess.run([FFPROBE, "-v", "error", "-show_entries", "format=duration", "-of",
                                 "default=noprint_wrappers=1:nokey=1", str(p)], capture_output=True, text=True, check=True).stdout)


async def edge_synth(text, voice, rate, raw):
    try:
        import edge_tts
    except ImportError:
        raise Missing("edge-tts is not installed (pip install edge-tts)")
    c = edge_tts.Communicate(text, voice, rate=rate, boundary="WordBoundary")
    words, audio = [], bytearray()
    async for ch in c.stream():
        if ch["type"] == "audio":
            audio += ch["data"]
        elif ch["type"] == "WordBoundary":
            s = ch["offset"] / 1e7
            words.append([ch["text"], round(s, 3), round(s + ch["duration"] / 1e7, 3)])
    raw.write_bytes(bytes(audio))
    return words


def piper_bin():
    for c in ((os.environ.get("AGENTFX_PIPER") or os.environ.get("REEL_PIPER")), os.path.join(os.path.dirname(sys.executable), "piper"), shutil.which("piper")):
        if c and os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    raise Missing("piper is not installed (pip install piper-tts, or set AGENTFX_PIPER)")


def piper_voice(voice, cache):
    """Path of the voice model; downloads it into <cache>/../piper-voices when missing."""
    if voice.endswith(".onnx"):
        if not os.path.isfile(voice):
            raise Missing(f"piper voice {voice} not found")
        return voice
    m = PIPER_VOICE.match(voice)
    if not m:
        raise SystemExit(f"not a piper voice name: {voice} (e.g. en_US-ljspeech-medium)")
    dirs = [d for d in ((os.environ.get("AGENTFX_PIPER_VOICES") or os.environ.get("REEL_PIPER_VOICES")), str(cache.parent / "piper-voices")) if d]
    for d in dirs:
        p = Path(d) / f"{voice}.onnx"
        if p.is_file() and p.with_suffix(".onnx.json").is_file():
            return str(p)
    if (os.environ.get("AGENTFX_PIPER_DOWNLOAD") or os.environ.get("REEL_PIPER_DOWNLOAD") or "1") == "0":
        raise Missing(f"piper voice {voice} not found in {', '.join(dirs)}")
    lang, region, name, quality = m.groups()
    dst = Path(dirs[-1])
    dst.mkdir(parents=True, exist_ok=True)
    for suffix in (".onnx.json", ".onnx"):
        url = f"{HF}/{lang}/{lang}_{region}/{name}/{quality}/{voice}{suffix}"
        print(f"tts.py: downloading {url}", file=sys.stderr)
        tmp = dst / f"{voice}{suffix}.part"
        with urllib.request.urlopen(url, timeout=60) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f)
        tmp.rename(dst / f"{voice}{suffix}")
    return str(dst / f"{voice}.onnx")


def rate_factor(rate):
    """edge-style rate (`+6%`) -> speed factor (1.06)."""
    m = re.fullmatch(r"\s*([+-]?\d+(?:\.\d+)?)\s*%\s*", rate or "")
    return max(0.25, 1.0 + float(m.group(1)) / 100.0) if m else 1.0


def spread_words(text, pcm, sr):
    """Word times for audio without word boundaries: split the voiced span by word length (a
    sentence mark adds a pause share), then snap each boundary to the quietest 10 ms frame nearby."""
    toks = text.split()
    hop = max(1, sr // 100)
    energy = [sum(abs(x) for x in pcm[i:i + hop]) / hop for i in range(0, len(pcm), hop)] or [0.0]
    floor = max(energy) * 0.04
    voiced = [i for i, e in enumerate(energy) if e > floor] or [0, len(energy) - 1]
    a, b = voiced[0] / 100.0, (voiced[-1] + 1) / 100.0
    weight = [len(t) + (4 if t[-1] in ".!?;:" else 2 if t[-1] == "," else 0) + 1 for t in toks]
    total = sum(weight) or 1
    edges, t = [a], a
    for w in weight:
        t += (b - a) * w / total
        edges.append(t)
    def snap(x):
        i = int(round(x * 100))
        lo, hi = max(0, i - 8), min(len(energy) - 1, i + 8)
        if lo >= hi:
            return x
        j = min(range(lo, hi + 1), key=lambda k: (energy[k], abs(k - i)))
        return j / 100.0
    inner = [snap(x) for x in edges[1:-1]]
    edges = [a] + inner + [b]
    for i in range(1, len(edges)):  # keep them ordered
        edges[i] = max(edges[i], edges[i - 1] + 0.02)
    words = []
    for i, tok in enumerate(toks):
        s, e = edges[i], edges[i + 1]
        if tok[-1] in ".!?;:,":  # the pause share sits after the word
            e = s + (e - s) * (len(tok) + 1) / weight[i]
        words.append([tok, round(s, 3), round(e, 3)])
    return words


def piper_synth(text, voice, rate, raw, cache):
    exe, model = piper_bin(), piper_voice(voice, cache)
    wav = raw.with_suffix(".wav")
    subprocess.run([exe, "-m", model, "-f", str(wav), "--length-scale", f"{1.0 / rate_factor(rate):.4f}"],
                   input=text.encode(), check=True, capture_output=True)
    with wave.open(str(wav), "rb") as w:
        sr, n, width = w.getframerate(), w.getnframes(), w.getsampwidth()
        frames = w.readframes(n)
    pcm = array.array("h", frames) if width == 2 else array.array("h")
    words = spread_words(text, pcm, sr) if len(pcm) else []
    subprocess.run([FFMPEG, "-v", "error", "-y", "-i", str(wav), "-b:a", "160k", str(raw)], check=True)
    wav.unlink()
    return words


def one(item, engine, voice, rate, cache):
    text = item["text"]
    key = f"{voice}|{rate}|{text}" if engine == "edge" else f"{engine}|{voice}|{rate}|{text}"  # edge: pre-engine cache keys
    h = hashlib.sha256(key.encode()).hexdigest()[:16]
    mp3, meta = cache / f"{h}.mp3", cache / f"{h}.json"
    if meta.exists() and mp3.exists():
        return json.loads(meta.read_text())
    raw = cache / f"{h}.raw.mp3"
    if engine == "piper":
        words = piper_synth(text, voice, rate, raw, cache)
    else:
        for attempt in range(6):  # the edge endpoint answers 403 now and then
            try:
                words = asyncio.run(edge_synth(text, voice, rate, raw))
                break
            except Missing:
                raise
            except Exception as e:  # noqa
                err = e
                time.sleep(3 * (attempt + 1))
        else:
            raise SystemExit(f"tts failed for {item['key']}: {err}")
    total = probe(raw)
    est = not words
    if est:  # fallback: spread the words over the audio by character length
        toks = text.split()
        n = sum(len(t) for t in toks) or 1
        t, words = 0.05, []
        for tok in toks:
            d = (total - 0.1) * len(tok) / n
            words.append([tok, round(t, 3), round(t + d, 3)])
            t += d
    a = max(0.0, words[0][1] - PRE)
    b = min(total, words[-1][2] + POST)
    subprocess.run([FFMPEG, "-v", "error", "-y", "-ss", f"{a:.3f}", "-to", f"{b:.3f}", "-i", str(raw),
                    "-ar", "48000", "-ac", "1", "-b:a", "160k", str(mp3)], check=True)
    raw.unlink()
    words = [[w, round(s - a, 3), round(e - a, 3)] for w, s, e in words]
    out = {"mp3": str(mp3), "dur": round(probe(mp3), 3), "words": words, "estimated": est}
    meta.write_text(json.dumps(out, ensure_ascii=False))
    return out


def main():
    req = json.load(sys.stdin)
    cache = Path(req.get("cache", "cache/tts"))
    cache.mkdir(parents=True, exist_ok=True)
    engine = req.get("engine") or "piper"
    if engine not in DEFAULT_VOICE:
        raise SystemExit(f"unknown TTS engine {engine!r} (piper, edge, none)")
    voice = req.get("voice") or DEFAULT_VOICE[engine]
    if engine == "piper" and not (voice.endswith(".onnx") or PIPER_VOICE.match(voice)):
        print(f"tts.py: {voice} is not a piper voice, using {DEFAULT_VOICE['piper']}", file=sys.stderr)
        voice = DEFAULT_VOICE["piper"]
    res = {}
    try:
        for it in req["items"]:
            res[it["key"]] = one(it, engine, voice, req.get("rate", "+0%"), cache)
    except Missing as e:
        print(f"tts.py: {e}", file=sys.stderr)
        sys.exit(3)
    json.dump(res, sys.stdout, ensure_ascii=False)


if __name__ == "__main__":
    main()
