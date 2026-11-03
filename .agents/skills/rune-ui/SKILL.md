---
name: rune-ui
description: Answer UI questions and implement or debug Brazap's native desktop UI by consulting current Rune source, examples, and tests. Use for layout, components, styling, input, windows, rendering, animation, and UI integration.
---

# Rune UI

1. Work from the Brazap project root. If `.references/rune` is missing, clone
   `https://github.com/anomalyco/rune` there:

   ```sh
   mkdir -p .references
   git clone --depth 1 https://github.com/anomalyco/rune .references/rune
   ```

   Keep the checkout in the project, not in this skill's directory.
2. Before each new UI task, verify the checkout's origin matches that URL and
   determine the current upstream default branch with
   `git -C .references/rune ls-remote --symref origin HEAD`. Fetch `origin` and
   fast-forward the clean checkout to that branch using `--ff-only`; do not assume
   `main` or keep following an old default branch. Preserve local changes and
   divergent history. If an update fails, report the limitation and the cached
   commit from `git -C .references/rune rev-parse HEAD`.
3. Search `.references/rune` with `rg` for exact APIs, definitions, examples,
   tests, and naming patterns before answering or implementing Rune-specific code.
   Read the matching implementation and callers, not just search results.
4. Inspect existing Brazap code for local house style before introducing patterns.
   Check application manifests and lockfiles when present; verify APIs against
   the pinned Rune revision as well as current upstream source. Do not assume a
   new upstream API is already available to the app.
5. Prefer answers and implementations backed by specific source files, symbols,
   and nearby examples. Identify the relevant paths when explaining API behavior,
   and distinguish source-backed facts from proposed app design.

## Design implementation

Follow the UI direction in the project's `AGENTS.md`: dark mode, layered glass
surfaces, polished typography and spacing, and smooth transitions and motion.
Consult the glass examples below for blur, tint, borders, and material support;
check platform constraints before choosing a native material. Use Rune's motion
tokens and springs consistently, respect reduced motion, and avoid continuous
redraws once an interaction has settled.

On startup, follow the authentication flow in `AGENTS.md`: restore a saved
session or show the QR/phone-number pairing screen before the chat layout.
Read pairing and connection state from the shared WhatsApp service. Render
the current QR payload with a high-contrast background and quiet zone; keep
loading, expiry, retry, and failure states clear within the dark glass design.
Consult the WhatsApp skill before wiring these states to library events.

## Source entry points

Paths below are relative to `.references/rune`; re-locate them with `rg --files`
if upstream reorganizes the repository.

- `crates/rune/README.md` and `crates/rune/src/`: native app setup, views, state,
  layout, input, rendering, windows, and runtime behavior.
- `crates/rune-kit/src/`, `crates/rune-kit/examples/`, and
  `crates/rune-kit/tests/`: reusable components and their behavior.
- `crates/rune-kit/src/shell/` and `crates/rune-kit/src/text_input.rs`:
  sidebar and main-pane composition, toolbars, and multiline composer input.
- `examples/hello/src/main.rs`, `examples/settings/`, and `examples/kit-gallery/`:
  application structure, composition, and usage patterns.
- `examples/glass-card/`, `examples/glass-panel/`, and `examples/frosted-bar/`:
  glass materials, backdrop blur, surface layering, and animated transitions.
- `crates/rune-motion/`, especially `src/tokens.rs`: animation implementation,
  shared motion tokens, springs, reduced-motion behavior, and tests.
- `site/`: documentation source; confirm API details in Rust implementations.

For work connecting UI to WhatsApp, also read and follow the
[WhatsApp skill](../whatsapp-rust/SKILL.md).
