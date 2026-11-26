# Public release packaging validation

## Tag conditions

The workflow trigger remains `push.tags: ['v*']`. The `desktop` matrix has no
release-only condition, so all matching tags build and upload the existing two
artifacts with seven-day retention. `publish` needs the entire matrix to succeed
and uses:

```yaml
if: startsWith(github.ref, 'refs/tags/v') && !contains(github.ref_name, '-rc')
permissions:
  contents: write
```

- `v0.1.0`: both conditions true; publish a Release with generated notes and
  `MobBoss-macOS.dmg` / `MobBoss-Windows.zip`.
- `v0.1.0-rc1` or `v0.1.0-rc.1`: second condition false; artifacts only.
- A branch push or non-v tag: workflow does not trigger. The explicit ref check
  also prevents publishing from a branch if a manual trigger is added later.
- Either matrix build fails: `needs: desktop` prevents publishing.

No tags were created to validate these conditions. Review the complete YAML diff
with `git show -- .github/workflows/release.yml` after the packaging commit.

## DMG layout

`scripts/package-dmg.mjs` uses the Tauri macOS DMG config: 660 × 480 window,
app at (180,170), Applications at (480,170). The opening note occupies its own
row at (330,330), leaving room below for its filename. The app is copied with
`ditto` to preserve its signature, permissions and symlinks. The script verifies
the final compressed image with `hdiutil verify`.

This Linux machine cannot run Finder/hdiutil. Visual layout and actual Gatekeeper
opening instructions still need a macOS check; Windows SmartScreen and opening
the support link also need a native packaged-app check.

## FFmpeg notice audit

Evidence inspected: `scripts/ffmpeg-sources.json`, `scripts/fetch-ffmpeg.mjs`,
`THIRD-PARTY-NOTICES.md`, `docs/licenses/*`, and the existing Windows sidecar's
embedded configuration and extracted `FFmpeg-Windows-LICENSE.txt`.

- macOS: source-built FFmpeg 8.1.3. Complete configure flags disable autodetection,
  enable no GPL/nonfree options or external codec libraries. This supports the
  notice's LGPL 2.1-or-later classification. There is no Mac binary here to run;
  native fetch/build validation checks `-version` and `-L` before release bundling.
- Windows: pinned static LGPL archive, n8.1.3-41-ga8029b6528. The existing binary
  embeds `--enable-version3`, no `--enable-gpl`/`--enable-nonfree`, and LGPL licence
  wording. Its extracted licence is LGPL 3. This supports LGPL 3-or-later,
  not a GPL FFmpeg build. Native CI additionally checks the running executable.
- Notices already give the exact Mac source archive and SHA-256, FFmpeg source
  revisions for both platforms, Windows binary archive/hash, pinned upstream
  build recipes/dependency versions, and rebuild directions. Full LGPL 2.1,
  LGPL 3 and incorporated GPL 3 texts exist. Mac resources include notices/texts;
  Windows staging copies these plus the upstream licence.

Gaps: the Windows build enables many external libraries. The current notices
provide their source/build recipe location, but do not enumerate all dependency
copyright notices or licence texts. This audit has not established that those
notices or all corresponding dependency sources are complete/retrievable. The
project also does not host a complete corresponding-source bundle for the pinned
Windows binary. Upstream URL availability and exact-source rebuildability remain
unverified; do not describe the existing links as a completed source compliance
audit. No licence classification was inferred from the system FFmpeg executable.

## Local checks

`cargo test` passed across the workspace; `npx tsc -b` passed; Vitest passed
all 37 tests in five files. YAML/JSON parsing, representative release-condition
assertions, `node --check scripts/package-dmg.mjs`, and `git diff --check` passed.

Additional desktop-feature cross-checks reached the native toolchain boundary:
macOS failed compiling Objective-C because Linux `cc` cannot accept Apple `-arch`
flags; Windows passed capability processing but failed resource generation because
`llvm-rc` is unavailable. Neither is recorded as a successful desktop build.
Native release CI remains required for those checks and the DMG layout review.
