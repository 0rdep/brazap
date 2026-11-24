# Rune build patch

`rune.rev` identifies the exact upstream `dev` commit in `Cargo.toml`.
`rune.patch` carries the few Brazap fixes upstream Rune does not have yet.
Everything else Brazap once patched is now upstream and is used through Rune's
own API:

- GPU memory: `App::gpu_memory_policy(GpuMemoryPolicy::Manual { .. })` with
  bounded 4–8 MiB suballocation blocks (`src/ui/mod.rs`), as the patch used to
  hard-code. They keep the GPU memory a mostly idle chat window reserves small;
  buffers and textures still grow with their content.
- The maximized main window: `App::maximized(true)`. Call windows keep their own size.
- Windows committed text for printable keys (dead keys, AltGr), Ctrl vs Command
  word and line navigation, Unicode word boundaries around pills and secure
  fields, single-line placeholder clipping, the first-frame DWM cloak, and DXGI
  adapter selection.

## What the patch still changes

- **App emoji font first** (`rune-text`: `TextSystem::new_local`,
  `load_symbol_families`). An emoji family the app registers itself becomes the
  colour emoji family, ahead of the system's. Brazap uses this to render its
  bundled Twemoji artwork inside normal text, including names, previews,
  messages and menus, with Rune's shaping, wrapping, selection and truncation.
  Emoji the bundled font lacks still fall back to the system's fonts.
- **Ctrl+Up/Down by paragraph** (`rune-edit`: `TextEdit::paragraph_edge`). On
  Windows and Linux, Ctrl+Up/Down moves (Shift: selects) to the previous/next
  paragraph start, as Windows text editors do. Upstream moves by visual line.
  macOS is unchanged.
- **Narrow no-break space ends a word** (`rune-text`: `segmenter::splits`).
  Unicode word rules join letters and digits across U+202F (common in pasted
  times such as `10:30 AM`), so Ctrl+Arrow, Ctrl+Backspace and double-click
  would treat both sides as one word. It is whitespace, so it ends one.
- **A recreated window reopens maximized as it hid** (`rune`: `Host::hide`,
  `Host::try_create`). Brazap's tray mode closes with `release_gpu_when_closed`,
  which destroys the window; upstream applies `App::maximized` only to the
  first window, so reopening from the tray would show a small restored window.
  Hiding records whether the window was maximized, and the next window made
  opens that way (still cloaked on Windows until its first frame).

These apply to normal windows and native overlays. No reference checkout or
unrecorded build-source edits are used.

## Rune's own crate patches

Rune vendors patched copies of some crates.io dependencies (wgpu-hal, naga,
winit, sctk-adwaita, parley_engine and the AccessKit adapters) through
`[patch.crates-io]` in its root `Cargo.toml`. Cargo does not inherit a
dependency's patches, so `.cargo/config.toml` repeats that table, pointing at
the vendored crates in the same prepared checkout. `scripts/prepare-rune`
fails if the crate list differs from Rune's at the pinned revision.

Rune's `accessibility` feature is enabled explicitly (Brazap disables Rune's
default features): the labels Brazap already gives its controls reach Narrator,
NVDA and other screen readers. Rune builds no accessibility tree until an
assistive client asks for one.

## Building

Run `./scripts/prepare-rune` once before using plain Cargo, or use
`./scripts/cargo` / `./scripts/build-windows`, which prepare the source automatically.
The script fetches that commit into the ignored `.build-deps/rune` checkout,
verifies and applies the patch, and records the exact resulting diff. Subsequent
runs reuse it. Unexpected local edits are preserved and cause a clear error.
`.cargo/config.toml` directs Cargo to these patched packages. Upstream reference
checkouts under `.references` remain read-only and are never build dependencies.

When updating Rune, update the manifest pins and `rune.rev` together, compare
`[patch.crates-io]` with Rune's root `Cargo.toml`, drop patch hunks upstream now
covers and review the rest against the new source. Then build and manually
check the Windows UI, image/video/voice playback, and renderer allocations. Do
not add tests unless explicitly requested, as required by `AGENTS.md`.
