#!/usr/bin/env python3
"""Print a readable trace of one eval case's agent session (tool calls, short results, text).

usage: python3 evals/trace.py <workdir>/_transcript.jsonl [--full] [--max 400]
"""
import json
import sys


def main():
    a = sys.argv[1:]
    if not a:
        sys.exit(__doc__)
    full = "--full" in a
    mx = 400
    if "--max" in a:
        mx = int(a[a.index("--max") + 1])
    if full:
        mx = 100000
    n = 0
    for line in open(a[0], errors="replace"):
        try:
            e = json.loads(line)
        except Exception:
            continue
        t = e.get("type")
        if t == "assistant":
            for b in e["message"].get("content", []):
                if b.get("type") == "tool_use":
                    n += 1
                    print(f"\n[{n}] TOOL {b['name']}: {json.dumps(b.get('input'), ensure_ascii=False)[:mx]}")
                elif b.get("type") == "text" and b.get("text", "").strip():
                    print(f"\nTEXT: {b['text'][:mx]}")
        elif t == "user":
            c = e.get("message", {}).get("content")
            if isinstance(c, list):
                for b in c:
                    if b.get("type") == "tool_result":
                        r = b.get("content")
                        r = r if isinstance(r, str) else " ".join(x.get("text", "[image]" if x.get("type") == "image" else "") for x in (r or []) if isinstance(x, dict))
                        print(f"  -> {'ERROR ' if b.get('is_error') else ''}{r[:mx]!r}")
        elif t == "result":
            print(f"\nRESULT turns={e.get('num_turns')} cost={e.get('total_cost_usd')} ms={e.get('duration_ms')} subtype={e.get('subtype')}")
            print((e.get("result") or "")[:mx])


if __name__ == "__main__":
    main()
