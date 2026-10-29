#!/usr/bin/env python3
"""Transcribe an audio file into Whisper JSON with word timestamps, for `captions src=`.

Uses faster-whisper when it is importable (it is not a dependency of reel; run this script with
the Python of an environment that has it, e.g. `AGENTFX_WHISPER_PY=/path/to/venv/bin/python`
via tools/transcribe.sh). Output is the openai-whisper shape that `captions src=` reads:

    {"language": "en", "segments": [{"start", "end", "text", "words": [{"word", "start", "end"}]}]}

Usage: transcribe.py audio.(wav|mp3) [-o out.json] [--model small] [--lang en] [--prompt "names, jargon"]
"""
import argparse
import json
import sys


def decode(path):
    """16 kHz mono float32 via ffmpeg (avoids PyAV version mismatches inside faster-whisper)."""
    import os
    import subprocess
    import numpy as np
    ff = (os.environ.get("AGENTFX_FFMPEG") or os.environ.get("REEL_FFMPEG") or "ffmpeg")
    raw = subprocess.run([ff, "-v", "error", "-i", path, "-f", "f32le", "-ac", "1", "-ar", "16000", "-"], check=True, capture_output=True).stdout
    return np.frombuffer(raw, dtype=np.float32).copy()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("audio")
    ap.add_argument("-o", "--out", help="output JSON (default: stdout)")
    ap.add_argument("--model", default="small", help="faster-whisper model name or path (default: small)")
    ap.add_argument("--lang", default=None, help="language code, e.g. en or de (default: detect)")
    ap.add_argument("--prompt", default=None, help="initial prompt: spellings of names and jargon")
    a = ap.parse_args()
    try:
        from faster_whisper import WhisperModel
    except ImportError:
        sys.exit("faster-whisper is not importable by this Python; set AGENTFX_WHISPER_PY to an environment that has it")
    model = WhisperModel(a.model, device="cpu", compute_type="int8", cpu_threads=3)
    segs, info = model.transcribe(decode(a.audio), language=a.lang, word_timestamps=True, initial_prompt=a.prompt, vad_filter=False, beam_size=5)
    out = {"language": info.language, "segments": []}
    for s in segs:
        out["segments"].append({
            "start": round(s.start, 3),
            "end": round(s.end, 3),
            "text": s.text.strip(),
            "words": [{"word": w.word.strip(), "start": round(w.start, 3), "end": round(w.end, 3)} for w in (s.words or [])],
        })
    text = json.dumps(out, ensure_ascii=False, indent=1)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write(text + "\n")
    else:
        print(text)


if __name__ == "__main__":
    main()
