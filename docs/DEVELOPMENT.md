# Development

Brazap is a Rust desktop application built with Rune and whatsapp-rust. The UI and
stdio MCP adapter share one service and one SQLite archive.

## Requirements

- Rust 1.99.1 or newer, Git and Bash.
- Windows: Visual Studio C++ build tools and Windows SDK, including `rc.exe`.
  Run the build from Git Bash with the developer environment loaded.
- Linux: C and C++ compilers, libclang, `pkg-config`, ALSA, Wayland, xkbcommon and fontconfig headers.
  On Ubuntu: `sudo apt install build-essential cmake libclang-dev pkg-config libasound2-dev libwayland-dev libxkbcommon-dev libfontconfig1-dev`.
- macOS: Xcode Command Line Tools.

CMake is required for the bundled Opus voice encoder on every platform. Microphone
capture uses the system's default input device; recordings are encoded while
capturing, and the original Ogg Opus bytes are retained in SQLite after sending.
Demo mode substitutes a synthetic microphone signal and sends only to its local
demo archive.

A desktop session and a working GPU driver are needed to run the UI. Linux uses
Vulkan and desktop portals for native file dialogs.

```sh
./scripts/cargo run
./scripts/cargo build --locked --release
```

The wrapper prepares `.build-deps/rune`, applying the tracked GPU-memory patch to
the exact upstream revision in `patches/rune.rev`. Do not edit this generated
checkout or use `.references` as build dependencies. See [the patch guide](../patches/README.md).

Appearance changes apply without restarting. Rune receives the bundled Inter and
emoji fonts plus the installed faces offered in Settings; system fonts are read
locally and are not redistributed. Unavailable optional families are hidden.
Settings edits update the UI immediately and save in order through the shared
service, with a revision guard against stale background snapshots.

## Windows builds from WSL

Install the MinGW-w64 C/C++ toolchain and binutils, including `windres` and `dlltool`:

```sh
rustup target add x86_64-pc-windows-gnu
./scripts/build-windows --release --features memory-profiling
```

Output: `target/x86_64-pc-windows-gnu/release/brazap.exe`. Set
`BRAZAP_WINDOWS_CC` and `BRAZAP_WINDOWS_CXX` when the cross compilers are outside `PATH`.

## Local profiles and commands

```sh
brazap --data-dir /path/to/profile
brazap --headless
brazap --mcp
brazap --diagnostics
brazap --version
brazap --demo --data-dir /path/to/empty/demo-profile
```

`BRAZAP_DATA_DIR` also selects the profile. Use the same profile for the UI,
headless service and MCP client.

MCP is off by default. Enable it in **Settings → MCP** (saved as `mcp.enabled` in
the profile's `app_settings`) or with `BRAZAP_MCP=1` for the process that owns the
service, for example `BRAZAP_MCP=1 brazap --demo --data-dir /path/to/demo --headless`.
`BRAZAP_MCP=0` forces it off. The service checks the setting on every request: while
disabled it refuses MCP protocol methods and every request forwarded by
`brazap --mcp`, and `brazap --mcp` exits before serving stdio. Internal `brazap/*`
requests from the desktop window keep using the same loopback channel. Each profile has one service owner and one
active desktop window; a second launch activates the existing window.

Keep database files, logs, cached media, session keys and upstream checkouts out
of Git. `.local`, `.references`, `.build-deps`, `target` and `dist` are ignored.

## Architecture

- `src/service.rs`: WhatsApp lifecycle, shared operations and durable ingestion.
- `src/storage.rs`, `migrations/`, `queries/`: application archive and SQLite views.
- `src/ipc.rs`, `src/mcp.rs`: authenticated local service coordination and the opt-in stdio MCP server.
- `src/ui/`: Rune layout, interactions, playback and settings.
- `src/updates.rs`: release discovery, checksum validation and replacement.
- `locales/`: English, Brazilian Portuguese and Spanish UI strings.

The library manages authentication, encryption/session state and its protocol
schema. Application-owned tables hold chat history, identities, attachments and
preferences. Original media lives in SQLite BLOBs; playback may use temporary files.

## Validation

```sh
./scripts/cargo fmt --all -- --check
./scripts/cargo check --locked
./scripts/cargo build --locked --release
```

Use manual verification for pairing, playback, scrolling, notifications and
updates. No unit, integration or end-to-end tests should be added unless the user
explicitly requests them. Read [AGENTS.md](../AGENTS.md) and consult the relevant
[source skills](../.agents/skills/) before changing Rune or WhatsApp behavior.

## Memory inspection

Open **Settings → Diagnostics → Open memory diagnostics**, or use
`brazap --diagnostics` while the service runs. For an isolated backend sample,
close the app and run `brazap --headless-memory`.

Optimized builds need `--features memory-profiling` for Rust allocation counters.
Image and SDK estimates overlap the counted heap; do not add them to process RAM.
GPU reservation, actual allocation and process working set are different metrics.

## Documentation screenshots

The README captures the actual Windows UI using fictional fixture conversations
and the offline `--demo` SQLite profile. Screenshots do not contain a real WhatsApp
session, personal messages, contact photos or a usable pairing QR code.

## Voice recording

The composer microphone opens a vertical recorder above its button. Pause stops
microphone capture, Play previews the local recording, and Resume appends audio
without the paused interval. Sending is an explicit action in the recorder.

Audio is encoded to Ogg Opus incrementally. A bounded temporary PCM file supports
lossless previews without keeping the entire recording in memory; both it and
preview files are removed when the recording is sent, discarded, or closed.
Demo mode uses a synthetic signal, including pause and preview, without opening
the microphone or connecting to WhatsApp.

## Clipboard attachments

Paste a copied image, screenshot, or Explorer/Finder file selection into the chat
composer with Ctrl+V / Cmd+V. Pasting opens a local draft; sending is explicit.
The same preview handles files chosen through the attachment menu. Review up to
20 files, remove unwanted items, and add a separate caption to each photo, video,
or document. The caption field supports Unicode and the bundled emoji picker.
WhatsApp audio messages have no caption field, so audio-file previews say so.

Image preparation runs outside the UI thread with bounded decode memory and
small thumbnails. Pasted image bytes use temporary files, removed when the draft
is discarded or sent. Confirmed messages and original bytes are archived through
the shared service in SQLite. Batch sends stop on failure and retain only unsent
items for review; successful items are removed from the draft.
