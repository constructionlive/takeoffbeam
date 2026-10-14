# Building & releasing

```sh
scripts/fetch-pdfium.sh            # vendor/pdfium (chromium/7881, matches pdfium-render 0.9)
cargo test --workspace             # unit + parity + UI interaction tests
cargo run -p takeoff-app           # the app (debug)
```

## Tests

- **Parity:** `crates/takeoff-core/tests/parity.rs` checks badge numbering, measurement maths,
  unit keys and vision-coordinate sizing against golden outputs from construction.live's
  TypeScript. Maintainers regenerate the fixtures with `PLATFORM=… scripts/parity/gen.sh`.
- **UI interaction:** `crates/takeoff-app/tests/interaction.rs` drives the real UI with simulated
  input: count, calibrate, length, area, undo, delete, autosave.
- **UI snapshots:** run
  `SNAP_DIR=/tmp/snaps cargo test -p takeoff-app --test snapshots -- --ignored`. It renders the
  window headless (wgpu) in dark and light themes, and those images are the ones in these docs.

## macOS package

```sh
scripts/package-macos.sh                                          # ad-hoc signed, dist/*.dmg
SIGN_IDENTITY="Developer ID Application: Neuratwin Inc. (8T3B74J76Z)" scripts/package-macos.sh
NOTARIZE=1 APPLE_ID=… APPLE_TEAM_ID=… APPLE_APP_SPECIFIC_PASSWORD=… SIGN_IDENTITY=… scripts/package-macos.sh
```

The bundle (about 46 MB as a dmg) contains:
- the app;
- the `takeoff` CLI;
- `libpdfium.dylib` (in Frameworks);
- llama.cpp's `llama-server` and its dylibs: the newest official ggml-org build, or set
  `LLAMA_TAG=b11617` to pin one.

Every Mach-O is signed inside-out with the hardened runtime.

## Windows installer

```sh
brew install mingw-w64 makensis      # once (macOS); or run on Windows / in CI
scripts/package-windows.sh           # → dist/Takeoff-Desk-<version>-x64-setup.exe
```

The script cross-compiles with mingw-w64 (with the icon and version info embedded) and fetches:
- `pdfium.dll`;
- llama.cpp's **Vulkan** build, which runs on NVIDIA, AMD and Intel GPUs and falls back to CPU;
- Microsoft's Visual C++ runtime installer, which setup runs silently only if the runtime is
  missing.

The NSIS installer adds a Start-menu and desktop shortcut, an *Open with → Takeoff Desk* entry for
PDFs, and an uninstaller in Add/Remove Programs.

`.github/workflows/release.yml` builds the same installer natively (MSVC) on a Windows runner for
every `v*` tag and attaches it to the GitHub release.

The installer is not Authenticode-signed yet, so Windows SmartScreen shows "unknown publisher"
until it has a code-signing certificate.

## Linux

The code is portable (egui/wgpu, pdfium, llama.cpp) and CI builds and tests it on Ubuntu.
A packaged AppImage is on the roadmap.

## Shipping an update

Every installed copy checks the update feed at launch and every 6 hours:
`https://pub-061018a047d54079b309fb16a26a1ce0.r2.dev/takeoff-desk/latest.json` (Cloudflare R2).
If that fails, it falls back to the latest GitHub release. When the feed has a newer version, a
banner offers **Download & install**: it downloads the dmg or setup.exe for the user's platform
into Downloads and opens it. It also has *What's new*, *Later* and *Skip this version*.
*Help → Check for updates…* runs the check on demand.

1. Bump `version` in the root `Cargo.toml`.
2. Build the installers:
   - `SIGN_IDENTITY=… NOTARIZE=1 … scripts/package-macos.sh`
   - `scripts/package-windows.sh`
3. Publish, with R2 credentials in the environment:
   ```sh
   NOTES="- Auto area\n- Faster thumbnails" scripts/publish-update.sh
   ```
   Add `CRITICAL=1` for a fix users can't skip. Add `DRY_RUN=1` to preview the manifest.
4. Optionally tag `vX.Y.Z` and push. CI builds the Windows installer and attaches it to the
   GitHub release.

![Update banner](images/dark-update.png)

## Feedback

*Feedback* (top right, or *Help → Send feedback…*) opens the user's email app with a message to
rahul@construction.live. The subject and body are pre-filled with the version, OS and the model
in use.
