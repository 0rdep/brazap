# Releases and updates

Release binaries and update metadata live in the public
[0rdep/brazap repository](https://github.com/0rdep/brazap/releases).

## Publish a release

1. Update `[package].version` in `Cargo.toml` and refresh `Cargo.lock` with
   `./scripts/cargo check`, then verify the locked build.
2. Write user-facing release notes in `docs/releases/v<version>.md`.
3. Complete formatting, compilation and manual checks. Commit and push to `main`.
4. Tag that commit with the exact package version:

   ```sh
   git tag -a v0.1.0 -m "Brazap v0.1.0"
   git push origin v0.1.0
   ```

The [Release workflow](../.github/workflows/release.yml) builds Windows x64,
Linux x64, macOS ARM64 and macOS x64. Publishing waits for all four builds and
uploads their executables, license notices and `checksums.sha256` together.
A tag/package mismatch fails before building. Pre-release tags produce GitHub
pre-releases and do not replace the latest stable release.

Pushing dependency, patch, script or workflow changes to `main` runs the same
builds without publishing, which saves the Rust dependency cache. A tag can only
restore caches saved by `main` (or by that same tag), so release builds reuse the
cache from the previous `main` build instead of compiling every dependency again.

Running the workflow manually builds downloadable workflow artifacts without
publishing a release. The workflow does not run automated tests. Binaries are
currently unsigned; checksums establish download integrity against the release,
not an independent publisher signature.

## Asset contract

Keep these names aligned with `src/updates.rs`:

| Target | Release asset |
| --- | --- |
| `x86_64-pc-windows-msvc` | `brazap-windows-x86_64.exe` |
| `x86_64-unknown-linux-gnu` | `brazap-linux-x86_64` |
| `aarch64-apple-darwin` | `brazap-macos-aarch64` |
| `x86_64-apple-darwin` | `brazap-macos-x86_64` |

`checksums.sha256` uses the standard `sha256sum` format, with one entry for each
binary. Do not rename or replace a published version's binaries; publish a new
version instead.

## Update behavior

The app reads GitHub's latest stable release, compares semantic versions and
selects its platform's asset. It accepts download links only from this repository's
GitHub release path. The downloaded binary must match its SHA-256 entry before
it is marked ready. Downloads are staged in the profile's cache directory.

Updates check and download in the background shortly after startup and every
five minutes. Choose **Restart and update** above Settings in the sidebar to
install immediately. The helper waits for the app to exit, replaces the
executable and restarts with the same data directory. If a verified download is
already staged, the next fresh desktop launch applies it before opening the
window. Opening an already running/tray app does not restart it.
On Windows, the previous executable is retained as `*.exe.previous` for recovery
if replacement or launch fails. Running from a writable directory is required.

## Manually verify an update

Use a disposable profile and a copied executable, without account credentials:

1. Start an older build with `--demo --data-dir` pointing to the disposable profile.
   For builds with the background updater, use `BRAZAP_DEMO_UPDATE=check` to enable
   real release checks without creating a WhatsApp client.
2. Wait for the update prompt above Settings. Older releases with the legacy
   update page require checking through Settings → Updates instead.
3. Confirm the automatically downloaded binary matches the release checksum.
4. Choose Restart and update (Restart and install on legacy releases). Confirm the old process exits, the new version
   starts, and the selected data directory is unchanged.
5. Confirm About reports the new installed version and the update prompt is gone.

Never use a live account or modify a published release just to simulate an update.
