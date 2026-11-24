# Native path fixes

The path layer resolves the longest existing ancestor through `dunce`, then
appends any uncreated tail. Windows paths expand 8.3 aliases, use native
separators and keep extended syntax internally only when required. Scanning,
index writes, selected destinations, transfer planning, journal reads and reveal
share this form. Version 2 migrates legacy file/folder keys and scan scopes,
preserving clip membership. Unsafe or disconnected rows remain available for
review rather than being silently resolved through an internal link.

UI responses and CSV paths use a separate formatter that removes the extended
prefix, including the UNC variant. Filesystem paths remain intact. Missing-UID
clip IDs and SQLite grouping triggers use the same public path spelling, so long
paths still identify the same clip in Rust and the frontend.

Root selection resolves system aliases first. A source operation uses the saved
scan root, or its Avid/collection tree root, as its boundary. Checks inspect each
unresolved component at and below that boundary before resolving the final file.
This permits system ancestors such as macOS's aliases into `/private` and an
explicitly selected volume alias, while refusing internal source, collection and
destination symlinks. Protected-path checks use the canonical path, including
destinations that do not yet exist.

Old journal paths are normalized when read. Pair deduplication, indexed-file
matching and undo use the comparison key introduced for duplicate detection;
the CSV schema, status meanings and no-overwrite behavior are unchanged.

Validation on the Linux development host:

- `cargo test --workspace --all-targets`: 116 passed, one ignored process helper
  exercised by its cleanup test. The entire suite also passed with `TMPDIR`
  behind a symlink pointing to a simulated `private/var` directory.
- The same run with verified LGPL ffmpeg and required codec smoke gates passed,
  including posters, waveforms, stereo fragmented streaming and seeks.
- All-target/all-feature workspace clippy with warnings denied: clean.
- Linux native Tauri debug build: passed.
- Regression coverage includes system ancestors, a selected volume alias through
  copy and undo, source and destination links inside trees, protected aliases,
  legacy journal matching, index migration and grouping preservation, plus
  platform-independent Windows prefix/UNC formatting.
- A Windows-only regression exercises actual paths longer than MAX_PATH,
  filesystem access, prefix-free JSON/CSV and fallback clip IDs. It is configured
  for the full native release suite; it has not run on this Linux host.

Mac and Windows confirmation awaits the native release gate. No push, tag,
merge or deployment was performed for this fix.
