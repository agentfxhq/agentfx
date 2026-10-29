# Security policy

Please report security issues privately by email to **office@levels.dev**, not through
public issues. Include what you found, how to reproduce it, and the version or commit
(`agentfx --version`).

We aim to acknowledge a report within a few working days and will tell you when a fix is
released. AgentFX is early software; only the latest release and the `main` branch get
security fixes.

Areas worth a close look: parsing of `.reel` scripts and IR JSON, file paths in scripts
(assets, fonts, includes), the HTML/CSS frontend, and the helper scripts in `tools/` and
`mcp/`, which run external programs.
