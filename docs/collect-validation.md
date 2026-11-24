# Phase 3 validation — the dev machine Linux, 2026-11-24

Branch: `feat/collect`, based on Phase 2 merge `32adfc0`. No push, merge, deployment or branch deletion was performed.

## Automated checks

- `cargo test --offline`: **45 passed** — 22 core unit tests (including 11 planner tests and two executor fault/publication tests), 14 operation integration tests, eight scanner integration tests, one desktop service test. The optional cross-filesystem case returns early without its environment variable; it was also run separately on real distinct filesystems below.
- `cargo clippy --offline --all-targets -- -D warnings`: clean.
- `cargo clippy --offline -p mobboss-desktop --features desktop --all-targets -- -D warnings`: clean.
- `npm test` in `apps/desktop`: **6 Vitest tests passed**.
- `npm run tauri -- build --debug --no-bundle`: full native Linux build passed (including TypeScript/Vite). Output: `target/debug/MobBoss`.
- `MOBBOSS_CROSS_FS=/dev/shm cargo test --offline -p mobboss-core --test operations cross_filesystem -- --nocapture`: passed with different filesystem device IDs. Moves copied and verified first, logged `intent → published → done`, then removed sources. Undo restored the original checksum tree across the filesystem boundary.

Operation tests cover destination races/no overwrite, a deliberately corrupted staged copy (source retained; no final destination), cancellation after the current file, source edits, stale partial retention, duplicate handling, insufficient-space refusal before destination creation, symlink rejection, arbitrary indexed-folder selection, modified-copy undo refusal, occupied original paths, forged database paths in undo logs, and exact original media checksums after Collect + Undo. PMR content is preserved.

The historical external sample test now targets `Avid MediaFiles/MXF/EDIT-PC.1`, its original 344-file fixture set. The sample root has gained another 12 files from a second project, so asserting the historical count against the entire sample root was no longer valid. No sample content was changed.

## Native GUI walkthrough

Copied the 12 small still-image MXFs from sample `MXF/1` and the ten smallest audio MXFs plus PMR from `MXF/EDIT-PC.1` into **two temporary drive roots** under `/tmp/mobboss-phase3-demo`. Scanned them into `/tmp/mobboss-phase3-demo/index.sqlite`; settings and app-data were isolated under the same temporary directory. There were 22 indexed files across two projects.

Ran the real native Tauri app under xvfb with an isolated D-Bus session. Selected **AU 2019 Online**, chose Collect and an optional shuttle destination under `/tmp`, inspected the plan and explicit Quit-Avid confirmation, and executed:

- 12 same-drive Moves into per-drive project collections.
- 12 checksum-verified Copies into the shuttle collection.
- **24 completed, zero skipped**; SQLite and the visible table updated to 24 AU files (plus ten untouched BD audio files).
- All 24 destination MXFs independently matched the read-only sample's SHA-256 checksums. Both copied PMRs remained byte-identical.
- Used the GUI's Undo button, reviewed its reverse-order actions and move-back space budgets, confirmed, and reversed all 24 actions. All 22 original media paths/checksums and the original 22-row inventory were restored; shuttle MXFs were removed. Logs and empty directories intentionally remain.

Screenshots from the real native app:

- [Plan preview](screenshots/plan-preview.png)
- [Explicit confirmation](screenshots/confirm.png)
- [Collect result](screenshots/collect-result.png)

## Real CLI round trips

Using that temporary index and app-data, verified that `plan` and `copy` without `--yes` left media and log sets unchanged. Then executed and undid:

- Collect **BD Demo Reel** in place: ten actions.
- Move **AU 2019 Online** to an explicit destination root: 12 actions.
- Copy one indexed audio selection into a named selection collection: one action.

Every round trip had zero skips and restored the exact original media/PMR checksums. Undo without `--yes` returned a preview with reverse-order rows and free-space summaries.

## Decisions and remaining limits

The agreed destination layout is used for every operation; no `<Project> - <folder>` layout was introduced. `MEDIA_BY_PROJECT` is a constant. The free-space margin is 5% plus 16 MiB. Space planning deliberately budgets full transfer bytes even when a same-filesystem atomic move would need less, favoring a conservative refusal over under-budgeting. Duplicate moves retain their source and log a skip; no deduplication deletion is attempted.

Move accepts an optional explicit destination, enabling cross-drive moves; default Move and Collect use source roots. `plan` is a Copy preview; `move`, `copy`, and `collect` each preview their own operation without `--yes`. CLI Undo also requires `--yes`, preserving dry-run-first safety. Cancellation uses Ctrl-C in CLI and the Cancel button in GUI.

Linux native behavior is verified. macOS/Windows builds and native atomic-rename/File Manager behavior still require those platforms. The implementation uses Linux `renameat2(RENAME_NOREPLACE)`, macOS `renamex_np(RENAME_EXCL)`, and Windows `MoveFileW` for no-overwrite atomic publication. There is no automatic crash recovery if a process is killed between filesystem publication and SQLite/log completion; retain the journals, rescan and inspect before retrying. Avid must be closed, and editors must expose the collected tree at an appropriate volume/workspace root for Avid to discover it. No media database files are edited.
