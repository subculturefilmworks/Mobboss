//! Opt-in essence reads. Scanning never invokes ffmpeg or writes to source media.
use crate::{clips::Clip, CancellationToken, MediaFile};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub binary: PathBuf,
}
impl Ffmpeg {
    /// Bundled sidecars win. Never discover a system ffmpeg through PATH.
    pub fn locate(bundled: &[PathBuf]) -> Option<Self> {
        bundled
            .iter()
            .find(|p| p.is_file())
            .cloned()
            .or_else(|| {
                std::env::var_os("MOBBOSS_FFMPEG")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute() && p.is_file())
            })
            .map(|binary| Self { binary })
    }
    pub fn run(
        &self,
        args: &[OsString],
        cancel: &CancellationToken,
        mut progress: impl FnMut(f64),
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "Preview cancelled");
        let mut command = Command::new(&self.binary);
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-progress",
                "pipe:1",
                "-nostats",
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .context("Could not start ffmpeg; previews unavailable")?;
        let stdout = child.stdout.take().context("ffmpeg stdout")?;
        let stderr = child.stderr.take().context("ffmpeg stderr")?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let output = std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(value) = line
                    .strip_prefix("out_time_us=")
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    let _ = sender.send(value / 1_000_000.0);
                }
            }
        });
        let errors = std::thread::spawn(move || {
            let mut tail = Vec::new();
            for part in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                tail.extend_from_slice(part.as_bytes());
                tail.push(b'\n');
                if tail.len() > 8192 {
                    tail.drain(..tail.len() - 8192);
                }
            }
            String::from_utf8_lossy(&tail).into_owned()
        });
        let status = loop {
            for seconds in receiver.try_iter() {
                progress(seconds);
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
                break child.wait();
            }
            match child.try_wait() {
                Ok(Some(s)) => break Ok(s),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(e);
                }
            }
        };
        let _ = output.join();
        let stderr = errors.join().unwrap_or_default();
        ensure!(!cancel.is_cancelled(), "Preview cancelled");
        ensure!(status?.success(), "Preview failed: {stderr}");
        Ok(())
    }
}
pub fn cache_key(file: &MediaFile) -> String {
    let mut hash = blake3::Hasher::new();
    for part in [
        "poster-v1",
        file.metadata
            .file_package_uid
            .as_deref()
            .unwrap_or_else(|| file.path.to_str().unwrap_or("")),
        &file.size.to_string(),
        &file.modified_ns,
    ] {
        hash.update(&(part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}
pub fn seconds(file: &MediaFile) -> f64 {
    file.metadata
        .edit_rate
        .as_ref()
        .filter(|r| r.numerator > 0 && r.denominator > 0)
        .map(|r| {
            file.metadata.duration.unwrap_or(0).max(0) as f64 * f64::from(r.denominator)
                / f64::from(r.numerator)
        })
        .unwrap_or(0.0)
}
fn strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}
pub fn composite_filter(width: u32) -> String {
    format!("[0:v:0]scale={width}:-2,format=rgba,split[fg][bg];[bg]lutrgb=r=32:g=32:b=32:a=255[base];[base][fg]overlay=shortest=1:format=auto,format=yuv420p[v]")
}
pub fn thumbnail_args(file: &MediaFile, output: &Path) -> Vec<OsString> {
    let seek = if file.metadata.duration == Some(1) {
        0.0
    } else {
        seconds(file) * 0.1
    };
    let mut args = strings(&["-y", "-threads", "1", "-ss", &format!("{seek:.6}"), "-i"]);
    args.push(file.path.as_os_str().into());
    args.extend(strings(&[
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        &composite_filter(320),
        "-map",
        "[v]",
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        "mjpeg",
        "-q:v",
        "4",
        "-f",
        "image2",
    ]));
    args.push(output.as_os_str().into());
    args
}
#[derive(Clone, Debug)]
pub struct PreviewCache {
    pub root: PathBuf,
}
impl PreviewCache {
    pub fn open(root: &Path) -> Result<Self> {
        let root = crate::validate_destination(root)?;
        fs::create_dir_all(&root)?;
        let cache = Self {
            root: root.canonicalize()?,
        };
        cache.cleanup_partials()?;
        Ok(cache)
    }
    pub fn path(&self, key: &str, extension: &str) -> Result<PathBuf> {
        ensure!(
            key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid preview key"
        );
        ensure!(
            ["jpg", "mp4", "webm"].contains(&extension),
            "Invalid preview type"
        );
        let path = self.root.join(format!("{key}.{extension}"));
        ensure!(
            !fs::symlink_metadata(&path).is_ok_and(|s| s.file_type().is_symlink()),
            "Preview cache symlink rejected"
        );
        Ok(path)
    }
    pub fn existing(&self, key: &str, extension: &str) -> Result<Option<PathBuf>> {
        let path = self.path(key, extension)?;
        if fs::metadata(&path).is_ok_and(|m| m.len() > 0 && m.is_file()) {
            File::open(&path)?.set_modified(SystemTime::now())?;
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
    /// Publish atomically; concurrent attempts serialize per key. Failed/cancelled jobs
    /// remove only their own temporary output, so retries never see partial previews.
    pub fn generate(
        &self,
        key: &str,
        extension: &str,
        job: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<PathBuf> {
        let output = self.path(key, extension)?;
        let lock_path = self.root.join(format!("{key}.{extension}.lock"));
        ensure!(
            !fs::symlink_metadata(&lock_path).is_ok_and(|s| s.file_type().is_symlink()),
            "Preview lock symlink rejected"
        );
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .context("Preview already being generated; retry shortly")?;
        if let Some(path) = self.existing(key, extension)? {
            return Ok(path);
        }
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let temp = self.root.join(format!(
            "{key}-{}-{}.partial.{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            job(&temp)?;
            ensure!(fs::metadata(&temp)?.len() > 0, "Empty preview output");
            fs::rename(&temp, &output)?;
            Ok(output)
        })();
        let _ = fs::remove_file(temp);
        result
    }
    pub fn thumbnail(
        &self,
        ffmpeg: &Ffmpeg,
        file: &MediaFile,
        cancel: &CancellationToken,
    ) -> Result<PathBuf> {
        ensure!(
            file.metadata.codec.as_deref() != Some("DNxUncompressed"),
            "No preview (Avid uncompressed title)"
        );
        verify_source(file)?;
        self.generate(&cache_key(file), "jpg", |temp| {
            ffmpeg.run(&thumbnail_args(file, temp), cancel, |_| {})?;
            verify_source(file)
        })
    }
}
pub fn verify_source(file: &MediaFile) -> Result<()> {
    let stat = fs::metadata(&file.path)?;
    ensure!(
        stat.is_file() && stat.len() == file.size && crate::ops::stamp(&stat) == file.modified_ns,
        "Source changed; rescan before generating previews"
    );
    Ok(())
}
#[derive(Clone, Serialize)]
pub struct ThumbnailProgress {
    pub done: usize,
    pub total: usize,
    pub clip_id: String,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
    pub cancelled: bool,
}
pub fn thumbnails(
    cache: &PreviewCache,
    ffmpeg: &Ffmpeg,
    clips: &[Clip],
    cancel: &CancellationToken,
    progress: impl Fn(ThumbnailProgress) + Sync,
) {
    let jobs: Vec<_> = clips
        .iter()
        .filter_map(|c| {
            c.files
                .iter()
                .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
                .map(|f| (c.id.clone(), f))
        })
        .collect();
    let next = AtomicUsize::new(0);
    let completed = Mutex::new(0usize);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let jobs = &jobs;
            let next = &next;
            let completed = &completed;
            let progress = &progress;
            scope.spawn(move || {
                while !cancel.is_cancelled() {
                    let Some((clip_id, file)) = jobs.get(next.fetch_add(1, Ordering::Relaxed))
                    else {
                        break;
                    };
                    let result = cache.thumbnail(ffmpeg, file, cancel);
                    let mut done = completed.lock().unwrap_or_else(|e| e.into_inner());
                    *done += 1;
                    let (path, error) = match result {
                        Ok(p) => (Some(p), None),
                        Err(e) => (None, Some(e.to_string())),
                    };
                    progress(ThumbnailProgress {
                        done: *done,
                        total: jobs.len(),
                        clip_id: clip_id.clone(),
                        path,
                        error,
                        cancelled: cancel.is_cancelled(),
                    });
                }
            });
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    fn file() -> MediaFile {
        MediaFile {
            path: "/media/video.mxf".into(),
            folder: "/media".into(),
            size: 100,
            modified_ns: "123".into(),
            metadata: crate::MediaMetadata {
                file_package_uid: Some("uid".into()),
                duration: Some(100),
                edit_rate: Some(crate::Rational {
                    numerator: 25,
                    denominator: 1,
                }),
                ..Default::default()
            },
        }
    }
    #[test]
    fn cache_keys_invalidate_and_copies_share() {
        let a = file();
        let mut b = a.clone();
        b.path = "/other/copy.mxf".into();
        assert_eq!(cache_key(&a), cache_key(&b));
        b.size += 1;
        assert_ne!(cache_key(&a), cache_key(&b));
        b = a.clone();
        b.modified_ns = "124".into();
        assert_ne!(cache_key(&a), cache_key(&b));
    }
    #[test]
    fn args_are_separate_seek_and_alpha_composite() {
        let a = file();
        let args = thumbnail_args(&a, Path::new("/cache/poster.jpg"));
        assert_eq!(args[4], "0.400000");
        assert!(args.iter().any(|s| s == "mjpeg"));
        assert!(composite_filter(320).contains("a=255"));
        let mut still = a;
        still.metadata.duration = Some(1);
        assert_eq!(
            thumbnail_args(&still, Path::new("/cache/a.jpg"))[4],
            "0.000000"
        );
    }
    #[test]
    fn retry_removes_partials_and_reuses_success() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        let key = cache_key(&file());
        assert!(cache
            .generate(&key, "jpg", |p| {
                fs::write(p, "partial")?;
                anyhow::bail!("cancelled")
            })
            .is_err());
        assert!(cache.existing(&key, "jpg").unwrap().is_none());
        let path = cache
            .generate(&key, "jpg", |p| {
                fs::write(p, "jpeg")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            cache.generate(&key, "jpg", |_| panic!("cached")).unwrap(),
            path
        );
        assert!(cache.path("../bad", "jpg").is_err());
        assert!(!fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|p| p.file_name().to_string_lossy().contains("partial")));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyPlatform {
    Mac,
    Windows,
    Linux,
}
impl ProxyPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
    pub fn extension(self) -> &'static str {
        if self == Self::Linux {
            "webm"
        } else {
            "mp4"
        }
    }
}
pub fn audio_files(clip: &Clip) -> Vec<&MediaFile> {
    let mut seen = std::collections::BTreeSet::new();
    clip.files
        .iter()
        .filter(|f| f.metadata.track_kind.as_deref() == Some("audio"))
        .filter(|f| {
            seen.insert(
                f.metadata
                    .file_package_uid
                    .clone()
                    .unwrap_or_else(|| f.path.to_string_lossy().into()),
            )
        })
        .collect()
}
fn proxy_inputs<'a>(
    clip: &'a Clip,
    tracks: &[usize],
) -> Result<(Option<&'a MediaFile>, Vec<&'a MediaFile>)> {
    ensure!(tracks.len() <= 2, "Choose at most two audio tracks");
    ensure!(
        tracks.len() != 2 || tracks[0] != tracks[1],
        "Choose different audio tracks"
    );
    let audio = audio_files(clip);
    let selected = tracks
        .iter()
        .map(|i| audio.get(*i).copied().context("Invalid audio track"))
        .collect::<Result<Vec<_>>>()?;
    let video = clip
        .files
        .iter()
        .find(|f| f.metadata.track_kind.as_deref() == Some("video"));
    ensure!(
        video.is_some() || !selected.is_empty(),
        "No playable video or audio in clip"
    );
    Ok((video, selected))
}
pub fn proxy_key(clip: &Clip, tracks: &[usize], platform: ProxyPlatform) -> Result<String> {
    let (video, audio) = proxy_inputs(clip, tracks)?;
    let mut hash = blake3::Hasher::new();
    hash.update(format!("proxy-v1-{platform:?}").as_bytes());
    for file in video.into_iter().chain(audio) {
        hash.update(cache_key(file).as_bytes());
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn tc_seconds(file: &MediaFile) -> Option<f64> {
    let m = &file.metadata;
    let rate = m
        .timecode_rate
        .as_ref()
        .map(|r| f64::from(r.numerator) / f64::from(r.denominator))
        .or(m.timecode_fps.map(f64::from))?;
    (rate > 0.0)
        .then(|| m.start_timecode.map(|n| n as f64 / rate))
        .flatten()
}
pub fn proxy_args(
    clip: &Clip,
    tracks: &[usize],
    platform: ProxyPlatform,
    output: &Path,
) -> Result<Vec<OsString>> {
    let (video, audio) = proxy_inputs(clip, tracks)?;
    let mut args = strings(&["-y"]);
    for file in video.into_iter().chain(audio.iter().copied()) {
        args.extend(strings(&["-threads", "2", "-i"]));
        args.push(file.path.as_os_str().into());
    }
    let mut filters = Vec::new();
    if video.is_some() {
        filters.push("[0:v:0]setpts=PTS-STARTPTS,scale=w='min(960,iw)':h='min(540,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,format=rgba,split[fg][bg];[bg]lutrgb=r=32:g=32:b=32:a=255[base];[base][fg]overlay=shortest=1:format=auto,format=yuv420p[v]".to_string());
    }
    let origin = video
        .or_else(|| audio.first().copied())
        .and_then(tc_seconds);
    for (i, file) in audio.iter().enumerate() {
        let delta = tc_seconds(file)
            .zip(origin)
            .map(|(a, v)| a - v)
            .unwrap_or(0.0);
        // Account for timecode rolling over midnight.
        let delta = if delta > 43200.0 {
            delta - 86400.0
        } else if delta < -43200.0 {
            delta + 86400.0
        } else {
            delta
        };
        let input = i + usize::from(video.is_some());
        filters.push(format!("[{input}:a:0]atrim=start={:.6},asetpts=PTS-STARTPTS,pan=mono|c0=c0,adelay={}:all=1,apad[a{i}]",(-delta).max(0.0),(delta.max(0.0)*1000.0).round() as u64));
    }
    match audio.len() {
        1 => filters.push("[a0]pan=stereo|c0=c0|c1=c0[a]".into()),
        2 => filters.push("[a0][a1]amerge=inputs=2,pan=stereo|c0=c0|c1=c1[a]".into()),
        _ => {}
    }
    args.extend(strings(&[
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        &filters.join(";"),
    ]));
    if video.is_some() {
        args.extend(strings(&[
            "-map",
            "[v]",
            "-c:v",
            match platform {
                ProxyPlatform::Mac => "h264_videotoolbox",
                ProxyPlatform::Windows => "h264_mf",
                ProxyPlatform::Linux => "libvpx",
            },
            "-b:v",
            "1800k",
            "-pix_fmt",
            "yuv420p",
        ]));
        if platform == ProxyPlatform::Linux {
            args.extend(strings(&["-deadline", "realtime", "-cpu-used", "4"]));
        }
        if platform == ProxyPlatform::Windows {
            args.extend(strings(&["-hw_encoding", "1"]));
        }
    }
    if !audio.is_empty() {
        args.extend(strings(&[
            "-map",
            "[a]",
            "-c:a",
            if platform == ProxyPlatform::Linux {
                "libopus"
            } else {
                "aac"
            },
            "-b:a",
            "128k",
            "-ar",
            "48000",
        ]));
    }
    let duration = video
        .map(seconds)
        .unwrap_or_else(|| audio.iter().map(|f| seconds(f)).fold(0.0, f64::max));
    ensure!(
        duration > 0.0 && duration.is_finite(),
        "Unknown clip duration; rescan headers"
    );
    args.extend(strings(&["-t", &format!("{duration:.6}"), "-threads", "2"]));
    if platform != ProxyPlatform::Linux {
        args.extend(strings(&["-movflags", "+faststart"]));
    }
    args.extend(strings(&[
        "-f",
        if platform == ProxyPlatform::Linux {
            "webm"
        } else {
            "mp4"
        },
    ]));
    args.push(output.as_os_str().into());
    Ok(args)
}
impl PreviewCache {
    pub fn proxy(
        &self,
        ffmpeg: &Ffmpeg,
        clip: &Clip,
        tracks: &[usize],
        platform: ProxyPlatform,
        cancel: &CancellationToken,
        progress: impl FnMut(f64),
    ) -> Result<PathBuf> {
        let (video, audio) = proxy_inputs(clip, tracks)?;
        for file in video.into_iter().chain(audio.iter().copied()) {
            verify_source(file)?;
        }
        self.generate(
            &proxy_key(clip, tracks, platform)?,
            platform.extension(),
            |temp| {
                ffmpeg.run(&proxy_args(clip, tracks, platform, temp)?, cancel, progress)?;
                for file in video.into_iter().chain(audio.iter().copied()) {
                    verify_source(file)?;
                }
                Ok(())
            },
        )
    }
    fn cleanup_partials(&self) -> Result<()> {
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((stem, extension)) = name.rsplit_once(".partial.") else {
                continue;
            };
            let Some((key, _)) = stem.split_once('-') else {
                continue;
            };
            if key.len() != 64
                || !key.bytes().all(|b| b.is_ascii_hexdigit())
                || !["jpg", "webm", "mp4"].contains(&extension)
            {
                continue;
            }
            let lock_path = self.root.join(format!("{key}.{extension}.lock"));
            if fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)?;
            if fs2::FileExt::try_lock_exclusive(&lock).is_ok() {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
    fn entries(&self) -> Result<Vec<(PathBuf, u64, SystemTime)>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((key, ext)) = name.rsplit_once('.') else {
                continue;
            };
            if key.len() != 64
                || !key.bytes().all(|b| b.is_ascii_hexdigit())
                || !["jpg", "mp4", "webm"].contains(&ext)
            {
                continue;
            }
            if entry.file_type()?.is_file() {
                let m = entry.metadata()?;
                files.push((path, m.len(), m.modified()?));
            }
        }
        Ok(files)
    }
    pub fn size(&self) -> Result<u64> {
        Ok(self.entries()?.iter().map(|(_, n, _)| n).sum())
    }
    /// Modification times are access times for immutable completed previews.
    /// Locked generations and the currently playing file are never evicted.
    pub fn prune(&self, cap: u64, protected: &[PathBuf]) -> Result<u64> {
        self.cleanup_partials()?;
        let mut entries = self.entries()?;
        entries.sort_by_key(|(_, _, time)| *time);
        let mut size: u64 = entries.iter().map(|(_, n, _)| n).sum();
        for (path, n, _) in entries {
            if size <= cap {
                break;
            }
            if protected.contains(&path) {
                continue;
            }
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            if fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)?;
            if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
                continue;
            }
            match fs::remove_file(path) {
                Ok(()) => size = size.saturating_sub(n),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => size = size.saturating_sub(n),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(size)
    }
    pub fn clear(&self) -> Result<()> {
        self.prune(0, &[])?;
        // Zero-byte lock files stay stable across app instances.
        Ok(())
    }
}
#[cfg(test)]
mod proxy_tests {
    use super::*;
    fn clip() -> Clip {
        let mut files = Vec::new();
        for (i, kind) in ["video", "audio", "audio", "audio"].into_iter().enumerate() {
            files.push(MediaFile {
                path: format!("/media/input {i}.mxf").into(),
                folder: "/media".into(),
                size: 10,
                modified_ns: "0".into(),
                metadata: crate::MediaMetadata {
                    file_package_uid: Some(format!("uid-{i}")),
                    track_kind: Some(kind.into()),
                    duration: Some(100),
                    edit_rate: Some(crate::Rational {
                        numerator: 25,
                        denominator: 1,
                    }),
                    start_timecode: Some(0),
                    timecode_fps: Some(25),
                    ..Default::default()
                },
            });
        }
        crate::clips::group(files)
            .into_iter()
            .next()
            .unwrap_or_else(|| unreachable!())
    }
    fn grouped() -> Clip {
        let mut c = clip();
        let mut files = Vec::new();
        for (i, kind) in ["video", "audio", "audio", "audio"].into_iter().enumerate() {
            let mut f = c.files[0].clone();
            f.path = format!("/media/input {i}.mxf").into();
            f.metadata.file_package_uid = Some(format!("uid-{i}"));
            f.metadata.track_kind = Some(kind.into());
            files.push(f);
        }
        c.files = files;
        c
    }
    #[test]
    fn hardware_platforms_channels_and_proxy_keys() {
        let clip = grouped();
        let out = Path::new("/cache/proxy.mp4");
        for (platform, encoder) in [
            (ProxyPlatform::Mac, "h264_videotoolbox"),
            (ProxyPlatform::Windows, "h264_mf"),
            (ProxyPlatform::Linux, "libvpx"),
        ] {
            let args = proxy_args(&clip, &[0, 1], platform, out).unwrap();
            assert!(args.iter().any(|s| s == encoder));
            let graph = args
                .iter()
                .find(|s| s.to_string_lossy().contains("amerge="))
                .unwrap()
                .to_string_lossy();
            assert!(graph.contains("[1:a:0]"));
            assert!(graph.contains("[2:a:0]"));
            assert!(graph.contains("c1=c1"));
            assert_eq!(
                args.iter().any(|s| s == "+faststart"),
                platform != ProxyPlatform::Linux
            );
        }
        let mono = proxy_args(&clip, &[0], ProxyPlatform::Mac, out).unwrap();
        assert!(mono.iter().any(|s| s.to_string_lossy().contains("c1=c0")));
        assert_ne!(
            proxy_key(&clip, &[0, 1], ProxyPlatform::Mac).unwrap(),
            proxy_key(&clip, &[1, 2], ProxyPlatform::Mac).unwrap()
        );
        assert!(proxy_args(&clip, &[3], ProxyPlatform::Mac, out).is_err());
        assert!(proxy_args(&clip, &[0, 0], ProxyPlatform::Mac, out).is_err());
    }
    #[test]
    fn orphan_partials_are_cleaned_but_live_jobs_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(temp.path()).unwrap();
        let key = "a".repeat(64);
        let path = cache.root.join(format!("{key}-123-1.partial.mp4"));
        fs::write(&path, [0; 50]).unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(cache.root.join(format!("{key}.mp4.lock")))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        PreviewCache::open(temp.path()).unwrap();
        assert!(path.exists());
        drop(lock);
        PreviewCache::open(temp.path()).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn sync_offsets_trim_or_delay_and_track_order_is_in_the_key() {
        let mut c = grouped();
        c.files[1].metadata.start_timecode = Some(25);
        c.files[2].metadata.start_timecode = Some(-25);
        let args = proxy_args(
            &c,
            &[0, 1],
            ProxyPlatform::Mac,
            Path::new("/cache/proxy.mp4"),
        )
        .unwrap();
        let graph = args
            .iter()
            .find(|s| s.to_string_lossy().contains("amerge"))
            .unwrap()
            .to_string_lossy();
        assert!(graph.contains("atrim=start=1.000000"));
        assert!(graph.contains("adelay=1000:all=1"));
        assert_ne!(
            proxy_key(&c, &[0, 1], ProxyPlatform::Mac).unwrap(),
            proxy_key(&c, &[1, 0], ProxyPlatform::Mac).unwrap()
        );
    }
    #[test]
    fn lru_keeps_recent_and_protected_and_clear_ignores_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(temp.path()).unwrap();
        let keys = ["1".repeat(64), "2".repeat(64), "3".repeat(64)];
        let mut paths = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            let path = cache.path(key, "mp4").unwrap();
            fs::write(&path, [0; 10]).unwrap();
            File::open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(i as u64))
                .unwrap();
            paths.push(path);
        }
        assert_eq!(cache.prune(20, &[paths[0].clone()]).unwrap(), 20);
        assert!(paths[0].exists());
        assert!(!paths[1].exists());
        assert!(paths[2].exists());
        fs::write(temp.path().join("unrelated.txt"), "keep").unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.size().unwrap(), 0);
        assert!(temp.path().join("unrelated.txt").exists());
    }
}
