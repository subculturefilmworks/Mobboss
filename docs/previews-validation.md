# Phase 4 preview validation

## Implemented behavior

- SQLite grouping migration stores clip membership by material UID. Triggers keep it
  consistent through rescan, transfer and deletion. Missing UIDs remain single files.
  All physical members remain available to actions, even with file filters applied.
- Header-only parsing follows linked material/source package tracks and timecode
  components for start frame, rounded FPS, actual rate and drop flag. Missing linked
  timecode remains unknown. Existing indexed headers refresh once for these fields.
- Two background poster workers run separately from scanning. JPEGs are 320 pixels
  wide, sampled around 10% of a video or at frame zero for a still. Generation can
  be cancelled and retried. No output is placed beside source media.
- Clips and Grid show lazy posters. Clip rows expand to member files. Clicking a
  clip opens a proxy player; checkboxes remain selection controls for file actions.
- HTML reports optionally embed one cached JPEG per clip, with an off-by-default
  checkbox, print stylesheet and Print / Save as PDF button. CSV is unchanged.
- On-demand proxies map mono to both speakers or two selected tracks to left/right,
  adjusting linked timecode offsets with audio trim/delay. Additional audio tracks
  can be selected in the player. Proxies include the first video variant in a clip.
  Transport includes scrub, play/pause, J/K/L, Space and approximate frame steps.
- Completed previews have a default 5,000,000,000-byte LRU limit. The current player
  is protected. Oversized proxies are refused after generation. Settings can change
  the limit or clear previews. Active generations can temporarily exceed the limit;
  abandoned partials are cleaned on cache open/prune, while live jobs stay locked.
  Empty lock files remain for safe coordination between app instances.

## Verification on Linux

The full read-only sample header scan found **356 files / 310 material clips**:
272 single-file clips, 30 pairs and eight triples. Linked timecode was available
for 189 file headers. These counts describe package identities, not equal names.
The source media tree was never written.

- `cargo test --workspace`: 71 tests, including grouping/migration, TC fixtures,
  cache keys, LRU/protected entries, orphan cleanup, retry safety, platform encoder
  arguments and mono/stereo/sync-offset construction. The real-codec test skips
  cleanly without both environment variables below.
- Workspace all-target/all-feature clippy with `-D warnings`: clean.
- Vitest: 10 checks; frontend production build: passes.
- Native Tauri debug and release Linux builds with `--no-bundle`: pass.
- Real-codec integration: temporary copies of a DNxHR video, matching PCM audio,
  DNxUncompressed title and one-frame DNxHR still. Video and still JPEGs pass;
  the title checks the expected missing-decoder error. A generated proxy contains
  video and two-channel audio, confirmed with ffprobe. Tested with system ffmpeg
  6.1.1 (development override only) and a checksum-verified LGPL BtbN 8.1 build.
- Official FFmpeg 8.1.3 source also compiled as a minimal Linux codec check with
  the same decoder/filter subset, replacing platform H.264 with native MPEG-4 for
  that check. This temporary executable is never a packaged sidecar.
- A synthetic fully transparent red RGBA input passed through the exact poster
  filter graph produced grey RGB values of 31 (8-bit conversion rounding around
  the requested value of 32). This verifies composition, not the unavailable
  DNxUncompressed title decoder.

To opt into real media tests, set `MOBBOSS_SAMPLE_DIR` to a read-only sample tree
and `MOBBOSS_FFMPEG` to an absolute development executable, then run:

```sh
cargo test -p mobboss-core --test previews -- --nocapture
```

No test writes the sample tree: inputs are copied to a disposable temporary
`Sample Drive/Avid MediaFiles/MXF/Edit.1` directory before decoding.

## Native GUI and asset protocol

The committed screenshots were captured under xvfb at 1440 × 900 from full media
copies in a sanitized temporary Sample Drive/Edit.1 tree. Project and clip display
names were made generic in the disposable index. No private paths are in the
screenshots:

- [Clips and expanded members](screenshots/clips.png)
- [Grid](screenshots/grid.png)
- [Player after seeking](screenshots/player.png)

