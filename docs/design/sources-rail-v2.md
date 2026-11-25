The rail screenshot uses only synthetic volume, folder, and project names.

Run the UI server with `npm run dev --prefix apps/desktop`, then run
`npm run test:ui --prefix apps/desktop`. Install Chromium first with
`cd apps/desktop && npx playwright install chromium`, or point
`PLAYWRIGHT_CHROMIUM_EXECUTABLE` at an existing Chromium executable.
`UI_URL` can override the default http://127.0.0.1:1420.

The browser test uses a mocked desktop bridge. It verifies no detection during
startup, 5.5 seconds idle, or focus; explicit Search pruning and unplug/replug tick memory;
Add folder without scanning; Scan disabled with no checked sources; and the exact
checked paths submitted for scanning. It also checks the All projects
all/none/mixed states and label reset, Mounted Sources without remove buttons,
Custom Folders with remove buttons, and single-line rows plus table fit at
1280 and 1024 px. The rail stays 320 px wide at both sizes. Rust separately tests
the command's checked root selection, serialized tick memory without mounted rows,
and recursive Custom Folder scanning in Quick/Auto/Deep modes. On Unix the
recursive test also includes directory and file symlinks and verifies neither
is followed. The test regenerates sources-rail-v2.png from dark and light
screenshots of the actual rail.

Cross-target Rust checking on this Linux machine:
```sh
CARGO_FEATURE_PURE=1 LIBSQLITE3_SYS_USE_PKG_CONFIG=1 PKG_CONFIG_ALLOW_CROSS=1 \
  cargo check --target aarch64-apple-darwin -p mobboss-desktop
```
This bypasses BLAKE3 C compilation and uses host SQLite metadata for type checking.
It does not link a Mac application or verify native dialogs, permissions, signing,
or runtime behavior. Signing configuration and Info.plist remain unchanged.

The same C-dependency bypass also passed with
`--target x86_64-pc-windows-msvc`. Linux desktop-feature tests and clippy
compile the native shell. Cross checks cover the services/discovery library,
without the Tauri desktop feature; native Mac and Windows bundles remain unverified.

Selection-scoped results: ticks form a union of online roots with one record per
normalized file path. Unticking preserves the index; reticking needs no scan.
Projects and codec/track options derive from that union. Project ticks and table
filters then narrow the table, operations, CSV and HTML exports. Unknown project
sorts last. The All projects checkbox supports all/none/mixed; the label resets
to all projects. Nothing selected shows only the quiet folder-selection hint.
Quick/Auto/Deep is persisted and lives under Scan, outside Settings.

The regression browser suite also verifies tick/untick/retick without scanning,
Unknown project ordering, project/search-scoped summary export, empty project
selection, the rail scan control, and the no-folder hint. Unit tests cover path
boundaries, Windows aliases, nested roots, offline exclusion and scoped exports.
