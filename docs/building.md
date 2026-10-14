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
SIGN_IDENTITY="Developer ID Application: …" scripts/package-macos.sh
NOTARIZE=1 APPLE_ID=… APPLE_TEAM_ID=… APPLE_APP_SPECIFIC_PASSWORD=… SIGN_IDENTITY=… scripts/package-macos.sh
```

The bundle (about 46 MB as a dmg) contains:
- the app;
- the `takeoff` CLI;
- `libpdfium.dylib` (in Frameworks);
- llama.cpp's `llama-server` and its dylibs: the newest official ggml-org build, or set
  `LLAMA_TAG=b11617` to pin one.

Every Mach-O is signed inside-out with the hardened runtime.

## Windows / Linux

The code is portable (egui/wgpu, pdfium, llama.cpp). `scripts/fetch-pdfium.sh` knows the Linux
binaries. Installers for these platforms are on the roadmap.
