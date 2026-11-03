---
name: whatsapp-rust
description: Answer WhatsApp questions and implement or debug Brazap's WhatsApp integration by consulting current oxidezap/whatsapp-rust source, examples, and tests. Use for pairing, authentication, SQLite persistence, sessions, messaging, contacts, groups, media, connection lifecycle, protocol behavior, and local MCP tools that access WhatsApp.
---

# WhatsApp Rust

1. Work from the Brazap project root. If `.references/whatsapp-rust` is missing,
   clone `https://github.com/oxidezap/whatsapp-rust` there:

   ```sh
   mkdir -p .references
   git clone --depth 1 https://github.com/oxidezap/whatsapp-rust .references/whatsapp-rust
   ```

   Keep the checkout in the project, not in this skill's directory.
2. Before each new WhatsApp task, verify the checkout's origin matches that URL
   and determine the current upstream default branch with
   `git -C .references/whatsapp-rust ls-remote --symref origin HEAD`. Fetch `origin`
   and fast-forward the clean checkout to that branch using `--ff-only`; do not
   assume `main` or keep following an old default branch. Preserve local changes
   and divergent history. If an update fails, report the limitation and the cached
   commit from `git -C .references/whatsapp-rust rev-parse HEAD`.
3. Search `.references/whatsapp-rust` with `rg` for exact APIs, definitions,
   examples, tests, and naming patterns before answering or implementing
   WhatsApp-specific code. Read the matching implementation and callers, not
   just search results.
4. Inspect existing Brazap code for local house style before introducing patterns.
   Check application manifests and lockfiles when present; verify APIs and feature
   flags against the pinned whatsapp-rust version as well as current upstream
   source. Do not assume a new upstream API is already available to the app.
5. Prefer answers and implementations backed by specific source files, symbols,
   and nearby examples. Identify the relevant paths when explaining behavior.
   Distinguish behavior implemented by the library from assumptions about the
   WhatsApp service. For wire-protocol questions, consult the upstream guidance in
   `agent_docs/wa_web_reference.md` before drawing conclusions.

## Desktop and local MCP integration

Brazap exposes WhatsApp through a native desktop UI and a local MCP server for
agents, including message reads and sends. Route both interfaces through the
shared Rust WhatsApp service and session state. Verify the underlying operations
in whatsapp-rust before designing MCP tools; do not invent capabilities or
duplicate connection and session ownership in the MCP adapter.

## SQLite and authentication

- Follow the SQLite and startup requirements in the project's `AGENTS.md`.
  Reuse the library's SQLite backend for device, authentication, and protocol
  state, and add application-owned tables for the complete chat archive.
  Inspect `SqliteDatabase::shared` and `SharedSqlite` when sharing its database
  with application tables. Preserve the upstream schema and migration ownership.
- Do not treat `sent_messages` or `pending_inbound_messages` as a permanent
  conversation archive. Inspect the current durability hooks before implementing
  incoming-message and history-sync persistence; make commits idempotent and
  account-scoped. Use SQLite for retained attachment bytes as well as metadata.
- Check `on_qr_code`, `on_pair_code`, `on_pair_code_error`, `on_connected`, and
  `on_logged_out` in `src/bot.rs`. QR callbacks supply a payload to render, not
  a ready-made QR image. Phone linking uses `PairCodeOptions` and
  `Client::pair_with_code` or `BotBuilder::with_pair_code`; the resulting code
  must be entered on the phone. Verify expiry and refresh behavior in
  `src/pair_code.rs`; QR rotation must not repeatedly request new phone codes.
- Restore authentication through the persisted backend and drive the UI from
  actual session events. Do not infer a valid session from a database file or
  treat a temporary network failure as a reason to recreate device keys.

## Source entry points

Paths below are relative to `.references/whatsapp-rust`; re-locate them with
`rg --files` if upstream reorganizes the repository.

- `README.md`, `Cargo.toml`, and `examples/demo.rs`: public usage, feature flags,
  pairing, callbacks, and lifecycle examples.
- `src/lib.rs`, `src/bot.rs`, `src/bot/`, `src/client.rs`, and `src/features/`:
  public exports, client setup, event delivery, and feature APIs.
- `src/media.rs`, `src/features/stickers.rs`, and `examples/demo.rs`: media
  message construction, sticker operations, and message-sending examples.
- `tests/` and nearby source tests: behavioral contracts and edge cases.
- `wacore/src/` and `waproto/`: protocol types, events, JIDs, and message schemas.
- `storages/sqlite-storage/src/database.rs` and
  `storages/sqlite-storage/src/shared.rs`: database ownership and access for
  application-owned tables.
- `storages/sqlite-storage/src/sqlite_store.rs`,
  `storages/sqlite-storage/src/schema.rs`, and
  `storages/sqlite-storage/migrations/`: session and device persistence and
  schema coverage.
- `src/types/durability_hook.rs`, `src/types/history_sync_durability.rs`, and
  `examples/durability_hook.rs`: incoming-message commits, history recovery, and
  replay contracts. Adapt the example's persistence to SQLite.
- `src/pair_code.rs` and `examples/demo.rs`: phone-number linking, QR pairing,
  pairing-code expiry, and authentication events.
- `agent_docs/architecture.md`, `agent_docs/support.md`, and
  `agent_docs/testing.md`: architecture, supported configurations, and validation.

For work connecting WhatsApp to UI, also read and follow the
[UI skill](../rune-ui/SKILL.md).
