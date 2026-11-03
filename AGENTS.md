# Brazap

Brazap is an alternative WhatsApp client written in Rust. It uses
[Rune](https://github.com/anomalyco/rune) for its native desktop UI and
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust) for WhatsApp integration.

It must also expose a local Model Context Protocol (MCP) server so local agents
can interact with WhatsApp, including reading and sending messages.

## Architecture direction

- Put WhatsApp session ownership and operations in a shared Rust service layer.
  The desktop UI and local MCP server use that layer and the same session state.
- Keep WhatsApp behavior out of UI event handlers and MCP transport code. Both
  interfaces should expose consistent message data, outcomes, and errors.
- Keep the MCP server local by default, using stdio or a loopback-only listener
  as appropriate for the chosen transport.

## Validation

- Do not write unit tests, integration tests, or end-to-end tests unless the user
  explicitly asks for them. This also applies to future implementation work.
- Use formatting checks, compilation/build checks, and manual verification for
  changes unless tests are explicitly requested.
- Prefer `brazap --demo --data-dir <empty-directory>` for UI checks and screenshot
  fixtures. Demo mode uses a separate SQLite profile and must never initialize a
  WhatsApp client, modify a real profile, or send real messages. Its data is also
  available through MCP for future explicitly requested E2E tests.

## Patched Rune builds

- Use `./scripts/cargo` or `./scripts/build-windows`; they prepare the small,
  reproducible Rune GPU-memory patch described in `patches/README.md`.
- The user authorized carrying this patch for Brazap builds. It is applied to
  the ignored `.build-deps/rune` build checkout, never `.references/rune`.
- Keep `patches/rune.rev` synchronized with the manifest's upstream Rune pins.
  Preserve unexpected local changes in build sources. Maintain the tracked patch
  instead of making unrecorded dependency edits.

## SQLite persistence

- Store all durable WhatsApp data in SQLite: authentication and device state,
  cryptographic sessions, chats, contacts, groups, message history, receipts,
  attachment metadata, and retained media, file, and sticker bytes. Store binary
  content as BLOBs; external files must not be the sole persistent copy.
- Reuse whatsapp-rust's SQLite backend for its authentication and protocol
  state. Keep application-owned tables and migrations separate from the
  library-managed schema, even when they share a database file.
- Build a persistent application archive for received, sent, and history-synced
  messages. The library's protocol, retry, and pending-message stores do not
  replace the chat history needed by Brazap and its MCP server.
- Use the upstream durability hooks for incoming messages and history sync where
  supported by the pinned revision. Commit to SQLite before acknowledging
  successful persistence, and make replay idempotent using account-scoped message
  identities.
- Respect WhatsApp chat locks: exclude locked chats from browsing, search, media
  downloads, and automatic older-history requests. Keep already archived bytes
  without exposing them while the chat is locked.
- Automatically prefetch each eligible chat once and persist that coverage.
  Fetch additional pages when the user scrolls history, not on every restart.
- Persist completed history boundaries and exhausted chats across restarts and
  phone/LID aliases. Do not repeat successful requests for the same boundary.
- Notify only for fresh incoming messages, not offline/history/retry replays.
  Suppress notifications for the active foreground chat and coalesce bursts.
- The desktop UI and MCP server must read and write through the same service and
  SQLite-backed data layer so their view of chats and messages stays consistent.

## Startup and authentication

- On startup, open SQLite and attempt to restore the existing WhatsApp session.
  A database file's existence alone does not establish authentication.
- If no valid session is available, show a pairing screen before the chat layout.
  Offer QR-code pairing and an alternative phone-number linking flow.
- QR pairing must render the current payload emitted by whatsapp-rust as a
  scannable QR code, with a high-contrast background and quiet zone. Show linking
  instructions and replace expired codes when the library issues new ones.
- Phone-number linking accepts a number with country code, requests a pairing
  code, and shows instructions to enter that code in WhatsApp's Linked Devices
  flow on the phone. A phone number alone does not authenticate the client.
- Show QR/code loading, expiry, and actionable retry/error states. Do not show
  routine connection labels or green status dots on the pairing screen or sidebar.
  Respect pairing cooldowns and keep QR rotation separate from phone code refreshes.
- Enter the chat layout after the library confirms authentication. Restore the
  saved session on subsequent launches without requiring another pairing.
