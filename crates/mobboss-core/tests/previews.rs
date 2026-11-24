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
                    message.contains("decoder") || message.contains("Decoder"),
                    "unexpected title failure: {message}"
                );
                eprintln!("DNxUncompressed title limitation verified: {message}");
            }
            Err(e) => return Err(e),
        }
        copies.push(file);
    }
    assert_eq!(
        fs::read_dir(&media)?.count(),
        3,
        "only copied inputs in media"
    );
    Ok(())
}
