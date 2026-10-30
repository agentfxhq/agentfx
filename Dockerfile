# AgentFX multi-stage Dockerfile
# Builds release binaries and packages a minimal runtime with ffmpeg.
#
# Build:   docker build -t agentfx .
# Render:  docker run --rm -v "$PWD":/work agentfx render x.reel -o x.mp4 --no-tts
# Lint:    docker run --rm -v "$PWD":/work agentfx lint x.reel --measure --all-formats --no-tts
# Frame:   docker run --rm -v "$PWD":/work agentfx frame x.reel --at 2s -o frame.png
#
# GPU (Vulkan) is not available inside the container; all rendering uses the CPU
# software path. See docs/BUILD.md for details.

# ---- build stage ----
FROM rust:1.85-bookworm AS builder

WORKDIR /src
COPY . .

RUN cargo build --release --locked \
    && strip target/release/agentfx target/release/agentfx-render

# ---- runtime stage ----
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ffmpeg \
       ca-certificates \
       fontconfig \
       python3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/agentfx     /usr/local/bin/agentfx
COPY --from=builder /src/target/release/agentfx-render /usr/local/bin/agentfx-render
COPY --from=builder /src/assets/  /usr/local/share/agentfx/assets/
COPY --from=builder /src/design/  /usr/local/share/agentfx/design/
COPY --from=builder /src/tools/   /usr/local/share/agentfx/tools/
COPY --from=builder /src/mcp/     /usr/local/share/agentfx/mcp/
COPY --from=builder /src/docs/    /usr/local/share/agentfx/docs/

ENV AGENTFX_HOME=/usr/local/share/agentfx
ENV AGENTFX_GPU=0

WORKDIR /work

# MCP registry: the image is the package for io.github.agentfxhq/agentfx (see server.json).
# MCP server: docker run -i --rm -v "$PWD:/work" --entrypoint python3 <image> /usr/local/share/agentfx/mcp/server.py
LABEL io.modelcontextprotocol.server.name="io.github.agentfxhq/agentfx"
LABEL org.opencontainers.image.source="https://github.com/agentfxhq/agentfx"

ENTRYPOINT ["agentfx"]
