# Mob Boss

Mob Boss is an open-source, cross-platform media inventory tool for Avid Media Composer. It reads MXF headers to identify clips and projects without Avid, bins, or media databases. Phase 1 provides a Rust library and CLI; a Tauri GUI is planned for a later phase.

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

`--index /path/to/index.sqlite` works on every command, before or after its subcommand. Use the same path when querying a custom index. Without it, the index lives in the platform's local app-data directory (`directories::ProjectDirs`, application `MobBoss`). The index must be outside the scanned tree and outside media directories; invalid destinations are rejected before creating files.

Scanning is read-only for media and Avid databases. It never moves, renames, copies, or deletes media. Corrupt or unsupported files produce warnings and are skipped. Reported MXF totals count successfully identified files; warnings identify failures.

## Scan modes

| Mode | Behavior |
| --- | --- |
| `auto` (default) | Quick policy in folders containing PMR/MDB; otherwise reads all headers. |
| `quick` | Parses PMR and inspects MDB container labels, compares listing and cached size/mtime/package IDs, and reads new, changed or unknown headers. A first scan still reads all unknown headers. |
| `deep` | Reads every MXF header regardless of the cache. |

Discovery recursively accepts arbitrary MXF folders and nested Avid MediaFiles/MXF, OMFI MediaFiles, NEXIS workspace and hostname.number layouts, regardless of capitalization. Directory symlinks are not followed. Phase 1 inventories MXF files, not legacy OMF essence.

Scan output includes project totals, timing, headers read, cached files, warnings, and counts for missing/unlisted/changed files. `diff` prints the detailed paths as JSON and refreshes the index through a quick scan. A valid PMR defines the inventory for its folder; otherwise the prior index does. Changed means size/mtime differs from the previous indexed scan. A same-size edit that preserves mtime requires a deep scan to detect.

## Architecture

The Cargo workspace contains:

- `crates/mobboss-core`: UI-agnostic, serde-serializable media types; bounded MXF KLV/Primer Pack reader; PMR parser; Bento MDB label inspector; recursive discovery; transactional SQLite inventory and scan history.
- `crates/mobboss-cli`: the `mobboss` binary, argument parsing, terminal summaries, JSON and CSV exports.
- `tests/fixtures`: approved real header-only fixtures and a complete PMR inventory. Core integration tests use temporary indexes, never write sample media, and skip external-data validation if the sample mount is absent.

MXF reads are limited to 512 KiB per file, including buffering. Large reserved header filler is sought over. Essence values are never read. Project tags are resolved through Primer Pack property ULs rather than a hard-coded dynamic tag number. Metadata includes package IDs, clip/project names, track type, codec/raw coding UL when present, rates, duration, dimensions, bit depth and timestamps. Duration is measured in descriptor edit-rate units. Unsupported/uncertain fields remain null.

MDB object inventory is not implemented; its Bento label and TOC extent are validated, then scanning falls back to PMR, the index and MXF. Captured/imported/render classification remains unknown pending reliable provenance-property evidence. See [format notes](docs/format-notes.md), [MDB findings](docs/mdb-notes.md), and [fixture provenance](tests/fixtures/README.md).

## Sample validation

On the supplied read-only BD Demo Reel sample (344 MXFs, network mount, Linux release build):

| Scan | Files | Headers | Cached | Time |
| --- | ---: | ---: | ---: | ---: |
| Deep CLI | 344 | 344 | 0 | 31.530 s |
| Warm quick CLI | 344 | 0 | 344 | 0.077 s |

All 344 files resolve to **BD Demo Reel**; PMR and disk inventories agree. The sample has 296 DNxHR SQ video files (1920×1080, 8-bit), 40 PCM audio files (48 kHz, 16-bit), and 8 DNxUncompressed alpha files (1920×1080, 16-bit alpha). Video rates include 24000/1001, 30000/1001 and 60000/1001. Timing varies with network/cache conditions; integration validation also measured deep 27.150 s and quick 0.148 s.

The PMR parser is a bounds-checked Rust port of DJFio's MIT-licensed [MDV snippet](https://github.com/DJFio/MDV/blob/master/snippets/pmrtool.cpp), credited in its source. Mob Boss does not use the abandoned MDVx application's code or require it at runtime.

Deep scanning is I/O-bound on the tested NAS: network file opens and header reads dominate the measured work. Bounded parallel reads are an option for a later phase and are not implemented yet. The earlier 36.005-second deep run used a debug build; the release result is 31.530 seconds. Additional NAS I/O timing breakdowns, cache-condition caveats and sample JSON rows are in [performance notes](docs/performance-notes.md).
