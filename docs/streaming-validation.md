# Streaming preview validation

This replaces the disk-proxy playback described in the historical
[Phase 4 notes](previews-validation.md). Mac and Windows are the shipping
platforms; Linux is used for development and CI.

## Behavior and ownership

One ffmpeg child writes a pipe per active playback session. Every input gets an
input-side seek. The output uses roughly one-second GOPs and fragmented MP4 with
absolute timestamps. The bounded box splitter extracts initialization and
moof/mdat fragments, including track codecs, timescales and decode timestamps.
The scoped `preview:` protocol accepts only the main webview's origin, active
session ID and fragment number. It cannot open arbitrary filesystem paths.

The frontend appends fragments through MediaSource, or ManagedMediaSource where
needed. Seeking within buffered data uses the existing buffer. Seeking outside
it aborts pending fetches/appends, stops and reaps the child, and opens a new
session at the requested time. Generation checks fence late async responses.
J/K/L, Space, scrub and approximate frame steps share this timeline and start TC.

The backend retains the current clip and three recent clips, at most 240 MB of
encoded data. The frontend has a 40 MB budget, with room reserved for one bounded
16 MiB transfer/read chunk: about 300 MB total encoded data, excluding native
decoder/encoder working memory. Older clips are evicted first. Completed retained
ranges can be reused. Data more than 30 seconds behind the playhead is released.
At 60 seconds ahead the reader waits; the pipe fills and ffmpeg stops reading
the source. Clip close, seek replacement and window close kill/wait the child.

No stream output is written to disk. Startup removes recognized legacy MP4/WebM
proxy files and preserves JPEGs. The old 5 GB setting is ignored on load and
omitted on save. Settings shows thumbnail usage and Clear thumbnails.

Reading/Encoding progress uses ffmpeg's actual output time/frame counters and
output bytes, with an ETA to the first fragment. Displayed input bytes are the
selected files' total size, not a per-read NAS counter. Reading and encoding
overlap; no claim is made of a measured percentage of source bytes read.

## Codecs and platform gates

- macOS: VideoToolbox H.264 / AAC in fragmented MP4, strictly required by the
  native release gate. The minimal LGPL build includes MOV demuxing, H.264/AAC
  decoders/parsers and PCM output for independent smoke decoding.
- Windows: Media Foundation H.264 / AAC first. Only an encoder-open failure
  triggers one VP9/Opus fragmented MP4 retry. The successful choice is remembered
  per ffmpeg binary. The native gate always exercises VP9/Opus, then logs the
  Media Foundation result or real fallback. Pinned LGPL BtbN has libvpx/libopus.
- Linux: the same session, memory and backpressure code can swap in VP9/Vorbis
  WebM clusters when WebKitGTK rejects fragmented MP4 MSE. Its separate bounded
  EBML parser is about 150 lines. If neither MSE format works, a Linux-only
  complete in-memory VP9/Opus Blob is capped at 40 MB. This compatibility path
  does not change either shipping platform's transport.

Both Mac and Windows release jobs run production builders against the committed
synthetic one-second DNxHR plus two PCM MXF tracks. Each must produce a JPEG,
produce a blue waveform JPEG for an audio-only clip, return a first fragment within **8 seconds**, seek to 50% with video `tfdt` within
60 ms of the requested point, and decode both a video frame and stereo PCM from
the actual fragments. Forced fallback and automatic selection use fresh sessions
so a retained result cannot conceal an encoder failure. Missing ffmpeg fails the
release gate. These new native gates are configured but have **not run** on this
branch from the Linux host. WKWebView/ManagedMediaSource and WebView2 UI playback
still need native review; the prior rc7 disk-proxy gates passed both platforms.

## Local evidence

- `cargo test --workspace`: **105 passed**, one ignored child-process helper
  that is launched and killed by the process cleanup test. Codec tests skip
  cleanly without an explicit ffmpeg override; the separate real-codec run passed.
