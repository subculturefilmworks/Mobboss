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

Scanning is read-only for media and Avid databases. Scans never move, rename, copy, or delete media. Explicitly confirmed media operations are described below. Corrupt or unsupported files produce warnings and are skipped. Reported MXF totals count successfully identified files; warnings identify failures.

## Move, Copy, Collect and Undo

**Quit Media Composer on every machine using the media before executing an operation.** Mob Boss cannot reliably detect Avid's open files.

Scan first, then preview an operation. Transfer and undo commands default to readable dry-run summaries (steps, destination counts/bytes, space, warnings and conflicts); add `--json` for complete machine-readable plans/results. Every command defaults to a dry run; `--yes` is the explicit confirmation to execute. `plan` always previews a Copy plan, even with `--yes`. Use the same `--index` when scanning, planning, executing and undoing.

```sh
mobboss scan '/Volumes/Editorial' --mode deep
mobboss plan --project 'My Film' --dest '/Volumes/Shuttle'
mobboss copy --project 'My Film' --dest '/Volumes/Shuttle'       # preview only
mobboss copy --project 'My Film' --dest '/Volumes/Shuttle' --yes
mobboss move --project 'My Film'                               # preview in-place move
mobboss move --project 'My Film' --dest '/Volumes/Archive' --yes # another root/drive
mobboss collect --project 'My Film'                            # preview per-drive moves
mobboss collect --project 'My Film' --dest '/Volumes/Shuttle' --yes
mobboss undo '/path/to/collect-log-<timestamp>.csv'             # preview only
mobboss undo '/path/to/collect-log-<timestamp>.csv' --yes
```

Repeat `--file '/absolute/indexed/file.mxf'` to operate on an explicit selection instead of the entire project. The project name then names the collection folder. Ctrl-C cancels execution after the current file finishes safely.

In the desktop app, choose one project or select table rows, then use **Actions → Move / Copy / Collect**. Choose an optional destination (required for Copy), inspect the dry-run drive summaries, free space, warnings, conflicts and complete action list, then continue to the explicit **Yes, I'm sure** confirmation. Progress includes Cancel. The result lists completed/skipped files, warnings, touched media folders and logs, with **Reveal log** and **Undo**. Actions also offers Undo from an older CSV log. The SQLite inventory refreshes immediately after execution or undo.

### Layout and Avid visibility

All destinations use this layout:

```text
<root>/Media By Project/<sanitized project>/Avid MediaFiles/MXF/<original folder>/<file.mxf>
```

Move defaults to each source drive/workspace root; `--dest` overrides it. Copy uses the chosen destination root. Collect performs all per-drive Moves first, then optionally Copies the results to the chosen destination. Source roots are inferred from the parent of `Avid MediaFiles` (an Avid drive or workspace). For indexed MXFs outside an Avid MediaFiles tree, an explicit destination root is required; Mob Boss never guesses a filesystem root. Collect moves such files directly into the selected root’s project collection. Already collected paths retain their original drive root. `MEDIA_BY_PROJECT` in `crates/mobboss-core/src/plan.rs` is the single folder-name constant.

**Avid scans `Avid MediaFiles` at a drive/volume root.** After collecting into a nested project folder, the editor must point Avid at that tree through an appropriate mounted volume/workspace. On NEXIS, use the project directory as the workspace root. Mob Boss does not mount volumes or make a nested tree automatically visible to Avid. The project name is made portable for Mac/Windows, including reserved names and trailing dots/spaces. Folder collisions from different source roots get a readable drive name and stable short drive ID; folders split at 5,000 MXFs into `.2`, `.3`, etc. MXF filenames and contents are preserved except that different-content filename conflicts get `_dup1`, `_dup2`, etc.

### Safety model and logs

