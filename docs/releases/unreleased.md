# Unreleased

## Changes

- **MCP is now optional and off by default.** Brazap no longer offers its local MCP server to AI agents unless you turn it on. Existing installations also start with MCP off, because no earlier version had a setting for it; turn it back on once and the choice is kept.
  - In the app: **Settings → MCP → Enable MCP server**. It applies immediately, without a restart.
  - Without a window: `BRAZAP_MCP=1 brazap --headless`. `BRAZAP_MCP=0` forces MCP off for that run.
  - While MCP is off, `brazap --mcp` exits with an explanation and agents get no tools. The rest of the app works as before.