- Workspace all-target/all-feature clippy with `-D warnings`: clean.
- Vitest: **20 passed**, including seek decisions and late-open/close fencing.
- Native Tauri Linux debug build: passed. No Linux ffmpeg is bundled.
- Real-codec fixture gate passed with a verified LGPL FFmpeg 8.1 executable:
  first fragment about **38–61 ms**, correct seek-half timestamp, video decode
  and two-channel audio decode. The real MF-failure/software-retry wrapper test
  confirmed the failed child exits before the next child starts.
- A copied **39.58 s / 581 MB** DNxHR input returned its first fMP4 fragment in
  **262 ms**, about **3.82× realtime**; seek to 19.79 s returned the first fragment
  in **231 ms**, with `tfdt=19.790583`. These are warm local-copy measurements,
  not cold-NAS or Mac/Windows hardware benchmarks.
- Native xvfb WebKitGTK MSE on that copy showed a first decoded frame in
  **0.42–0.45 s** and an immediate jump to an unbuffered 30 s in **0.39–0.41 s**.
  Playback and timecode advanced, native controls stayed hidden, and ArrowLeft
  stepped one frame while paused. This webview accepts VP9/Vorbis WebM MSE;
  it rejects fragmented MP4 and Opus MSE, hence the small Linux transport swap.
- A synthetic 180 s input stopped at a 60 s read-ahead boundary. The ffmpeg
  `/proc` read counter stayed unchanged while backpressured. Seeking to 90 s
  returned `tfdt=90` in **56 ms**. Tests cover memory cap eviction, four-clip
  retention, history trim, retained-range reuse and process ownership/cleanup.
  In the running native app, a 180 s preview was paused with an active,
  backpressured ffmpeg child at 63 s. Sending the normal window close event
  exited the app successfully and left no ffmpeg child.
- The copied-sample integration still exercises DNxHR/PCM streaming, a still
  poster and the accepted DNxUncompressed unsupported-title result. Inputs are
  copied before decoding; the original media tree remains read-only.
- Upgrade verification removed injected legacy MP4/WebM files while all nine
  existing JPEG hashes stayed unchanged. Clear thumbnails then reduced usage
  to zero without touching source media.
- A minimal FFmpeg 8.1.3 Linux build with the Mac component subset (native
  MPEG-4 substituted for unavailable VideoToolbox) independently decoded an
  H.264/AAC stream to JPEG and stereo PCM. This caught and fixed the configure
  muxer name `pcm_s16le` (CLI format `s16le`). It does not prove Mac SDK/runtime.

## Screenshots and sizes

Sanitized temporary Sample Drive / Sample Project copies only:
[Clips](screenshots/clips.png), [Grid](screenshots/grid.png),
[streaming player](screenshots/player.png), [audio waveform player](screenshots/audio-player.png),
[Settings](screenshots/settings.png).
Audio-only tiles and rows carry an Audio badge plus the track summary. Their
posters use the first audio file, bounded to 60 seconds, through the existing
two-worker priority queue. The waveform player overlays the current playhead;
long clips label the bounded waveform sample. Audio-only fragmented MP4 is also
smoke-tested at zero and 50%, with stereo decode and correct timestamp. The
minimal Linux build with the Mac component subset also passed the production
video-poster and waveform builders after enabling showwavespic. Native xvfb
audio-only playback reached its first decoded data in 0.17 s, advanced time and
the waveform playhead, and scrubbed to 12 s with TC 00:00:12:00 and no media error.

The title placeholder now wraps and reads “No preview · title”. The accepted
DNxUncompressed decoder limitation remains; inventory and file actions work.

The last native **rc7 Windows ZIP was 61,550,636 bytes** (61.55 MB), slightly over
the 60,000,000-byte reporting threshold and accepted for now. Its ffmpeg.exe was
134,050,304 bytes. The rc5 universal Mac app was reported as **25.1 MB**. These are
previous release measurements, not predictions for this branch. The new workflow
keeps `actions: read` and makes ZIP-size reporting `continue-on-error: true` so
metadata/reporting errors cannot fail an otherwise good build. New final bundle
sizes await the native release jobs. No push, tag, merge or deployment was done.