- Planning reads only indexed source selections, checks size/mtime against the scan, and inventories destination conflicts without reading media contents. Only same-size candidates colliding with existing destination files are hashed with BLAKE3 to identify duplicates. Free-space budgets include 5% plus 16 MiB margin. The budget conservatively reserves copy-sized space even for same-filesystem moves; the executor checks free space again before starting.
- Normal atomic rename/hard-link publication never overwrites. Equal size and checksum means skip as a duplicate, retaining the original source (also for Move). Different content gets a planned suffix and warning. A file appearing after planning causes that action to skip; it is never silently renamed during execution.
- Copy hashes the source while streaming it once into a new `.mobboss-partial` file, flushes it, and reads that destination once to verify its size/checksum before publication. Cross-filesystem Move follows that sequence and checks the source size/mtime again before removing it. Same-filesystem Move rechecks size/mtime and uses a rename without hashing media. A genuinely different device or EXDEV triggers copying. Unsupported no-replace rename on the same device instead tries hard-link/unlink; if hard links are also unsupported, it uses check-exists-then-rename. That last fallback has a race window on concurrently modified shares: its weaker overwrite guard is logged before acting and warned in the result. Other rename errors stop that action rather than copying the file. Existing partial files are retained for inspection and block that filename; the program cleans up its own partial after a handled failure.
- Every source is re-statted before acting and checked for size/mtime changes across a copy. Changed files skip with a warning; failed Collect moves prevent their dependent copy steps. Cancellation finishes the current file and leaves completed actions in the log. A force quit/power failure can leave a partial or a published complete copy; this is not a full crash-recovery system.
- CSV journals record `op,source,dest,size,checksum,status,guard`, flushed to disk before destructive steps. `intent`, `published` (verified cross-drive copy), `done`, `duplicate`, and skip records preserve what happened. Full operation journals are mirrored into each destination's `Media By Project/<Project>/collect-log-<timestamp>.csv` and local app-data. Undo previews move-back space requirements and writes a separate journal mirrored beside the original collection logs and in app-data. An empty checksum records a metadata-only move; copied files record their streaming checksum. The guard column identifies atomic rename, hard links, or the weaker check-then-rename fallback. Older six-column logs remain readable. Keep the logs: use one complete log, not several mirrored copies of the same run.
- Undo processes successful entries in reverse order. Moves return to the original path after size/mtime checks against the current index; copies are deleted only if their current checksum matches the log and the file is still indexed. Changed copies and occupied original paths are retained with a warning. Media contents and paths are restored; logs and empty collection directories remain for audit. Automatic recovery of a process killed between filesystem and SQLite updates is not implemented; rescan and inspect the journal before retrying.
- Avid `.pmr` and `.mdb` files are never moved, deleted or written. **Avid will rebuild its media databases on next launch.** Results list touched media folders. Deployments and demo scripts can protect read-only roots by setting `MOBBOSS_PROTECTED_PATHS` to a platform-separated path list (colon-separated on Unix, semicolon-separated on Windows). No developer-specific paths are embedded in the binary. Tests and demonstrations use temporary copies. Symlink paths and parent traversal are rejected.

The safety suite includes two simulated drives, checksum comparisons before/after Collect + Undo, destination races, changed sources, stale partials, cancellation, duplicates and undo refusal after edits. To additionally run a real cross-filesystem round trip on Linux:

```sh
MOBBOSS_CROSS_FS=/dev/shm cargo test -p mobboss-core --test operations cross_filesystem -- --nocapture
```

This creates disposable files on `/tmp` and the requested second filesystem, verifies that `published` precedes `done` in each cross-device move journal, undoes the moves and compares original media checksums. It removes its own temporary files. Linux is verified locally; macOS/Windows atomic publication code still needs native platform testing.

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

The dark-first UI (with a light theme) provides multiple sources, mounted volumes/shares, Quick/Auto/Deep scans with live progress and cancellation, a virtualized media table, persisted sortable/resizable/hideable columns, multi-selection, full-column search and project/track/codec/folder filters. Projects show counts and sizes and export to CSV/HTML; the current filtered or selected table exports to CSV. Reveal opens Finder/Explorer at a selected file. The Actions menu provides Move, Copy, Collect, and log-based Undo for one selected project or selected indexed rows. Clip grouping, thumbnail and playback features are described under Phase 4 below. Render cleanup remains unavailable.

