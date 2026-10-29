#!/usr/bin/env python3
"""agentfx picks: design pick memory (L4). Every human or VLM pick and rejection is kept with its reason,
so later pitches and critics can read what this team prefers (design/picks.jsonl, append-only).

usage:
  agentfx picks add --choice <what was picked> --reason "<why>" [--rejected a,b] [--context "<brief>"] [--by human|critic|agent] [--lens x]
  agentfx picks list [--last 20] [--grep word]
"""
import json, sys, time
from pathlib import Path

PICKS = Path(__file__).resolve().parent.parent / "design/picks.jsonl"


def main():
    a = sys.argv[1:]
    if not a or a[0] not in ("add", "list"):
        print(__doc__)
        sys.exit(2)
    cmd, a = a[0], a[1:]

    def opt(k, d=None):
        if k in a:
            i = a.index(k)
            v = a[i + 1]
            del a[i:i + 2]
            return v
        return d

    if cmd == "add":
        choice, reason = opt("--choice"), opt("--reason")
        if not choice or not reason:
            print("--choice and --reason are required (a pick without a reason teaches nothing)", file=sys.stderr)
            sys.exit(2)
        rec = {"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "choice": choice, "reason": reason,
               "rejected": [x for x in (opt("--rejected") or "").split(",") if x], "context": opt("--context", ""),
               "by": opt("--by", "human"), "lens": opt("--lens", "")}
        PICKS.parent.mkdir(parents=True, exist_ok=True)
        with open(PICKS, "a") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        print(json.dumps(rec, ensure_ascii=False))
    else:
        last, grep = int(opt("--last", "20")), opt("--grep")
        rows = [json.loads(l) for l in PICKS.read_text().splitlines() if l.strip()] if PICKS.exists() else []
        if grep:
            rows = [r for r in rows if grep.lower() in json.dumps(r, ensure_ascii=False).lower()]
        for r in rows[-last:]:
            print(json.dumps(r, ensure_ascii=False))


if __name__ == "__main__":
    main()
