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

The tracked patch also carries desktop text-input fixes against the same pin:

- On Windows, use the keyboard layout's committed text for printable keypresses.
  US International dead-key + Space produces a literal accent even though the
  logical key is Space; invalid combinations can produce two characters. Keep
  shortcut keys separate, including Ctrl+C/V, and allow AltGr text.
- Distinguish macOS Command navigation from Windows/Linux Ctrl navigation. Ctrl
  moves/selects/deletes by word, Ctrl+Up/Down moves by paragraph, and Ctrl+Home/End
  moves to document boundaries. Plain Home/End follows the current visual line
  on Windows/Linux. macOS Command/Option behavior is preserved.
- Word boundaries also handle Unicode whitespace (including pasted non-breaking
  spaces), punctuation and emoji. Navigation and deletion preserve atomic pills
  and hidden secure fields instead of treating an entire pasted phrase as a word.
- Single-line placeholders stay on one line and clip inside their input at large
  font sizes, instead of wrapping over neighboring controls.

These fixes are applied to normal windows and native overlays. No reference
checkout or unrecorded build-source edits are used.

`WindowOptions::maximized` passes an initial maximized state to the native
window manager. Brazap uses it for its main window; call windows retain their
own size. It does not enter fullscreen.

The Windows first-frame reveal fix is backported from upstream Rune
`f73f72cdc563534e4f7de9f5a79bed007ac42384` (`platform/win32/cloak.rs` and
the window host). DWM keeps newly shown windows cloaked until a rendered frame
has reached the GPU, with a bounded fallback if no frame arrives. This prevents
the white startup flash and applies when reopening from the tray as well.
The reveal also waits for DWM to consume the pending present and temporarily
disables the native show transition, preventing an old redirection frame from
flashing during startup or tray restore. Normal native transitions are restored
after the reveal.
Windows maximization is deferred until that cloak is installed: winit's initial
maximize call uses `ShowWindow` even when the initial visibility is false, which
otherwise exposes the unpainted window for a frame on launch and tray recreation.
The helper is kept in the tracked `platform/win32.rs` file so the reproducible
build checkout does not gain untracked patch files.

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
