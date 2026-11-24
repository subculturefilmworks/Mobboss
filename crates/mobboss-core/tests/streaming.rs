//! Real pipe/splitter/seek gate. Synthetic fixture only; no generated media on disk.
use anyhow::{ensure, Context, Result};
use mobboss_core::{
    clips, fmp4,
    previews::{Ffmpeg, PreviewCache},
    streaming::{Encoder, ReadResult, StartStream, Streaming},
    MediaFile,
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn input(path: &Path) -> Result<MediaFile> {
    let stat = fs::metadata(path)?;
    let mut metadata = mobboss_core::mxf::read(path)?;
    metadata.material_package_uid = Some("synthetic-stream".into());
    Ok(MediaFile {
        path: path.into(),
        folder: path.parent().context("fixture parent")?.into(),
        size: stat.len(),
        modified_ns: stat
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string(),
        metadata,
    })
}
struct ShortReads<R>(R);
impl<R: std::io::Read> std::io::Read for ShortReads<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let size = buffer.len().min(3);
        self.0.read(&mut buffer[..size])
    }
}
fn decode(ffmpeg: &Ffmpeg, args: &[&str], encoded: Vec<u8>) -> Result<std::process::Output> {
    let mut child = Command::new(&ffmpeg.binary)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("decode stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&encoded));
    let decoded = child.wait_with_output()?;
    match writer.join().unwrap() {
        Ok(()) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::BrokenPipe && decoded.status.success() => {}
        Err(error) => return Err(error.into()),
    }
    Ok(decoded)
}
#[test]
fn fixture_pipe_seek_and_stereo() -> Result<()> {
    let Some(ffmpeg) = Ffmpeg::locate(&[]) else {
        ensure!(
            std::env::var_os("MOBBOSS_REQUIRE_CODEC_SMOKE").is_none(),
            "native stream gate requires ffmpeg"
        );
        eprintln!("skipped: set MOBBOSS_FFMPEG for streaming codec gate");
        return Ok(());
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/previews");
    let clip = clips::group(vec![
        input(&root.join("video.mxf"))?,
        input(&root.join("audio.mxf"))?,
        input(&root.join("audio-right.mxf"))?,
    ])
    .remove(0);
    let temp = tempfile::tempdir()?;
    let cache = PreviewCache::open(temp.path())?;
    let profiles = if cfg!(windows) {
        vec![Some(Encoder::Vp9), None]
    } else {
        vec![None]
    };
    for (i, profile) in profiles.into_iter().enumerate() {
        for start in [0.0, 0.5] {
            let engine = Streaming::default();
            let began = Instant::now();
            let session = engine.start(
                ffmpeg.clone(),
                cache.clone(),
                StartStream {
                    id: format!("fixture-stream-{i}-{start}").replace('.', "-"),
                    clip: clip.clone(),
                    tracks: vec![0, 1],
                    start,
                    encoder: profile,
                },
            )?;
            ensure!(!session.reused, "native gate must encode a fresh pipe");
            let (init, encoder) = match session.read(None, Duration::from_secs(8)) {
                ReadResult::Init(init, encoder) => (init, encoder),
                ReadResult::Failed(e) => anyhow::bail!("{e}"),
                _ => anyhow::bail!("no first stream fragment within 8 seconds"),
            };
            let first = match session.read(Some(0), Duration::from_secs(1)) {
                ReadResult::Media(c) => c,
                _ => anyhow::bail!("missing first fragment"),
            };
            let elapsed = began.elapsed();
            ensure!(
                (first.fragment.start - start).abs() < 0.06,
                "tfdt {} must reflect seek {start}",
                first.fragment.start
            );
            ensure!(init.tracks.iter().any(|t| t.video), "missing video");
            ensure!(
                init.tracks.iter().any(|t| t.audio && t.channels == 2),
                "expected stereo audio"
            );
            if cfg!(target_os = "macos") {
                ensure!(
                    init.mime.contains("avc1") && init.mime.contains("mp4a"),
                    "Mac must use H.264/AAC"
                );
            }
            if profile == Some(Encoder::Vp9) {
                ensure!(
                    init.mime.contains("vp09") && init.mime.contains("opus"),
                    "Windows must prove VP9/Opus fMP4"
                );
            }
            let mut encoded = init.bytes.clone();
            encoded.extend_from_slice(&first.fragment.bytes);
            // Re-read real pipe bytes with short reads and validate splitter reconstruction.
            let mut splitter = fmp4::Splitter::new(ShortReads(std::io::Cursor::new(&encoded)));
            ensure!(
                matches!(splitter.next_segment()?, Some(fmp4::Segment::Init(_))),
                "missing split init"
            );
            ensure!(
                matches!(splitter.next_segment()?, Some(fmp4::Segment::Media(_))),
                "missing split fragment"
            );
            ensure!(splitter.next_segment()?.is_none(), "unexpected split tail");
            let decoded = decode(
                &ffmpeg,
                &[
                    "-map",
                    "0:a:0",
                    "-c:a",
                    "pcm_s16le",
                    "-f",
                    "s16le",
                    "pipe:1",
                ],
                encoded.clone(),
            )?;
            let picture = decode(
                &ffmpeg,
                &[
                    "-map",
                    "0:v:0",
                    "-frames:v",
                    "1",
                    "-c:v",
                    "mjpeg",
                    "-update",
                    "1",
                    "-f",
                    "image2",
                    "pipe:1",
                ],
                encoded,
            )?;
            ensure!(
                picture.status.success()
                    && picture.stdout.starts_with(&[0xff, 0xd8])
                    && picture.stdout.ends_with(&[0xff, 0xd9]),
                "stream video decode failed: {}",
                String::from_utf8_lossy(&picture.stderr)
            );
            ensure!(
                decoded.status.success(),
                "stereo decode failed: {}",
                String::from_utf8_lossy(&decoded.stderr)
            );
            ensure!(
                decoded.stdout.len() > 1000 && decoded.stdout.len() % 4 == 0,
                "missing stereo PCM"
            );
            let differences = decoded
                .stdout
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|b| b[..2] != b[2..])
                .count();
            ensure!(
                differences > decoded.stdout.len() / 16,
                "left/right tones should differ"
            );
            let status = session.tick(start)?;
            eprintln!("{encoder}: seek {start:.3}s, first fragment {:.3}s, tfdt {:.6}, {:.2}x realtime, {} bytes, {}",
                elapsed.as_secs_f64(), first.fragment.start, (first.fragment.end-first.fragment.start)/elapsed.as_secs_f64(), first.fragment.bytes.len(), init.mime);
            ensure!(status.output_bytes > 0, "missing measured stream bytes");
            engine.stop(Some(&session.id));
            ensure!(
                matches!(session.read(Some(0), Duration::ZERO), ReadResult::Gone),
                "stopped session remains accessible"
            );
        }
    }
    ensure!(
        fs::read_dir(temp.path())?.all(|e| e.is_ok_and(|e| e.file_name() == "windows-encoder.txt")),
        "stream must never write proxy files"
    );
    Ok(())
}

#[test]
fn copied_long_input_backpressure_and_seek() -> Result<()> {
    let (Some(source), Some(ffmpeg)) = (
        std::env::var_os("MOBBOSS_STREAM_MEDIA"),
        Ffmpeg::locate(&[]),
    ) else {
        eprintln!("skipped: set MOBBOSS_STREAM_MEDIA and MOBBOSS_FFMPEG for long-input streaming measurement");
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let media = temp.path().join("Sample Drive/MXF/Edit.1");
    fs::create_dir_all(&media)?;
    let path = media.join("Video.mxf");
    fs::copy(source, &path)?;
    let clip = clips::group(vec![input(&path)?]).remove(0);
    let total = mobboss_core::streaming::duration(&clip);
    let cache = PreviewCache::open(&temp.path().join("thumbnails"))?;
    let engine = Streaming::default();
    let began = Instant::now();
    let session = engine.start(
        ffmpeg.clone(),
        cache.clone(),
        StartStream {
            id: "long-input-start".into(),
            clip: clip.clone(),
            tracks: vec![],
            start: 0.0,
            encoder: Some(Encoder::Vp9),
        },
    )?;
    ensure!(
        matches!(
            session.read(None, Duration::from_secs(8)),
            ReadResult::Init(_, _)
        ),
        "missing initial fragment"
    );
    let first = match session.read(Some(0), Duration::ZERO) {
        ReadResult::Media(c) => c,
        _ => anyhow::bail!("missing first data"),
    };
    let elapsed = began.elapsed();
    eprintln!(
        "Copied {:.3}s / {}-byte input: first fMP4 fragment {:.3}s, initial {:.2}x realtime",
        total,
        clip.size,
        elapsed.as_secs_f64(),
        (first.fragment.end - first.fragment.start) / elapsed.as_secs_f64()
    );
    ensure!(
        first.fragment.end - first.fragment.start > elapsed.as_secs_f64(),
        "540p encoder must exceed realtime"
    );
    if total >= 120.0 {
        let deadline = Instant::now() + Duration::from_secs(15);
        while session.tick(0.0)?.buffered_end < 60.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        let full = session.tick(0.0)?;
        ensure!(
            (60.0..62.0).contains(&full.buffered_end) && full.backpressure,
            "60-second backpressure did not engage"
        );
        std::thread::sleep(Duration::from_millis(300));
        #[cfg(target_os = "linux")]
        let before = session
            .process_id()
            .map(|pid| fs::read_to_string(format!("/proc/{pid}/io")))
            .transpose()?;
        std::thread::sleep(Duration::from_millis(500));
        ensure!(
            session.tick(0.0)?.buffered_end == full.buffered_end,
            "paused player kept draining fragments"
        );
        #[cfg(target_os = "linux")]
        if let (Some(before), Some(pid)) = (before, session.process_id()) {
            let after = fs::read_to_string(format!("/proc/{pid}/io"))?;
            let rchar = |text: &str| {
                text.lines()
                    .find(|l| l.starts_with("rchar:"))
                    .map(str::to_owned)
            };
            ensure!(
                rchar(&before) == rchar(&after),
                "ffmpeg continued source reads while backpressured"
            );
            eprintln!(
                "Backpressure: buffered {:.3}s; process source-read counter unchanged while paused",
                full.buffered_end
            );
        }
    }
    let target = total / 2.0;
    let began = Instant::now();
    let seek = engine.start(
        ffmpeg,
        cache,
        StartStream {
            id: "long-input-seek".into(),
            clip,
            tracks: vec![],
            start: target,
            encoder: Some(Encoder::Vp9),
        },
    )?;
    ensure!(
        matches!(session.read(None, Duration::ZERO), ReadResult::Gone),
        "seek did not stop the old session"
    );
    ensure!(
        matches!(
            seek.read(None, Duration::from_secs(8)),
            ReadResult::Init(_, _)
        ),
        "no seek fragment"
    );
    let fragment = match seek.read(Some(seek.first_sequence), Duration::ZERO) {
        ReadResult::Media(c) => c,
        _ => anyhow::bail!("missing seek fragment"),
    };
    ensure!(
        (fragment.fragment.start - target).abs() < 0.06,
        "seek timestamps are not absolute"
    );
    eprintln!(
        "Copied input seek {:.3}s: first fragment {:.3}s, tfdt {:.6}",
        target,
        began.elapsed().as_secs_f64(),
        fragment.fragment.start
    );
    engine.shutdown();
    ensure!(
        seek.process_id().is_none(),
        "window shutdown left a process"
    );
    ensure!(
        fs::read_dir(temp.path().join("thumbnails"))?
            .next()
            .is_none(),
        "streaming wrote a proxy"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_hardware_child_exits_before_real_software_retry() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Some(ffmpeg) = Ffmpeg::locate(&[]) else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let wrapper = temp.path().join("encoder-probe");
    let quoted = ffmpeg.binary.to_string_lossy().replace('\'', "'\\''");
    fs::write(&wrapper, format!("#!/bin/sh\ncase \"$*\" in *h264_mf*) echo 'h264_mf: could not find any MFT for the given media type' >&2; exit 1;; esac\nexec '{quoted}' \"$@\"\n"))?;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/previews");
    let clip = clips::group(vec![
        input(&root.join("video.mxf"))?,
        input(&root.join("audio.mxf"))?,
    ])
    .remove(0);
    let engine = Streaming::default();
    let session = engine.start(
        Ffmpeg { binary: wrapper },
        PreviewCache::open(&temp.path().join("thumbnails"))?,
        StartStream {
            id: "hardware-failure-retry".into(),
            clip,
            tracks: vec![0],
            start: 0.0,
            encoder: Some(Encoder::Windows),
        },
    )?;
    match session.read(None, Duration::from_secs(8)) {
        ReadResult::Init(init, encoder) => {
            ensure!(
                init.mime.contains("vp09") && init.mime.contains("opus"),
                "wrong fallback container/codecs"
            );
            ensure!(encoder.contains("libvpx-vp9"), "wrong encoder detail");
        }
        ReadResult::Failed(e) => anyhow::bail!("{e}"),
        _ => anyhow::bail!("fallback did not produce a stream"),
    }
    engine.shutdown();
    ensure!(session.process_id().is_none(), "retry child was not reaped");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn linux_webm_mse_and_memory_only_blob() -> Result<()> {
    let Some(ffmpeg) = Ffmpeg::locate(&[]) else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/previews");
    let clip = clips::group(vec![
        input(&root.join("video.mxf"))?,
        input(&root.join("audio.mxf"))?,
    ])
    .remove(0);
    let engine = Streaming::default();
    for encoder in [Encoder::LinuxWebm, Encoder::LinuxBlob] {
        let session = engine.start(
            ffmpeg.clone(),
            PreviewCache::open(temp.path())?,
            StartStream {
                id: format!("linux-{encoder:?}"),
                clip: clip.clone(),
                tracks: vec![0],
                start: 0.0,
                encoder: Some(encoder),
            },
        )?;
        match session.read(None, Duration::from_secs(8)) {
            ReadResult::Init(init, _) if encoder == Encoder::LinuxWebm => {
                ensure!(
                    init.mime == "video/webm; codecs=\"vp9,vorbis\"",
                    "wrong Linux MSE codecs"
                );
                ensure!(
                    matches!(session.read(Some(0), Duration::ZERO), ReadResult::Media(_)),
                    "missing cluster"
                );
            }
            ReadResult::Blob(bytes, mime, _) if encoder == Encoder::LinuxBlob => {
                ensure!(
                    mime.contains("opus") && bytes.len() > 1000,
                    "invalid memory Blob"
                );
                ensure!(
                    bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
                    "missing WebM header"
                );
            }
            ReadResult::Failed(e) => anyhow::bail!("{e}"),
            _ => anyhow::bail!("wrong Linux transport"),
        }
        engine.stop(None);
    }
    ensure!(
        fs::read_dir(temp.path())?.next().is_none(),
        "Linux transport wrote a proxy"
    );
    Ok(())
}