Mounted network shares must already be connected in your operating system. Settings shows the index path (default: local app-data) and lets you choose a different index and header concurrency. The backend rejects indexes and reports inside source/media folders. Whole-drive scanning permits the usual app-data index on that drive, with the same media-directory guards. A different index starts a separate library; it does not copy the old database. Exported CSV quotes fields and neutralizes spreadsheet formulas. Sizes follow Finder on macOS and decimal units on Linux; Windows uses Explorer’s 1024-based KB/MB/GB labels. Hover over sizes for exact bytes; CSV and HTML exports retain exact byte counts. After scanning, each source has an expandable reconciliation of media, Avid databases, other files (up to 200 names), folders and total file bytes within its Avid trees (or the selected folder when no Avid tree is present). Folder counts exclude the selected root; totals are logical file bytes, not allocated disk space. Database details show size, modification time and PMR agreement with disk. Recognized MDB containers are informational; unreadable files, invalid headers and PMR mismatches remain warnings. Durations show `hh:mm:ss.mmm`, calculated from descriptor edit rate; audio duration uses samples.

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

### Clip previews (Phase 4)

Switch **Files | Clips | Grid** above the inventory. Clips group all files with the same material-package UID. Missing UIDs stay separate; equal names never imply a group. Expand a clip to inspect members. Clip checkboxes select every member for Move/Copy/Collect, including members hidden by a file-level filter. Click a clip or its Preview button to open playback. A clip with several video variants currently previews its first video file.

Posters are generated after scans by two background workers, with progress, Cancel and retry. They are 320-pixel JPEGs from about 10% into a clip (frame zero for stills). Decodable alpha is composited on dark grey. The scanner itself remains header-only; preview generation is separate and never writes to media folders. Timecode comes from linked material/source package components; absent timecode displays “—”.

On first play, Mob Boss makes a cached proxy at up to 960 × 540. macOS uses VideoToolbox H.264 and Windows uses Media Foundation H.264, with AAC audio in a fast-start MP4. Encoder or device failures appear in the player; there is no GPL software-encoder fallback in releases. Linux development uses VP8/Opus WebM and requires a suitable `MOBBOSS_FFMPEG`. The first audio track goes to both speakers when mono; two tracks map to left/right. Select different tracks in the player when more are present. Scrub or use **J / K / L** (reverse/pause/forward), **Space**, and arrow keys to step approximately one frame. Timecode follows start TC and the clip edit rate.

Previews live in the application's local app-data `previews/` directory. Keys include file UID, size and modification time; proxies also include audio selection and platform. Settings provides a **5 GB** default limit and **Clear preview cache**. Least recently used completed previews are removed first, while the current player is protected. Asset access is restricted to this cache, with byte ranges supported. WebKitGTK cannot stream the custom scheme directly, so Linux fetches the cached proxy into a browser Blob before playing; macOS/Windows use the asset URL directly. Linux loads the complete small proxy before playback.

HTML project reports have an **Include thumbnails** checkbox, off by default. Reports embed one existing JPEG per clip and include **Print / Save as PDF** using the browser's print dialog. Missing posters are marked unavailable. CSV remains a file-level export.

**Known codec limit:** the sample's DNxUncompressed `rl16` RGBA titles are recognized and grouped, but the checked official/BtbN FFmpeg versions have a DNxUC parser without a decoder. Their thumbnails and playback report unavailable. Real DNxHR video, PCM audio and single-frame stills are tested. Alpha composition is present in the filter graph; rendering these particular titles is not supported. See [preview validation](docs/previews-validation.md) for platform evidence and outstanding checks.

### Bundled LGPL ffmpeg

Mac/Windows builds automatically run [`scripts/fetch-ffmpeg.mjs`](scripts/fetch-ffmpeg.mjs) before building. No separate ffmpeg installation is needed by app users. The script ignores development overrides and system/Homebrew ffmpeg for packaging. It verifies pinned archive SHA-256 values from [`ffmpeg-sources.json`](scripts/ffmpeg-sources.json), checks the resulting native executable's LGPL license and required codecs, and caches results under `target/ffmpeg-cache` and the ignored sidecar directory.

Windows x64 uses a pinned LGPL BtbN/FFmpeg-Builds executable. macOS builds official FFmpeg 8.1.3 source with a minimal LGPL configuration because a trustworthy pinned universal LGPL prebuilt was not selected. Both Apple Silicon and Intel slices are built for universal releases, then joined with `lipo`. The first Mac build needs Xcode command-line tools (`clang`, `make`, `lipo`), `curl`, `tar` and network access; later builds reuse verified binaries. It does not need Homebrew ffmpeg or nasm. These commands still work from `apps/desktop`:

