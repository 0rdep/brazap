# Releases and updates

Release binaries and update metadata live in the public
[0rdep/brazap repository](https://github.com/0rdep/brazap/releases).

## Publish a release

1. Update `[package].version` in `Cargo.toml` and refresh `Cargo.lock` with
   `./scripts/cargo check`, then verify the locked build.
2. Write user-facing release notes in `docs/releases/v<version>.md`, starting from
   `docs/releases/unreleased.md`, and reset that file afterwards.
3. Complete formatting, compilation and manual checks. Commit and push to `main`.
4. Tag that commit with the exact package version:

   ```sh
   git tag -a v0.1.0 -m "Brazap v0.1.0"
   git push origin v0.1.0
   ```

The [Release workflow](../.github/workflows/release.yml) builds Windows x64,
Linux x64, macOS ARM64 and macOS x64, then packages each build with the scripts
below. Publishing waits for all four builds and uploads their executables,
installers, packages, update archives, license notices and `checksums.sha256`
together.
A tag/package mismatch fails before building. Pre-release tags produce GitHub
pre-releases and do not replace the latest stable release.

Pushing dependency, patch, script or workflow changes to `main` runs the same
builds without publishing, which saves the Rust dependency cache. A tag can only
restore caches saved by `main` (or by that same tag), so release builds reuse the
cache from the previous `main` build instead of compiling every dependency again.

Running the workflow manually builds downloadable workflow artifacts without
publishing a release. The workflow does not run automated tests. Checksums
establish download integrity against the release, not an independent publisher
signature.

## Packaging

| Script | Produces | Needs |
| --- | --- | --- |
| `scripts/package-windows <brazap.exe> <dir>` | Inno Setup installer from [`packaging/windows/brazap.iss`](../packaging/windows/brazap.iss) | Git Bash, Python 3, Inno Setup 6.6+ (CI installs 6.7.1 with Chocolatey) |
| `scripts/package-macos <binary> <aarch64\|x86_64> <dir>` | `Brazap.app` as a DMG and a `ditto` update archive | macOS (`iconutil`, `codesign`, `hdiutil`), Python 3 |
| `scripts/package-linux <binary> <dir>` | `.deb` and AppImage | Python 3, `objdump`, `dpkg-deb`, `curl` |

All icons are generated from `assets/logo.png` by
[`packaging/icons.py`](../packaging/icons.py), using the same square crop as the
Windows executable icon in `build.rs`. The macOS `Info.plist` template is
`assets/macos/Info.plist`; `build.rs` also embeds it in the macOS executable.
The Linux scripts download pinned `appimagetool` and AppImage runtime releases
and verify their SHA-256 digests. The `.deb` dependencies come from the binary's
linked libraries plus the graphics libraries Rune opens at startup; the script
stops if it finds a library it has no Debian package for.

The Windows installer is per user (`%LOCALAPPDATA%\Programs\Brazap`, no
administrator rights). Its Start menu shortcut is the same file, AppUserModelID
(`Brazap.Desktop`) and toast activator that the app maintains for notifications.
Upgrades close a running Brazap first. Uninstalling removes the program and the
registrations that point to it, but never the profile data directory. Keep
`AppId` in `brazap.iss` unchanged; `src/updates.rs` uses it to keep the version
shown in Installed apps current after in-app updates.

### macOS signing and notarization

Without signing secrets, the app is signed ad hoc. macOS then blocks the first
launch of a downloaded copy until the user chooses **Open Anyway** in System
Settings → Privacy & Security (or removes the quarantine attribute), and asks
for camera and microphone permission again after each update. To sign with a
Developer ID and the hardened runtime, add these repository secrets:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | Base64 of the exported Developer ID Application certificate and key (`.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | For example `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | Apple ID, team ID and an app-specific password for `notarytool` |

The first three enable signing on every build. With all six, release tags also
notarize and staple both the app and the DMG, which removes the Gatekeeper
prompt. Hardened-runtime entitlements are in
[`packaging/macos/brazap.entitlements`](../packaging/macos/brazap.entitlements)
(camera and microphone only; the app is not sandboxed). Windows builds are not
code signed, so SmartScreen may warn about the installer until it has a
reputation or a signing certificate is added.

## Asset contract

Keep these names aligned with `src/updates.rs`:

| Target | For new installs | Updates an installed copy |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `brazap-windows-x86_64-setup.exe` | `brazap-windows-x86_64.exe` (installer copy and standalone) |
| `x86_64-unknown-linux-gnu` | `brazap-linux-x86_64.deb`, `brazap-linux-x86_64.AppImage` | `brazap-linux-x86_64.AppImage` (AppImage), `brazap-linux-x86_64` (standalone) |
| `aarch64-apple-darwin` | `brazap-macos-aarch64.dmg` | `brazap-macos-aarch64.app.zip` (app), `brazap-macos-aarch64` (standalone) |
| `x86_64-apple-darwin` | `brazap-macos-x86_64.dmg` | `brazap-macos-x86_64.app.zip` (app), `brazap-macos-x86_64` (standalone) |

Releases up to v0.1.21 only know the standalone executable names, so keep
publishing them. `checksums.sha256` uses the standard `sha256sum` format, with
one entry for each `brazap-*` asset. Do not rename or replace a published
version's assets; publish a new version instead.

## Update behavior

The app reads GitHub's latest stable release, compares semantic versions and
selects its platform's asset. It accepts download links only from this repository's
GitHub release path. The downloaded binary must match its SHA-256 entry before
it is marked ready. Downloads are staged in the profile's cache directory.

Updates check and download in the background shortly after startup and every
five minutes. Choose **Restart and update** above Settings in the sidebar to
install immediately. The helper waits for the app to exit, replaces the
installation and restarts with the same data directory. If a verified download is
already staged, the next fresh desktop launch applies it before opening the
window. Opening an already running/tray app does not restart it.

The app picks its download from how it is installed:

- **Windows** (setup or standalone): replaces `brazap.exe` in place and keeps the
  previous executable as `*.exe.previous` for recovery if replacement or launch
  fails.
- **macOS app** (`*.app/Contents/MacOS/brazap`): extracts the `.app.zip` beside
  the bundle, swaps the whole bundle and reopens it with `open`. A failed swap
  restores the old bundle and discards the download.
- **AppImage** (`APPIMAGE` set by the AppImage runtime): replaces the AppImage file.
- **Standalone executable** on macOS or Linux: replaces the executable.
- **Package-managed or read-only** (`/usr/bin/brazap` from the `.deb`, or any
  directory the user cannot write, such as an app macOS runs from a translocated
  copy): nothing is downloaded. The sidebar shows **Download update**, which opens
  the release page.

## Manually verify an update

Use a disposable profile and a copied executable, without account credentials:

1. Start an older build with `--demo --data-dir` pointing to the disposable profile.
   For builds with the background updater, use `BRAZAP_DEMO_UPDATE=check` to enable
   real release checks without creating a WhatsApp client. Repeat for each
   installation type you changed (installer copy, app bundle, AppImage).
   `BRAZAP_DEMO_UPDATE=manual` previews the package-managed prompt without a check.
2. Wait for the update prompt above Settings. Older releases with the legacy
   update page require checking through Settings → Updates instead.
3. Confirm the automatically downloaded binary matches the release checksum.
4. Choose Restart and update (Restart and install on legacy releases). Confirm the old process exits, the new version
   starts, and the selected data directory is unchanged.
5. Confirm About reports the new installed version and the update prompt is gone.

Never use a live account or modify a published release just to simulate an update.
