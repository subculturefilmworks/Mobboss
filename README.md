# Mob Boss

Mob Boss is an open-source, cross-platform media inventory tool for Avid Media Composer. It reads MXF headers to identify clips and projects without Avid, bins, or media databases. The Rust library and CLI are joined by a Tauri 2 desktop app for macOS and Windows.

Licensed under **MIT OR Apache-2.0**, at your option.

## Build and use

Install a current stable Rust toolchain (Rust 1.88 or later) and a C compiler for bundled SQLite:

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings

cargo run --release -- scan '/path/to/workspace' --mode deep
cargo run --release -- scan '/path/to/workspace' --mode quick
cargo run --release -- projects
cargo run --release -- files --project 'BD Demo Reel' --json
cargo run --release -- files --csv > inventory.csv
cargo run --release -- diff '/path/to/workspace'
```

`--index /path/to/index.sqlite` works on every command, before or after its subcommand. Use the same path when querying a custom index. Without it, the index lives in the platform's local app-data directory (`directories::ProjectDirs`, application `MobBoss`). The index must be outside media directories and ordinarily outside the scanned tree. Whole-drive roots are an exception: the normal app-data directory is allowed, while media-directory guards still apply. Invalid destinations are rejected before creating files.

Scanning is read-only for media and Avid databases. It never moves, renames, copies, or deletes media. Corrupt or unsupported files produce warnings and are skipped. Reported MXF totals count successfully identified files; warnings identify failures.

## Scan modes

| Mode | Behavior |
| --- | --- |
| `auto` (default) | Quick policy in folders containing PMR/MDB; otherwise reads all headers. |
| `quick` | Parses PMR and inspects MDB container labels, compares listing and cached size/mtime/package IDs, and reads new, changed or unknown headers. A cold scan emits a provisional PMR + disk inventory first, then enriches every unknown header. |
| `deep` | Reads every MXF header regardless of the cache. |

Discovery recursively accepts arbitrary MXF folders and nested Avid MediaFiles/MXF, OMFI MediaFiles, NEXIS workspace and hostname.number layouts, regardless of capitalization. Directory symlinks are not followed. Phase 1 inventories MXF files, not legacy OMF essence.

Scan output includes project totals, timing, headers read, cached files, warnings, and counts for missing/unlisted/changed files. `diff` prints the detailed paths as JSON and refreshes the index through a quick scan. A valid PMR defines the comparison inventory for its folder; otherwise the prior index does. Changed means size/mtime differs from the previous indexed scan. A same-size edit that preserves mtime requires a deep scan to detect.

## Architecture

The Cargo workspace contains:

- `crates/mobboss-core`: UI-agnostic, serde-serializable media types; bounded MXF KLV/Primer Pack reader; PMR parser; Bento MDB label inspector; recursive discovery; transactional SQLite inventory and scan history.
- `apps/desktop`: React/TypeScript/Vite UI and a Tauri 2 shell; the Rust service module runs without a webview for tests.
- `crates/mobboss-cli`: the `mobboss` binary, argument parsing, terminal summaries, JSON and CSV exports.
- `tests/fixtures`: approved real header-only fixtures and a complete PMR inventory. Core integration tests use temporary indexes, never write sample media, and skip external-data validation if the sample mount is absent.

MXF reads are limited to 512 KiB per file, including buffering. Large reserved header filler is sought over. Essence values are never read. Project tags are resolved through Primer Pack property ULs rather than a hard-coded dynamic tag number. Metadata includes package IDs, clip/project names, track type, codec/raw coding UL when present, rates, duration, dimensions, bit depth and timestamps. Duration is measured in descriptor edit-rate units. Unsupported/uncertain fields remain null.

MDB object inventory is not implemented; its Bento label and TOC extent are validated, then scanning falls back to PMR, the index and MXF. Captured/imported/render classification remains unknown pending reliable provenance-property evidence. See [format notes](docs/format-notes.md), [MDB findings](docs/mdb-notes.md), and [fixture provenance](tests/fixtures/README.md).

## Phase 1 sample validation (historical)

On the supplied read-only BD Demo Reel sample (344 MXFs, network mount, Linux release build):

| Scan | Files | Headers | Cached | Time |
| --- | ---: | ---: | ---: | ---: |
| Deep CLI | 344 | 344 | 0 | 31.530 s |
| Warm quick CLI | 344 | 0 | 344 | 0.077 s |

All 344 files resolve to **BD Demo Reel**; PMR and disk inventories agree. The sample has 296 DNxHR SQ video files (1920×1080, 8-bit), 40 PCM audio files (48 kHz, 16-bit), and 8 DNxUncompressed alpha files (1920×1080, 16-bit alpha). Video rates include 24000/1001, 30000/1001 and 60000/1001. Timing varies with network/cache conditions; integration validation also measured deep 27.150 s and quick 0.148 s.

The PMR parser is a bounds-checked Rust port of DJFio's MIT-licensed [MDV snippet](https://github.com/DJFio/MDV/blob/master/snippets/pmrtool.cpp), credited in its source. Mob Boss does not use the abandoned MDVx application's code or require it at runtime.

Deep scanning is I/O-bound on the tested NAS. Header reads now use a bounded worker pool (8 by default, configurable with `scan --concurrency 1..64`). The earlier 36.005-second deep run used a debug build; the release result is 31.530 seconds. Additional NAS I/O timing breakdowns, cache-condition caveats and sample JSON rows are in [performance notes](docs/performance-notes.md).

## Desktop app (Phase 2)

The dark-first UI (with a light theme) provides multiple sources, mounted volumes/shares, Quick/Auto/Deep scans with live progress and cancellation, a virtualized media table, persisted sortable/resizable/hideable columns, multi-selection, full-column search and project/track/codec/folder filters. Projects show counts and sizes and export to CSV/HTML; the current filtered or selected table exports to CSV. Reveal opens Finder/Explorer at a selected file. Actions remain disabled: no media move/copy/delete or preview is implemented.

Mounted network shares must already be connected in your operating system. Settings shows the index path (default: local app-data) and lets you choose a different index and header concurrency. The backend rejects indexes and reports inside source/media folders. Whole-drive scanning permits the usual app-data index on that drive, with the same media-directory guards. A different index starts a separate library; it does not copy the old database. Exported CSV quotes fields and neutralizes spreadsheet formulas. Durations show `hh:mm:ss.mmm`, calculated from descriptor edit rate; audio duration uses samples.

```sh
# Node 20.19+ and current stable Rust; run from the repo root
cd apps/desktop
npm ci
npm run dev            # Browser UI only; scanning requires the desktop shell
npm run build         # Frontend alone, no Linux webview libraries needed
npm test              # Vitest: filters, table data, formatting and exports
npm run tauri -- dev  # Full native desktop app
npm run tauri -- build --debug --no-bundle
```

Rust service logic compiles without the webview by default. `cargo test` and `cargo clippy --all-targets -- -D warnings` at the repo root include it. Full-shell verification: `cargo clippy -p mobboss-desktop --features desktop --all-targets -- -D warnings`. The `desktop` feature is automatically enabled by the Tauri CLI configuration.

On Linux, full builds need WebKitGTK 4.1, GTK3, libsoup3 and their development packages, plus the usual Tauri build dependencies. They were initially absent on the dev machine; the final verification notes below record the actual result. Frontend and service tests remain independent of them.

### Portable builds

```sh
# Windows (run in apps/desktop on Windows): primary no-installer executable
# Set RUSTFLAGS=-C target-feature=+crt-static for a static C runtime (CI does this).
# With this flag, specify the target so host build tools keep their normal runtime.
npm run tauri -- build --target x86_64-pc-windows-msvc --no-bundle
# Output at repo root: target/x86_64-pc-windows-msvc/release/MobBoss.exe
npm run tauri -- build --target x86_64-pc-windows-msvc --bundles msi  # Optional installer

