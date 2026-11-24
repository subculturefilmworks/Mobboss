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
        Ok(Self {
            root: root.canonicalize()?,
        })
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
        verify_source(file)?;
        self.generate(&cache_key(file), "jpg", |temp| {
            ffmpeg.run(&thumbnail_args(file, temp), cancel, |_| {})
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
