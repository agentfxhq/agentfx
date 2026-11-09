"""Per-minute request limits (in memory; one service process) and client identification."""
from __future__ import annotations

import collections
import ipaddress
import threading
import time


class Window:
    """Sliding 60 s window counter per client."""

    def __init__(self):
        self.hits = collections.defaultdict(collections.deque)
        self.lock = threading.Lock()
        self.last_gc = time.monotonic()

    def allow(self, client: str, per_min: int) -> bool:
        now = time.monotonic()
        with self.lock:
            q = self.hits[client]
            while q and now - q[0] > 60:
                q.popleft()
            if len(q) >= per_min:
                return False
            q.append(now)
            if now - self.last_gc > 300:
                self.last_gc = now
                for k in [k for k, v in self.hits.items() if not v or now - v[-1] > 60]:
                    del self.hits[k]
            return True


def client_ip(scope_client, headers, trust_proxy: bool) -> str:
    """The client IP. Behind Caddy (AFX_TRUST_PROXY=1) the left-most X-Forwarded-For entry that
    Caddy set; Caddy is configured to overwrite (not append to) client supplied values."""
    ip = scope_client[0] if scope_client else "unknown"
    if trust_proxy:
        xff = headers.get("x-forwarded-for")
        if xff:
            ip = xff.split(",")[0].strip()
    try:
        a = ipaddress.ip_address(ip)
        if a.version == 6:
            # one bucket per /64: one host usually owns a whole /64
            return str(ipaddress.ip_network(f"{ip}/64", strict=False).network_address) + "/64"
        return str(a)
    except ValueError:
        return "unknown"