Native WebKit loaded a **960 × 540 VP8/Opus** proxy with duration **5.304 seconds**,
played it, paused and sought to 3 seconds. Displayed TC advanced from the indexed
start. Cache clear reduced the completed-file count to zero. Native folder/save
dialogs open through Rust wrappers; those selections never grant asset access.
Print/PDF output itself still needs a native platform print-dialog check.

The running webview requested bytes 100–199 from a cached proxy and received
**206**, exactly **100 bytes**, `Content-Range: bytes 100-199/1344731`, and
`Content-Type: video/webm`. A media MXF outside the cache returned **403**.
An unsatisfiable range is rejected; Tauri returns 416, whose missing CORS header
makes WebKit expose a rejected fetch rather than its status. The linked Tauri
2.12.7 asset implementation handles bounded single and multipart ranges.

WebKitGTK's GStreamer media loader could not stream `asset://` directly despite
successful range fetches. Linux therefore fetches the scoped cached proxy into a
browser Blob before playback. Blob seeking was verified. This loads the full
proxy before playback and can use more memory for long previews. macOS and
Windows retain direct asset URLs and require native range/playback validation.

## Packaging and remaining platform checks

`scripts/fetch-ffmpeg.mjs` pins source/archive SHA-256 values, caches verified
outputs and rejects GPL/nonfree/libx264 native executables. Mac/Windows Tauri
platform configs declare `binaries/ffmpeg` as an external binary, with target
suffixes generated before the build. Linux intentionally bundles no ffmpeg.
The system executable is never consulted by the fetch/build script.

- **macOS:** official FFmpeg **8.1.3** source, minimal LGPL 2.1-or-later build,
  VideoToolbox H.264 / native AAC. Universal builds compile both architectures
  and join with lipo. Intel cross-built slices are not executed on Apple Silicon
  hosts that may lack Rosetta; the final universal executable is validated on the
  host. Source/configuration, SDK compatibility, Intel/Apple Silicon runtime,
  hardware encoder, seeking and final app/DMG sizes need a Mac build.
- **Windows x64:** pinned BtbN **n8.1.3-41-ga8029b6528**, LGPL 3-or-later static
  executable, Media Foundation H.264 / native AAC. The archive checksum and ZIP
  extraction were verified on Linux. The executable contains the required MF,
  MJPEG and DNxHD components and no enabled GPL/nonfree/libx264 flags. Execution,
  license/codec validation, MF device support, WebView2 seeking and final portable
  ZIP size need Windows. The release workflow performs native validation and
  stages `MobBoss.exe`, `ffmpeg.exe` and notices into one downloadable ZIP.

The workflow writes executable/archive byte sizes, the uploaded Windows ZIP
size from GitHub artifact metadata, and Mac app disk usage to
its job summary. It has not been triggered for this branch; no push, tag, merge
or deployment was performed. See THIRD-PARTY-NOTICES.md and the pinned JSON for
source/version/checksum links and full license texts.

### Codec deviation

The sample's DNxUncompressed RGBA titles contain the `rl16` run-length variant.
Official 8.0.1/8.1.3 and the checked BtbN 8.1 binaries expose its parser but no
DNxUC decoder. The upstream proposed decoder handles other RGB/YUV variants,
not this rl16 variant. These title previews remain unavailable, with error
messages and retry controls; grouping, inventory and actions still work. No
claim is made that alpha title rendering has passed. The real-codec test attempts
the copied title and validates the missing-decoder failure instead of claiming a
successful thumbnail. This remains a departure from the requested completion
criteria.

### Measured sizes

The pinned Windows `ffmpeg.exe` is **134,050,304 bytes** (134.05 MB / 127.84 MiB).
Final Mac app/DMG and Windows application ZIP sizes cannot be measured on this
Linux host and must be reported from the native workflow after review.

The Linux release executable is **15,948,760 bytes** (15.95 MB); the debug
executable is 230,094,496 bytes. Linux has no packaged sidecar or distributable
bundle in this phase. These Linux sizes do not predict Mac/Windows bundle sizes.