- Distinguish temporary disconnections from logout or an invalid session. Retry
  connections without discarding stored authentication or chat history; return
  to pairing when authentication is actually required.
- Use the same session state for the UI and MCP server. Operations that require
  a connection must report when pairing or reconnection is needed.

## UI direction

- Build the app in dark mode, consistently across the main window, menus,
  dialogs, loading states, and empty states.
- Use layered glass surfaces: translucent dark panels, backdrop blur, subtle
  borders, soft highlights, and shadows that establish depth. Preserve readable
  text and clear controls over every background.
- Aim for a beautiful, polished native interface with deliberate typography,
  spacing, alignment, rounded corners, and clear visual hierarchy.
- Make transitions and motion part of the interaction design: smooth navigation,
  conversation changes, message arrivals, panel transitions, and hover/press
  feedback. Use a consistent set of motion tokens and springs.
- Keep typing and scrolling responsive. Respect reduced-motion preferences and
  let animations settle so an idle window does not keep redrawing.
- Verify Rune's glass and animation APIs in the upstream source. Where native
  glass materials are platform-specific, provide a visually consistent rendered
  fallback.

## Authenticated chat layout

- Use a two-pane desktop layout: a chat list on the left and the selected
  conversation filling the remaining center/right area.
- Let the chat list scroll independently. Selecting a chat highlights its row
  and opens that conversation in the main pane.
- Keep the selected chat's name and chat action buttons in a header at the top
  of the conversation pane.
- Put the scrollable message history between the header and composer. Keep
  both the header and composer visible while messages scroll.
- Anchor the composer at the bottom of the conversation pane. Include a
  multiline text input, a send button, and controls for stickers, media
  (images and videos), and file attachments.
- Show an intentional empty state when no chat is selected. Enable conversation
  actions and sending only when there is an appropriate selected chat.
- Apply the dark glass styling and motion requirements to this initial screen.
  Route text and attachment sends through the shared WhatsApp service.

## Localization

- Keep user-facing UI copy in `locales/en.json`, `locales/pt-BR.json`, and
  `locales/es.json` with matching keys. Use the UI's `locale::tr` helper for
  labels and `locale::format` for named placeholders.
- Translate buttons, tooltips, empty states, pairing instructions, settings,
  and dates. Keep user messages, contact names, filenames, paths, and protocol
  identifiers unchanged. Do not concatenate translated sentence fragments.
- Preserve placeholder names across languages. Apply language changes without
  requiring a restart; see `locales/README.md`.

## Required source consultation

Before answering questions, planning changes, implementing features, or debugging:

| Area | Read and follow | Upstream source |
| --- | --- | --- |
| UI, layout, components, styling, input, windows, rendering, or animation | [rune-ui](.agents/skills/rune-ui/SKILL.md) | `.references/rune` |
| WhatsApp, pairing, sessions, messages, contacts, groups, media, connection lifecycle, or MCP tools that access WhatsApp | [whatsapp-rust](.agents/skills/whatsapp-rust/SKILL.md) | `.references/whatsapp-rust` |

Use both skills when work crosses the UI and WhatsApp boundary. Consulting source
is required for questions as well as code changes; do not rely on remembered APIs.

Also inspect existing Brazap code for local architecture, naming, and house style
before introducing new patterns. Back API claims and implementation decisions with
specific upstream source files, symbols, tests, or nearby project examples.

## Reference freshness

- Keep upstream clones in this project's ignored `.references/` directory. Clone
  missing references there, never inside a skill directory.
- Refresh each relevant reference at the start of every new task, following the
  skill's workflow. Use the upstream's current default branch and fast-forward-only
  updates. Reuse that refreshed checkout during the task.
- Treat the clones as read-only reference material. Preserve local changes or
  divergent history; never reset, clean, or overwrite them to force an update.
- If cloning or refreshing fails, say so and identify the cached revision used.
  Do not describe unverified cached source as the latest upstream source.
- When dependency manifests and lockfiles exist, check the version Brazap actually
  uses before implementing an API. Distinguish latest upstream behavior from the
  pinned version; refreshing references does not update application dependencies.
- Reference clones must not become application path dependencies or committed
  vendor copies. Declare dependencies in the application's manifests.
