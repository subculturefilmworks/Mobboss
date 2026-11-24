//! Real codec smoke test. All essence reads use temporary copies; the sample stays read-only.
use mobboss_core::{
    previews::{Ffmpeg, PreviewCache},
    CancellationToken, MediaFile,
};
use std::{fs, path::PathBuf};

#[test]
fn copied_sample_previews() -> anyhow::Result<()> {
    let Some(ffmpeg) = Ffmpeg::locate(&[]) else {
        eprintln!("skipped: set MOBBOSS_FFMPEG for codec smoke tests");
        return Ok(());
    };
    let Some(sample) = std::env::var_os("MOBBOSS_SAMPLE_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set MOBBOSS_SAMPLE_DIR");
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let mut candidates = Vec::new();
    for e in walkdir::WalkDir::new(sample)
        .into_iter()
        .filter_map(Result::ok)
    {
        if e.file_type().is_file()
            && e.path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mxf"))
        {
            if let Ok(metadata) = mobboss_core::mxf::read(e.path()) {
                candidates.push((
                    e.path().to_path_buf(),
                    metadata,
                    fs::metadata(e.path())?.len(),
                ));
            }
        }
    }
    let video = candidates
        .iter()
        .filter(|(_, m, _)| {
            m.track_kind.as_deref() == Some("video")
                && m.codec.as_deref() == Some("DNxHR SQ")
                && m.duration.is_some_and(|n| n > 1)
                && candidates.iter().any(|(_, a, _)| {
                    a.track_kind.as_deref() == Some("audio")
                        && a.material_package_uid == m.material_package_uid
                })
        })
        .min_by_key(|(_, _, size)| *size)
        .expect("sample DNxHR video");
    let title = candidates
        .iter()
        .filter(|(_, m, _)| m.codec.as_deref() == Some("DNxUncompressed"))
        .min_by_key(|(_, _, size)| *size)
        .expect("sample RGBA title");
    let still = candidates
        .iter()
        .filter(|(_, m, _)| m.track_kind.as_deref() == Some("video") && m.duration == Some(1))
        .min_by_key(|(_, _, size)| *size)
        .expect("sample still");
    let media = temp.path().join("Sample Drive/Avid MediaFiles/MXF/Edit.1");
    fs::create_dir_all(&media)?;
    let cache = PreviewCache::open(&temp.path().join("app-data/previews"))?;
    let token = CancellationToken::default();
    let mut copies = Vec::new();
    for (i, (source, metadata, _)) in [video, title, still].into_iter().enumerate() {
        let path = media.join(format!("sample-{i}.mxf"));
        fs::copy(source, &path)?;
        let stat = fs::metadata(&path)?;
        let file = MediaFile {
            path,
            folder: media.clone(),
            size: stat.len(),
            modified_ns: stat
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
                .to_string(),
            metadata: metadata.clone(),
        };
        match cache.thumbnail(&ffmpeg, &file, &token) {
            Ok(poster) => {
                let data = fs::read(poster)?;
                assert_eq!(&data[..2], &[0xff, 0xd8]);
                assert!(data.len() > 500);
            }
            Err(e) if i == 1 => {
                // Upstream FFmpeg currently has only a DNxUC parser. Sample titles
                // use rl16, which the unmerged decoder patch does not handle either.
                let message = e.to_string();
                assert!(
                    message == "No preview (Avid uncompressed title)",
                    "unexpected title placeholder: {message}"
                );
                eprintln!("DNxUncompressed title limitation verified: {message}");
            }
            Err(e) => return Err(e),
        }
        copies.push(file);
    }
    let audio = candidates
        .iter()
        .find(|(_, m, _)| {
            m.track_kind.as_deref() == Some("audio")
                && m.material_package_uid == copies[0].metadata.material_package_uid
        })
        .expect("matching sample audio");
    let audio_path = media.join("sample-audio.mxf");
    fs::copy(&audio.0, &audio_path)?;
    let stat = fs::metadata(&audio_path)?;
    copies.push(MediaFile {
        path: audio_path,
        folder: media.clone(),
        size: stat.len(),
        modified_ns: stat
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string(),
        metadata: audio.1.clone(),
    });
    let clip = mobboss_core::clips::group(copies)
        .into_iter()
        .find(|c| {
            c.files
                .iter()
                .any(|f| f.metadata.track_kind.as_deref() == Some("audio"))
        })
        .unwrap();
    let engine = mobboss_core::streaming::Streaming::default();
    let session = engine.start(
        ffmpeg.clone(),
        cache.clone(),
        mobboss_core::streaming::StartStream {
            id: "copied-sample-stream".into(),
            clip,
            tracks: vec![0],
            start: 0.0,
            encoder: None,
        },
    )?;
    let init = match session.read(None, std::time::Duration::from_secs(8)) {
        mobboss_core::streaming::ReadResult::Init(init, _) => init,
        mobboss_core::streaming::ReadResult::Failed(e) => anyhow::bail!("{e}"),
        _ => anyhow::bail!("missing copied-sample stream"),
    };
    assert!(init.tracks.iter().any(|t| t.video));
    assert!(init.tracks.iter().any(|t| t.audio && t.channels == 2));
    assert!(matches!(
        session.read(Some(0), std::time::Duration::ZERO),
        mobboss_core::streaming::ReadResult::Media(_)
    ));
    engine.shutdown();
    assert!(fs::read_dir(&cache.root)?.flatten().all(|e| !e
        .path()
        .extension()
        .is_some_and(|s| s == "mp4" || s == "webm")));
    assert_eq!(
        fs::read_dir(&media)?.count(),
        4,
        "only copied inputs in media"
    );
    Ok(())
}
