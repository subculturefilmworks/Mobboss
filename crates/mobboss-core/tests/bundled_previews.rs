//! Release gate: use production argument builders and a tiny synthetic OP-Atom fixture.
use anyhow::{ensure, Context, Result};
use mobboss_core::{
    previews::{Ffmpeg, PreviewCache},
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
    // Native streaming, stereo decode and seek gates use the same fixture in streaming.rs.
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