```sh
npm run tauri build -- --bundles app
npm run tauri -- build --target universal-apple-darwin --bundles app,dmg
```

For development on Linux only, set `MOBBOSS_FFMPEG` to an absolute executable path before launching the desktop app or running codec tests. No Linux/system executable is copied into release bundles. Without a bundled binary or this override, inventory and file actions still work and the UI says **Previews unavailable**. Native-only source build and hardware encoder checks still need macOS/Windows validation. See [third-party notices](THIRD-PARTY-NOTICES.md) for exact sources and license texts.

### Portable builds

```sh
# Windows (run in apps/desktop on Windows): portable app + adjacent ffmpeg.exe
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

Download **MobBoss-Windows-portable.zip** from the workflow artifacts and extract the whole ZIP. Keep `MobBoss.exe`, `ffmpeg.exe` and the license notices together; run `MobBoss.exe` from the extracted folder. The Windows executable runs directly without installing Mob Boss; it uses the system **Microsoft Edge WebView2 Runtime**, normally present on Windows 10/11. A machine without that runtime needs it provisioned separately. On older or locked-down Windows machines where WebView2 is missing or disabled, ask your administrator to provision a supported Microsoft Edge WebView2 Runtime before running Mob Boss. The optional MSI is configured to skip runtime installation. Settings and the index still live in app-data, rather than beside the executable or media.

Mac builds are **unsigned and unnotarized** for now. Open the downloaded `.app` by right-clicking it and choosing **Open**, then confirm **Open**. If macOS blocks it, use System Settings → Privacy & Security → Open Anyway after attempting to open it. The `.app` can run from any writable location; no installer is required. The `.dmg` is a distribution container.

[The release workflow](.github/workflows/release.yml) builds the universal Mac `.app`/`.dmg` and the portable Windows ZIP (`MobBoss.exe` + `ffmpeg.exe` + notices) on a `v*` tag push, then uploads them as workflow artifacts (kept 7 days). It does not publish a GitHub Release. The MSI can still be built locally with the command above. Tauri build flags follow the [Tauri 2 CLI reference](https://v2.tauri.app/reference/cli/).

### Progressive scan API

`Index::scan_with(root, options, cancellation_token, callback)` emits discovery, inventory, enrichment and complete phases with done/total, folder, and row updates. Quick/Auto PMR rows are provisional and appear before header reads; metadata from headers remains authoritative. PMR source IDs are not assumed to be material package IDs. Failed headers warn and are omitted from the final index. Callers reconcile the final table from `Index::files` on completion/cancellation. Events do not persist partial metadata. Cancellation is checked during discovery, planning and between bounded header reads; in-flight filesystem I/O must return before cancellation finishes. A cancelled root does not commit index changes. With multiple roots, already completed roots remain committed. One desktop scan runs at a time.

The supplied sample's cold **index** scan delivered all 344 named files in **0.053 s** and enriched them in **12.732 s** at 8 workers. Filesystem/NAS caches were not reset. See [performance notes](docs/performance-notes.md) for the full 1/4/8/16 comparison and limitations.

### Verified on the dev machine

The Linux libraries became available during implementation: WebKitGTK 2.52.6, GTK3 3.24.41, libsoup3 3.4.4 and xvfb. Full `npm run tauri -- build --debug --no-bundle` succeeds, and the native app runs under xvfb. Live checks scanned the sample into a temporary index, displayed all 344 project-named rows while enrichment was still in progress, then showed 344 verified files (21.6 GiB). Searching DNxHR SQ matched 296 files and selecting all showed 296 selected (21.25 GiB). Source files were never changed. No Linux build blocker remains. macOS/Windows builds and Finder/Explorer behavior still need native platform verification; the workflow has only been configured, not run.

Screenshots: [media table](docs/screenshots/media-table.png), [projects summary and filtered selection in light mode](docs/screenshots/projects-summary.png). Missing-file diff rows retain previously indexed metadata; PMR-only missing paths without prior metadata have unknown size/project. The diff view describes the latest completed scan in the current session.

Phase 3 verification, exact test counts, native Collect/Undo evidence and current limits are recorded in [collect validation](docs/collect-validation.md).