# macOS (on a Mac): Intel + Apple Silicon universal app and disk image
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri -- build --target universal-apple-darwin --bundles app,dmg
# target/universal-apple-darwin/release/bundle/{macos,dmg}/
```

The Windows executable runs directly without installing Mob Boss; it uses the system **Microsoft Edge WebView2 Runtime**, normally present on Windows 10/11. A machine without that runtime needs it provisioned separately. The optional MSI is configured to skip runtime installation. Settings and the index still live in app-data, rather than beside the executable or media.

Mac builds are **unsigned and unnotarized** for now. Open the downloaded `.app` by right-clicking it and choosing **Open**, then confirm **Open**. If macOS blocks it, use System Settings → Privacy & Security → Open Anyway after attempting to open it. The `.app` can run from any writable location; no installer is required. The `.dmg` is a distribution container.

[The release workflow](.github/workflows/release.yml) builds the universal Mac `.app`/`.dmg`, portable Windows `.exe`, and optional MSI on a `v*` tag push, then uploads workflow artifacts. It does not publish a release. It has not been run or pushed from this machine. Tauri build flags follow the [Tauri 2 CLI reference](https://v2.tauri.app/reference/cli/).

### Progressive scan API

`Index::scan_with(root, options, cancellation_token, callback)` emits discovery, inventory, enrichment and complete phases with done/total, folder, and row updates. Quick/Auto PMR rows are provisional and appear before header reads; metadata from headers remains authoritative. PMR source IDs are not assumed to be material package IDs. Failed headers warn and are omitted from the final index. Callers reconcile the final table from `Index::files` on completion/cancellation. Events do not persist partial metadata. Cancellation is checked during discovery, planning and between bounded header reads; in-flight filesystem I/O must return before cancellation finishes. A cancelled root does not commit index changes. With multiple roots, already completed roots remain committed. One desktop scan runs at a time.

The supplied sample's cold **index** scan delivered all 344 named files in **0.053 s** and enriched them in **12.732 s** at 8 workers. Filesystem/NAS caches were not reset. See [performance notes](docs/performance-notes.md) for the full 1/4/8/16 comparison and limitations.

### Verified on the dev machine

The Linux libraries became available during implementation: WebKitGTK 2.52.6, GTK3 3.24.41, libsoup3 3.4.4 and xvfb. Full `npm run tauri -- build --debug --no-bundle` succeeds, and the native app runs under xvfb. Live checks scanned the sample into a temporary index, displayed all 344 project-named rows while enrichment was still in progress, then showed 344 verified files (21.6 GiB). Searching DNxHR SQ matched 296 files and selecting all showed 296 selected (21.25 GiB). Source files were never changed. No Linux build blocker remains. macOS/Windows builds and Finder/Explorer behavior still need native platform verification; the workflow has only been configured, not run.

Screenshots: [dark](docs/desktop-dark.png), [light](docs/desktop-light.png), [progressive scan](docs/desktop-progress.png), [filtered selection](docs/desktop-filtered.png). Missing-file diff rows retain previously indexed metadata; PMR-only missing paths without prior metadata have unknown size/project. The diff view describes the latest completed scan in the current session.
