# AgentFX render service image: the agentfx image (repo Dockerfile) + the Python service.
#
# Build from the repository root:
#   docker build -t agentfx:local .                                   # the CLI image (optional)
#   docker build -f service/Dockerfile -t agentfx-service:dev .       # FROM ghcr.io/agentfxhq/agentfx:latest
#   docker build -f service/Dockerfile --build-arg BASE=agentfx:local -t agentfx-service:dev .
ARG BASE=ghcr.io/agentfxhq/agentfx:latest
FROM ${BASE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends python3-venv iptables \
    && rm -rf /var/lib/apt/lists/*

COPY service/requirements.txt /opt/agentfx-service/requirements.txt
RUN python3 -m venv /opt/agentfx-service/venv \
    && /opt/agentfx-service/venv/bin/pip install --no-cache-dir -r /opt/agentfx-service/requirements.txt

# the service, the (refactored) stdio MCP server it reuses, and the templates
COPY service/agentfx_service /opt/agentfx-service/agentfx_service
COPY mcp/ /usr/local/share/agentfx/mcp/
COPY docs/ /usr/local/share/agentfx/docs/
COPY templates/ /usr/local/share/agentfx/templates/
COPY service/docker/entrypoint.sh /usr/local/bin/agentfx-service-entrypoint
RUN chmod 755 /usr/local/bin/agentfx-service-entrypoint \
    && find /opt/agentfx-service /usr/local/share/agentfx -type d -exec chmod 755 {} + \
    && find /opt/agentfx-service/agentfx_service /usr/local/share/agentfx/mcp /usr/local/share/agentfx/templates -type f -exec chmod 644 {} +

ENV PYTHONPATH=/opt/agentfx-service \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    AFX_DATA_DIR=/var/lib/agentfx \
    AFX_HOME=/usr/local/share/agentfx \
    AFX_BIN=/usr/local/bin/agentfx \
    AFX_TEMPLATES_DIR=/usr/local/share/agentfx/templates \
    AFX_SANDBOX_UID_BASE=20000 \
    AFX_REQUIRE_SANDBOX=1 \
    AFX_PORT=8780

WORKDIR /var/lib/agentfx
EXPOSE 8780
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD python3 -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8780/healthz', timeout=4).status == 200 else 1)"
ENTRYPOINT ["agentfx-service-entrypoint"]
