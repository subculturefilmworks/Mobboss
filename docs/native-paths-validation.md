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

## rc9.2 test corrections

The rc9.1 macOS core and native streaming gates passed. Its shell-wrapper retry
test incorrectly exercised the Windows libvpx profile against the minimal Mac
binary. That probe now runs on Linux only; the Windows native gate still proves
the real software fallback. The Mac build and its VideoToolbox gate are unchanged.

The Windows scanner panic occurred while constructing the legacy fixture,
before migration: `parent()` on its mixed-separator verbatim path returned
`None`. The fixture now stores its folder spelling independently. Added Windows
cases exercise uppercase verbatim paths with native and mixed separators, using
both canonical long names and actual `GetShortPathNameW` results. On volumes
without 8.3 aliases, the test logs that fact. Each case checks canonical file and
folder keys and SQLite clip membership, with and without a material UID.
Failures report stored file, clip-member and scan-root keys. No production
migration change was indicated by the original panic.

Local follow-up validation: all 116 workspace tests passed (one ignored process
helper), with required LGPL codec gates and a symlinked temporary directory;
all-target/all-feature workspace clippy was clean. The added Windows cases await
the rc9.2 native runner.
