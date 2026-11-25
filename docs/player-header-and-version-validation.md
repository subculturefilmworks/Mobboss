# Player header and build identity

Branch `fix/player-header-and-version` starts from `origin/main` at `02561d7`.
Only synthetic names are used in new tests.

The player title and its metadata now shrink within the header. The full clip
name is available on hover, and the × button stays at the top right. The modal
allows vertical scrolling only. Existing Escape dismissal is preserved.

Settings shows a muted build identity at its bottom; the app-name tooltip shows
the same text. `mobboss --version` prints that format without a program-name
prefix. Vite injects frontend metadata at build time; the CLI uses Cargo's build
script. Release CI shares one UTC timestamp and uses the pushed tag for both.
The platform bundle version uses the numeric tag core; see README for the policy.

Validation on Linux:

- `cargo test --workspace`: 129 passed, one intentionally ignored process helper.
- `cargo test -p mobboss-desktop --features desktop`: 12 passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `npm test --prefix apps/desktop`: 37 passed, including release formatting and
  missing-tag/missing-Git development fallbacks.
- `npm run build --prefix apps/desktop`: passed.
- `npm run test:ui --prefix apps/desktop`: existing sources suite and new player
  browser test passed using the installed Chromium 1248 executable via
  `PLAYWRIGHT_CHROMIUM_EXECUTABLE`. The player test mounts the real component
  with a 200-character name, long project/track metadata and transport/timecode
  at both 1280 and 1024 pixels. It checks bounds, actual click dismissal, no
  horizontal overflow, full-name tooltip, Escape dismissal and Settings identity.
- Both documented cross-target checks passed:
  `CARGO_FEATURE_PURE=1 LIBSQLITE3_SYS_USE_PKG_CONFIG=1 PKG_CONFIG_ALLOW_CROSS=1 cargo check --target aarch64-apple-darwin -p mobboss-desktop`
  and the same command with `--target x86_64-pc-windows-msvc`.
- `cargo run -q -p mobboss-cli -- --version`: emitted the expected development
  identity with UTC time and current short commit.

Cross-target checks type-check services; they do not link or run native Mac or
Windows applications. No release/package build was started, and nothing was
pushed, merged, deployed or tagged.
