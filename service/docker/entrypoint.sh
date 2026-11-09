#!/bin/sh
# AgentFX service entrypoint (runs as root in the container, with the capabilities from
# docker-compose.yml). 1. block all network traffic of the sandbox uid range, 2. drop
# NET_ADMIN/NET_RAW from the bounding set, 3. exec uvicorn. The service itself switches each
# renderer process to its slot uid (AFX_SANDBOX_UID_BASE + slot).
set -eu

BASE="${AFX_SANDBOX_UID_BASE:-20000}"
RANGE="$BASE-$((BASE + 255))"
DATA="${AFX_DATA_DIR:-/var/lib/agentfx}"
mkdir -p "$DATA"
chmod 711 "$DATA"   # traversable for the sandbox uids (job dirs), not listable
umask 077

if [ "$(id -u)" = 0 ]; then
  ok=1
  for ipt in iptables ip6tables; do
    if command -v "$ipt" >/dev/null 2>&1; then
      "$ipt" -w -C OUTPUT -m owner --uid-owner "$RANGE" -j REJECT 2>/dev/null \
        || "$ipt" -w -A OUTPUT -m owner --uid-owner "$RANGE" -j REJECT 2>/dev/null \
        || ok=0
    else
      ok=0
    fi
  done
  if [ "$ok" = 1 ]; then
    echo "agentfx-service: network blocked for sandbox uids $RANGE"
  elif [ "${AFX_REQUIRE_NETWORK_ISOLATION:-1}" = 1 ]; then
    echo "agentfx-service: cannot install the iptables rule for uids $RANGE (needs cap NET_ADMIN);" \
         "set AFX_REQUIRE_NETWORK_ISOLATION=0 to run without it" >&2
    exit 1
  else
    echo "agentfx-service: WARNING renderers have network access (AFX_REQUIRE_NETWORK_ISOLATION=0)" >&2
  fi
  set -- setpriv --bounding-set=-net_admin,-net_raw --inh-caps=-all --
else
  set --
fi

exec "$@" /opt/agentfx-service/venv/bin/uvicorn agentfx_service.app:app \
  --host 0.0.0.0 --port "${AFX_PORT:-8780}" --no-server-header --no-proxy-headers \
  --timeout-keep-alive 30 --limit-concurrency "${AFX_HTTP_CONCURRENCY:-256}"
