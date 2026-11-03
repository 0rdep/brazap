# Rune build patch

`rune.rev` identifies the exact upstream commit in `Cargo.toml`.
`rune-low-memory.patch` bounds wgpu's GPU allocation blocks for this desktop UI.
The default performance policy reserves large blocks even for a mostly idle
chat window. Small blocks reduce unused reservation; buffers and textures can
still grow with the content they actually need.

The same tracked patch lets an emoji font explicitly registered by the app take
priority over system emoji fonts. Brazap uses this to render its bundled Twemoji
artwork inside normal text, including names, previews, messages and menus.
This preserves Rune's shaping, wrapping, selection and truncation behavior.

The patch also adapts the Windows adapter-selection part of
[zAcherttp's Rune PR #46](https://github.com/anomalyco/rune/pull/46), reviewed at
`2f6ab83ecdadd5819cf638496fb5a0f9529540ca`. After explicit `WGPU_ADAPTER_NAME` and
`WGPU_POWER_PREF` overrides, it uses DXGI's first hardware adapter (normally the
primary display's GPU) instead of forcing the integrated GPU. It keeps the
existing fallback when no matching adapter can present the window. This is a
small backport, not the entire unmerged PR; Rune's existing DX12 configuration
already includes part of that work.

Run `./scripts/prepare-rune` once before using plain Cargo, or use
`./scripts/cargo` / `./scripts/build-windows`, which prepare the source automatically.
The script fetches that commit into the ignored `.build-deps/rune` checkout,
verifies and applies the patch, and records the exact resulting diff. Subsequent
runs reuse it. Unexpected local edits are preserved and cause a clear error.
`.cargo/config.toml` directs Cargo to these patched packages. Upstream reference
checkouts under `.references` remain read-only and are never build dependencies.

When updating Rune, update the manifest pins and `rune.rev` together, review the
patch against the new source, then build and manually check the Windows UI,
image/video/voice playback, and renderer allocations. Do not add tests unless
explicitly requested, as required by `AGENTS.md`.
