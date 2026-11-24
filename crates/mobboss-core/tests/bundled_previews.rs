//! Release gate: use production argument builders and a tiny synthetic OP-Atom fixture.
use anyhow::{ensure, Context, Result};
use mobboss_core::{
    clips,
    previews::{Ffmpeg, PreviewCache, ProxyPlatform},
    CancellationToken, MediaFile,
};
use std::{fs, path::Path};

fn input(path: &Path) -> Result<MediaFile> {
    let stat = fs::metadata(path)?;
    let mut metadata = mobboss_core::mxf::read(path)?;
    // ffmpeg's independent OP-Atom writers assign separate material UIDs.
    // Join the synthetic picture and tones in memory; no source file is edited.
    metadata.material_package_uid = Some("synthetic-preview-smoke".into());
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

// Read MP4 sample descriptions, independent of an additional ffprobe executable.
fn sample_entries(bytes: &[u8], entries: &mut Vec<(Vec<u8>, Vec<u8>)>) -> Result<()> {
    let mut offset = 0;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into()?) as usize;
        ensure!(size >= 8 && offset + size <= bytes.len(), "invalid MP4 box");
        let kind = &bytes[offset + 4..offset + 8];
        let body = &bytes[offset + 8..offset + size];
        if [b"moov", b"trak", b"mdia", b"minf", b"stbl"].contains(&kind.try_into()?) {
            sample_entries(body, entries)?;
        } else if kind == b"stsd" {
            ensure!(body.len() >= 8, "missing sample descriptions");
            let mut rest = &body[8..];
            while rest.len() >= 8 {
                let n = u32::from_be_bytes(rest[..4].try_into()?) as usize;
                ensure!(n >= 8 && n <= rest.len(), "invalid sample entry");
                entries.push((rest[4..8].to_vec(), rest[..n].to_vec()));
                rest = &rest[n..];
            }
        }
        offset += size;
    }
    Ok(())
}

#[test]
fn bundled_previews() -> Result<()> {
    let Some(ffmpeg) = Ffmpeg::locate(&[]) else {
        ensure!(
            std::env::var_os("MOBBOSS_REQUIRE_CODEC_SMOKE").is_none(),
            "release smoke test requires MOBBOSS_FFMPEG"
        );
        eprintln!("skipped: set MOBBOSS_FFMPEG to run the synthetic codec smoke test");
        return Ok(());
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/previews");
    let video = input(&root.join("video.mxf"))?;
    let audio = input(&root.join("audio.mxf"))?;
    ensure!(
        video
            .metadata
            .codec
            .as_deref()
            .is_some_and(|c| c.starts_with("DNxHR")),
        "fixture must be DNxHR"
    );
    ensure!(
        audio.metadata.track_kind.as_deref() == Some("audio"),
        "fixture must contain PCM audio"
    );
    let temp = tempfile::tempdir()?;
    let cache = PreviewCache::open(temp.path())?;
    let cancel = CancellationToken::default();
    let poster = cache.thumbnail(&ffmpeg, &video, &cancel)?;
    let jpeg = fs::read(poster)?;
    ensure!(
        jpeg.starts_with(&[0xff, 0xd8]) && jpeg.ends_with(&[0xff, 0xd9]),
        "expected a complete JPEG"
    );
    let right = input(&root.join("audio-right.mxf"))?;
    let clip = clips::group(vec![video, audio, right]).remove(0);
    // Exercise mono duplication and two-track merging with the real native encoders.
    let platform = ProxyPlatform::current();
    let profiles = if platform == ProxyPlatform::Windows {
        eprintln!(
            "Windows gate: mandatory VP8/Opus WebM, then optional h264_mf initialization probe"
        );
        vec![ProxyPlatform::WindowsWebm, ProxyPlatform::Windows]
    } else if platform == ProxyPlatform::Mac {
        vec![platform]
    } else {
        // The Windows software profile also runs on Linux development ffmpeg.
        vec![ProxyPlatform::WindowsWebm]
    };
    for profile in profiles {
        for tracks in [&[0][..], &[0, 1][..]] {
            let mut updates = Vec::new();
            let preview =
                cache.proxy_with_fallback(&ffmpeg, &clip, tracks, profile, &cancel, |update| {
                    updates.push(update)
                })?;
            if profile != ProxyPlatform::Windows {
                ensure!(
                    updates.last().is_some_and(|p| p.finished),
                    "missing final ffmpeg progress record"
                );
                ensure!(
                    updates.iter().any(|p| p.output_bytes.is_some()),
                    "missing measured encoded bytes"
                );
            }
            if profile != ProxyPlatform::Windows {
                ensure!(
                    preview
                        .path
                        .extension()
                        .is_some_and(|e| e == profile.extension()),
                    "unexpected proxy container for {profile:?}"
                );
            }
            let encoded = fs::read(&preview.path)?;
            ensure!(encoded.len() > 1000, "proxy is empty");
            if preview.path.extension().is_some_and(|e| e == "mp4") {
                let mut entries = Vec::new();
                sample_entries(&encoded, &mut entries)?;
                ensure!(
                    entries.iter().any(|(kind, _)| kind == b"avc1"),
                    "expected H.264 MP4 video"
                );
                let (_, aac) = entries
                    .iter()
                    .find(|(kind, _)| kind == b"mp4a")
                    .context("expected AAC MP4 audio")?;
                ensure!(
                    aac.len() >= 28 && u16::from_be_bytes(aac[24..26].try_into()?) == 2,
                    "expected two-channel AAC"
                );
                let position = |name: &[u8]| encoded.windows(4).position(|b| b == name);
                ensure!(
                    position(b"moov") < position(b"mdat"),
                    "expected faststart MP4"
                );
            } else {
                ensure!(
                    encoded.windows(5).any(|b| b == b"V_VP8"),
                    "expected VP8 WebM video"
                );
                let opus = encoded
                    .windows(8)
                    .position(|b| b == b"OpusHead")
                    .context("expected Opus WebM audio")?;
                ensure!(
                    encoded.get(opus + 9) == Some(&2),
                    "expected two-channel Opus"
                );
                // Decode the complete proxy, not just its headers. Windows's full LGPL
                // sidecar includes the null muxer; the minimal Mac gate stays unchanged.
                ffmpeg.run(
                    &[
                        "-i".into(),
                        preview.path.as_os_str().into(),
                        "-map".into(),
                        "0:v:0".into(),
                        "-map".into(),
                        "0:a:0".into(),
                        "-f".into(),
                        "null".into(),
                        "-".into(),
                    ],
                    &cancel,
                    |_| {},
                )?;
            }
            eprintln!(
                "Synthetic thumbnail and {} proxy tracks {tracks:?} passed ({} bytes)",
                preview.encoder,
                encoded.len()
            );
        }
    }
    Ok(())
}

#[test]
fn synthetic_headers_have_duration_with_aligned_filler() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/previews");
    for (name, duration) in [
        ("video.mxf", 24),
        ("audio.mxf", 48000),
        ("audio-right.mxf", 48000),
    ] {
        let file = input(&root.join(name))?;
        assert_eq!(file.metadata.duration, Some(duration));
        assert_eq!(mobboss_core::previews::seconds(&file), 1.0);
    }
    Ok(())
}
