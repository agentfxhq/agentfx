# Publishing a release

## 1. Version
Bump `version` in `Cargo.toml` (workspace) and in `server.json` (`version` and the image tag),
commit, tag `v<version>`, push the tag, and create the GitHub release.

## 2. Container image (ghcr.io)
```sh
docker build -t ghcr.io/agentfxhq/agentfx:<version> -t ghcr.io/agentfxhq/agentfx:latest .
echo "$TOKEN" | docker login ghcr.io -u <github user> --password-stdin   # token with write:packages
docker push ghcr.io/agentfxhq/agentfx:<version>
docker push ghcr.io/agentfxhq/agentfx:latest
```
Make the package public once (GitHub > agentfxhq > Packages > agentfx > Package settings).
The image carries the label `io.modelcontextprotocol.server.name`, which the MCP registry checks.

Smoke test of the MCP server in the image:
```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  | docker run -i --rm -v "$PWD:/work" --entrypoint python3 ghcr.io/agentfxhq/agentfx:<version> \
    /usr/local/share/agentfx/mcp/server.py
```

## 3. MCP registry (registry.modelcontextprotocol.io)
```sh
mcp-publisher login github      # namespace io.github.agentfxhq/*: needs agentfxhq org membership
mcp-publisher publish           # reads server.json
```
Directories such as PulseMCP and Glama pull from the registry.
