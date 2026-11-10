"""API keys (SQLite, sha256 of the key only) and daily usage counters.

Mint a key:   python3 -m agentfx_service.keys mint --name "acme" [--limits '{"duration_s": 300}']
List / revoke: python3 -m agentfx_service.keys list | revoke <id>
(AFX_DATA_DIR selects the database, default service/.data.)
"""
from __future__ import annotations

import argparse
import hashlib
import json
import secrets
import sqlite3
import sys
import threading
import time
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS keys (
  id TEXT PRIMARY KEY, hash TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
  limits TEXT NOT NULL DEFAULT '{}', created REAL NOT NULL, revoked REAL);
CREATE TABLE IF NOT EXISTS usage (
  client TEXT NOT NULL, day TEXT NOT NULL, kind TEXT NOT NULL, n REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (client, day, kind));
"""


def hash_key(key: str) -> str:
    return hashlib.sha256(key.encode()).hexdigest()


class Store:
    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(path, check_same_thread=False, isolation_level=None)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.executescript(SCHEMA)
        self.lock = threading.Lock()
        try:
            path.chmod(0o600)
        except OSError:
            pass

    # ---------------------------------------------------------- keys
    def mint(self, name: str, limits: dict | None = None) -> tuple[str, str]:
        key = "afx_" + secrets.token_urlsafe(32)
        kid = "k_" + secrets.token_hex(6)
        with self.lock:
            self.db.execute("INSERT INTO keys (id, hash, name, limits, created) VALUES (?, ?, ?, ?, ?)",
                            (kid, hash_key(key), name, json.dumps(limits or {}), time.time()))
        return kid, key

    def lookup(self, key: str) -> dict | None:
        if not key or not key.startswith("afx_") or len(key) > 200:
            return None
        with self.lock:
            row = self.db.execute("SELECT id, name, limits, revoked FROM keys WHERE hash = ?", (hash_key(key),)).fetchone()
        if not row or row[3] is not None:
            return None
        return {"id": row[0], "name": row[1], "limits": json.loads(row[2] or "{}")}

    def list(self):
        with self.lock:
            return self.db.execute("SELECT id, name, limits, created, revoked FROM keys ORDER BY created").fetchall()

    def revoke(self, kid: str) -> bool:
        with self.lock:
            cur = self.db.execute("UPDATE keys SET revoked = ? WHERE id = ? AND revoked IS NULL", (time.time(), kid))
        return cur.rowcount > 0

    # ---------------------------------------------------------- usage
    @staticmethod
    def day() -> str:
        return time.strftime("%Y-%m-%d", time.gmtime())

    def used(self, client: str, kind: str) -> float:
        with self.lock:
            row = self.db.execute("SELECT n FROM usage WHERE client = ? AND day = ? AND kind = ?",
                                  (client, self.day(), kind)).fetchone()
        return row[0] if row else 0

    def take(self, client: str, kind: str, limit: float, amount: float = 1) -> bool:
        """Atomically add `amount` unless that exceeds `limit` today."""
        with self.lock:
            day = self.day()
            row = self.db.execute("SELECT n FROM usage WHERE client = ? AND day = ? AND kind = ?",
                                  (client, day, kind)).fetchone()
            n = row[0] if row else 0
            if n + amount > limit:
                return False
            self.db.execute("INSERT INTO usage (client, day, kind, n) VALUES (?, ?, ?, ?) "
                            "ON CONFLICT (client, day, kind) DO UPDATE SET n = n + excluded.n",
                            (client, day, kind, amount))
            self.db.execute("DELETE FROM usage WHERE day < ?", (time.strftime("%Y-%m-%d", time.gmtime(time.time() - 3 * 86400)),))
            return True

    def give_back(self, client: str, kind: str, amount: float = 1):
        with self.lock:
            self.db.execute("UPDATE usage SET n = MAX(0, n - ?) WHERE client = ? AND day = ? AND kind = ?",
                            (amount, client, self.day(), kind))


def main(argv=None):
    from . import config
    ap = argparse.ArgumentParser(prog="python3 -m agentfx_service.keys")
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("mint", help="create a key (printed once)")
    m.add_argument("--name", required=True)
    m.add_argument("--limits", default="{}", help='JSON overrides of the key tier, e.g. {"duration_s": 300}')
    sub.add_parser("list")
    r = sub.add_parser("revoke")
    r.add_argument("id")
    a = ap.parse_args(argv)
    cfg = config.load()
    st = Store(cfg.db_path)
    if a.cmd == "mint":
        limits = json.loads(a.limits)
        if not isinstance(limits, dict):
            sys.exit("--limits must be a JSON object")
        kid, key = st.mint(a.name, limits)
        print(f"id:  {kid}\nkey: {key}\n(store it now; only its hash is kept)")
    elif a.cmd == "list":
        for kid, name, limits, created, revoked in st.list():
            state = "revoked" if revoked else "active"
            print(f"{kid}  {state:7}  {time.strftime('%Y-%m-%d', time.gmtime(created))}  {name}  {limits}")
    elif a.cmd == "revoke":
        print("revoked" if st.revoke(a.id) else "no such active key")


if __name__ == "__main__":
    main()
