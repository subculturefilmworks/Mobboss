//! Opt-in essence reads. Scanning never invokes ffmpeg or writes to source media.
use crate::{clips::Clip, CancellationToken, MediaFile};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap, HashMap},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct FfmpegProgress {
    pub seconds: f64,
    pub output_bytes: Option<u64>,
    pub speed: Option<f64>,
    pub frames: Option<u64>,
    pub finished: bool,
}
#[derive(Default)]
pub struct ProgressParser {
    current: FfmpegProgress,
}
impl ProgressParser {
    /// FFmpeg's out_time_ms, despite its name, is also expressed in microseconds.
    pub fn line(&mut self, line: &str) -> Option<FfmpegProgress> {
        let (key, value) = line.trim().split_once('=')?;
        let value = value.trim();
        let number = |text: &str| text.parse::<f64>().ok().filter(|n| n.is_finite());
        match key {
            "out_time_us" | "out_time_ms" => {
                if let Some(n) = number(value) {
                    self.current.seconds = (n / 1_000_000.0).max(0.0);
                }
            }
            "out_time" => {
                let parts: Vec<_> = value.split(':').collect();
                if parts.len() == 3 {
                    if let (Some(h), Some(m), Some(s)) =
                        (number(parts[0]), number(parts[1]), number(parts[2]))
                    {
                        if h >= 0.0 && (0.0..60.0).contains(&m) && (0.0..60.0).contains(&s) {
                            let total = h * 3600.0 + m * 60.0 + s;
                            if total.is_finite() {
                                self.current.seconds = total;
                            }
                        }
                    }
                }
            }
            "total_size" => self.current.output_bytes = value.parse().ok(),
            "speed" => {
                self.current.speed = number(value.trim_end_matches('x')).filter(|s| *s > 0.0)
            }
            "frame" => self.current.frames = value.trim().parse().ok(),
            "progress" if value == "continue" || value == "end" => {
                self.current.finished = value == "end";
                return Some(std::mem::take(&mut self.current));
            }
            _ => {}
        }
        None
    }
}
#[derive(Clone, Serialize)]
pub struct PreparationProgress {
    pub phase: &'static str,
    pub fraction: f64,
    pub seconds: f64,
    pub duration: f64,
    pub input_bytes: u64,
    pub input_files: usize,
    pub output_bytes: Option<u64>,
    pub eta_seconds: Option<f64>,
    pub speed: Option<f64>,
}
pub fn preparation_progress(
    update: FfmpegProgress,
    duration: f64,
    source: (u64, usize),
    elapsed: Duration,
) -> PreparationProgress {
    let duration = if duration.is_finite() {
        duration.max(0.0)
    } else {
        0.0
    };
    let seconds = update.seconds.max(0.0).min(duration);
    let speed = update
        .speed
        .filter(|n| n.is_finite() && *n > 0.0)
        .or_else(|| {
            (elapsed.as_secs_f64() > 0.0 && seconds > 0.0).then(|| seconds / elapsed.as_secs_f64())
        });
    PreparationProgress {
        phase: if update.finished {
            "finalizing"
        } else if seconds > 0.0 || update.frames.is_some_and(|n| n > 0) {
            "encoding"
        } else {
            "reading"
        },
        fraction: if update.finished {
            1.0
        } else if duration > 0.0 {
            seconds / duration
        } else {
            0.0
        },
        seconds,
        duration,
        input_bytes: source.0,
        input_files: source.1,
        output_bytes: update.output_bytes,
        eta_seconds: if update.finished {
            None
        } else {
            speed.map(|speed| (duration - seconds) / speed)
        },
        speed,
    }
}
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
        mut progress: impl FnMut(FfmpegProgress),
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
                "-stats_period",
                "0.25",
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
            let mut parser = ProgressParser::default();
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(update) = parser.line(&line) {
                    let _ = sender.send(update);
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
                Ok(Some(_)) => break child.wait(),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(e);
                }
            }
        };
        // Release the process handle before callers clean up or publish its output.
        drop(child);
        let _ = output.join();
        for update in receiver.try_iter() {
            progress(update);
        }
        let stderr = errors.join().unwrap_or_default();
        ensure!(!cancel.is_cancelled(), "Preview cancelled");
        ensure!(status?.success(), "Preview failed: {stderr}");
        Ok(())
    }
}
// Antivirus/indexer handles can briefly outlive a reaped Windows process.
// Do not retry unrelated errors or delay normal cleanup on other platforms.
fn retry_partial_cleanup(
    mut remove: impl FnMut() -> std::io::Result<()>,
    mut pause: impl FnMut(Duration),
    windows: bool,
) -> std::io::Result<()> {
    let delays = [25, 75, 150];
    let mut attempt = 0;
    loop {
        match remove() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if windows
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && attempt < delays.len() =>
            {
                pause(Duration::from_millis(delays[attempt]));
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
pub fn cache_key(file: &MediaFile) -> String {
    let mut hash = blake3::Hasher::new();
    for part in [
        if file.metadata.track_kind.as_deref() == Some("audio") {
            "waveform-v1"
        } else {
            "poster-v1"
        },
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
fn flatten_filter() -> &'static str {
    "format=rgba,split[fg][bg];[bg]lutrgb=r=32:g=32:b=32:a=255[base];[base][fg]overlay=shortest=1:format=auto,format=yuv420p[v]"
}
pub fn composite_filter(width: u32) -> String {
    format!("[0:v:0]scale={width}:-2,{}", flatten_filter())
}
/// Audio-only clips use the first track; video clips always keep their picture poster.
pub fn poster_source(clip: &Clip) -> Option<&MediaFile> {
    clip.files
        .iter()
        .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
        .or_else(|| {
            clip.files
                .iter()
                .find(|f| f.metadata.track_kind.as_deref() == Some("audio"))
        })
}
pub fn thumbnail_args(file: &MediaFile, output: &Path) -> Vec<OsString> {
    let audio = file.metadata.track_kind.as_deref() == Some("audio");
    let seek = if file.metadata.duration == Some(1) {
        0.0
    } else {
        seconds(file) * 0.1
    };
    let mut args = strings(&["-y", "-threads", "1"]);
    if audio {
        // showwavespic reads the whole input before emitting one frame. Bound NAS work.
        args.extend(strings(&["-t", "60", "-i"]));
    } else {
        args.extend(strings(&["-ss", &format!("{seek:.6}"), "-i"]));
    }
    args.push(file.path.as_os_str().into());
    let filter = if audio {
        format!("[0:a:0]aformat=channel_layouts=mono,showwavespic=s=320x180:colors=0x6aa8ff:scale=sqrt:draw=full:filter=peak,{}", flatten_filter())
    } else {
        composite_filter(320)
    };
    args.extend(strings(&[
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        &filter,
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
            root: crate::paths::canonical(&root)?,
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
        self.existing_with_touch(key, extension, |path| {
            // SetFileTime on Windows requires FILE_WRITE_ATTRIBUTES, which a
            // read-only File::open handle does not have. Never truncate cached data.
            let file = self.io("open for LRU update", &[path], || {
                OpenOptions::new().write(true).open(path)
            })?;
            self.io("update LRU timestamp", &[path], || {
                file.set_modified(SystemTime::now())
            })
        })
    }
    fn existing_with_touch(
        &self,
        key: &str,
        extension: &str,
        touch: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<Option<PathBuf>> {
        let path = self.path(key, extension)?;
        if fs::metadata(&path).is_ok_and(|m| m.len() > 0 && m.is_file()) {
            if let Err(error) = touch(&path) {
                // Recency bookkeeping must never prevent playback of a valid preview.
                eprintln!("Preview LRU update skipped: {error}");
            }
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
    fn io<T>(
        &self,
        operation: &str,
        paths: &[&Path],
        run: impl FnOnce() -> std::io::Result<T>,
    ) -> Result<T> {
        run().map_err(|error| {
            let names = paths
                .iter()
                .map(|path| {
                    path.strip_prefix(&self.root)
                        .unwrap_or(Path::new("<outside cache>"))
                        .to_string_lossy()
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            anyhow::anyhow!("Preview cache {operation} [{names}]: {error}")
        })
    }
    fn remove_partial(&self, path: &Path) -> Result<()> {
        self.io("remove partial", &[path], || {
            retry_partial_cleanup(|| fs::remove_file(path), std::thread::sleep, cfg!(windows))
        })
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
        let lock = self.io("open generation lock", &[&lock_path], || {
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
        })?;
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
        // A separate name and closed reservation handle for every encoder attempt.
        drop(self.io("create partial", &[&temp], || {
            OpenOptions::new().write(true).create_new(true).open(&temp)
        })?);
        let result = (|| {
            job(&temp)?;
            ensure!(
                self.io("stat partial", &[&temp], || fs::metadata(&temp))?
                    .len()
                    > 0,
                "Empty preview output"
            );
            self.io("publish without replacement", &[&temp, &output], || {
                crate::ops::publish(&temp, &output)
            })?;
            Ok(output)
        })();
        let cleanup = self.remove_partial(&temp);
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error}; {cleanup}")),
            (Err(error), _) => Err(error),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(path), Ok(())) => Ok(path),
        }
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
    #[serde(serialize_with = "crate::paths::serialize_optional")]
    pub path: Option<PathBuf>,
    pub error: Option<String>,
    pub cancelled: bool,
}
/// Rebuild priorities for pending jobs only; claimed/finished jobs never re-enter.
pub struct ThumbnailQueue {
    jobs: Vec<(String, MediaFile)>,
    state: Mutex<ThumbnailQueueState>,
}
struct ThumbnailQueueState {
    pending: BTreeSet<usize>,
    heap: BinaryHeap<Reverse<(u8, usize, usize)>>,
}
impl ThumbnailQueue {
    pub fn new(clips: &[Clip]) -> Self {
        let jobs: Vec<_> = clips
            .iter()
            .filter_map(|clip| poster_source(clip).map(|file| (clip.id.clone(), file.clone())))
            .collect();
        let queue = Self {
            state: Mutex::new(ThumbnailQueueState {
                pending: (0..jobs.len()).collect(),
                heap: BinaryHeap::new(),
            }),
            jobs,
        };
        queue.prioritize(&[], &[]);
        queue
    }
    pub fn prioritize(&self, visible: &[String], display: &[String]) {
        let display: HashMap<_, _> = display
            .iter()
            .enumerate()
            .map(|(rank, id)| (id, rank))
            .collect();
        let visible: BTreeSet<_> = visible.iter().collect();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.heap = state
            .pending
            .iter()
            .map(|&index| {
                let id = &self.jobs[index].0;
                let priority = match display.get(id) {
                    Some(&rank) if visible.contains(id) => (0, rank, index),
                    Some(&rank) => (1, rank, index),
                    None => (2, index, index),
                };
                Reverse(priority)
            })
            .collect();
    }
    fn pop(&self) -> Option<(String, MediaFile)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Reverse((_, _, index)) = state.heap.pop()?;
        state.pending.remove(&index);
        Some(self.jobs[index].clone())
    }
}
pub fn thumbnails(
    cache: &PreviewCache,
    ffmpeg: &Ffmpeg,
    queue: &ThumbnailQueue,
    cancel: &CancellationToken,
    progress: impl Fn(ThumbnailProgress) + Sync,
) {
    let completed = Mutex::new(0usize);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let completed = &completed;
            let progress = &progress;
            scope.spawn(move || {
                while !cancel.is_cancelled() {
                    let Some((clip_id, file)) = queue.pop() else {
                        break;
                    };
                    let result = cache.thumbnail(ffmpeg, &file, cancel);
                    let mut done = completed.lock().unwrap_or_else(|e| e.into_inner());
                    *done += 1;
                    let (path, error) = match result {
                        Ok(p) => (Some(p), None),
                        Err(e) => (None, Some(e.to_string())),
                    };
                    progress(ThumbnailProgress {
                        done: *done,
                        total: queue.jobs.len(),
                        clip_id,
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
    fn queued_clips() -> Vec<Clip> {
        crate::clips::group(
            (0..6)
                .map(|i| {
                    let mut f = file();
                    f.metadata.track_kind = Some("video".into());
                    f.metadata.material_package_uid = Some(i.to_string());
                    f.path = format!("/media/{i}.mxf").into();
                    f
                })
                .collect(),
        )
    }
    #[test]
    fn audio_waveform_jobs_share_priority_queue_and_prefer_video_when_present() {
        let mut audio = file();
        audio.path = "/media/audio.mxf".into();
        audio.metadata.track_kind = Some("audio".into());
        audio.metadata.material_package_uid = Some("audio-only".into());
        let mut video = audio.clone();
        video.path = "/media/video.mxf".into();
        video.metadata.track_kind = Some("video".into());
        let clips = crate::clips::group(vec![audio.clone()]);
        let queue = ThumbnailQueue::new(&clips);
        assert_eq!(queue.pop().unwrap().1.path, audio.path);
        assert!(queue.pop().is_none());
        let mixed = crate::clips::group(vec![audio.clone(), video.clone()]);
        assert_eq!(poster_source(&mixed[0]).unwrap().path, video.path);
        assert_ne!(cache_key(&audio), cache_key(&video));
        let args = thumbnail_args(&audio, Path::new("/cache/waveform.jpg"));
        let args: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(&args[input - 2..input], ["-t", "60"]);
        assert!(!args.iter().any(|a| a == "-ss"));
        let filter = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(filter.contains("[0:a:0]aformat=channel_layouts=mono,showwavespic=s=320x180"));
        assert!(filter.contains("colors=0x6aa8ff") && filter.contains("lutrgb=r=32:g=32:b=32"));
        assert_eq!(args.last().unwrap(), "/cache/waveform.jpg");
    }
    #[test]
    fn thumbnail_queue_orders_viewport_display_then_filtered_out() {
        let queue = ThumbnailQueue::new(&queued_clips());
        let ids = |items: &[u8]| items.iter().map(|i| format!("uid:{i}")).collect::<Vec<_>>();
        queue.prioritize(&ids(&[0, 4, 1]), &ids(&[3, 1, 4]));
        let mut order = Vec::new();
        while let Some((id, _)) = queue.pop() {
            order.push(id);
        }
        // Stale viewport ID 0 is filtered out and cannot jump the display order.
        assert_eq!(order, ids(&[1, 4, 3, 0, 2, 5]));
    }
    #[test]
    fn thumbnail_reprioritization_preserves_claimed_jobs_without_duplicates() {
        let queue = ThumbnailQueue::new(&queued_clips());
        assert_eq!(queue.pop().unwrap().0, "uid:0");
        assert_eq!(queue.pop().unwrap().0, "uid:1");
        queue.prioritize(
            &["uid:5".into(), "uid:0".into()],
            &["uid:5".into(), "uid:0".into(), "uid:3".into()],
        );
        assert_eq!(queue.pop().unwrap().0, "uid:5");
        queue.prioritize(
            &["uid:4".into()],
            &["uid:4".into(), "uid:3".into(), "uid:2".into()],
        );
        let mut order = Vec::new();
        while let Some((id, _)) = queue.pop() {
            order.push(id);
        }
        assert_eq!(order, ["uid:4", "uid:3", "uid:2"]);
        queue.prioritize(&[], &["uid:0".into()]);
        assert!(queue.pop().is_none());
    }
    #[test]
    fn concurrent_thumbnail_claims_remain_unique_during_reprioritization() {
        let queue = ThumbnailQueue::new(&queued_clips());
        let claimed = Mutex::new(Vec::new());
        let started = std::sync::Barrier::new(3);
        let resume = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let queue = &queue;
                let claimed = &claimed;
                let started = &started;
                let resume = &resume;
                scope.spawn(move || {
                    claimed.lock().unwrap().push(queue.pop().unwrap().0);
                    started.wait();
                    resume.wait();
                    while let Some((id, _)) = queue.pop() {
                        claimed.lock().unwrap().push(id);
                    }
                });
            }
            started.wait();
            for _ in 0..20 {
                queue.prioritize(&["uid:4".into()], &["uid:4".into(), "uid:2".into()]);
            }
            resume.wait();
        });
        let claimed = claimed.into_inner().unwrap();
        assert_eq!(claimed.len(), 6);
        assert_eq!(claimed.into_iter().collect::<BTreeSet<_>>().len(), 6);
    }
    #[test]
    fn progress_parser_reads_complete_records_and_microseconds() {
        let mut parser = ProgressParser::default();
        for line in [
            "frame= 42",
            "out_time_us=1500000",
            "out_time_ms=1500000",
            "total_size= 8192",
            "speed= 2.5x",
        ] {
            assert!(parser.line(line).is_none());
        }
        let p = parser.line("progress=continue").unwrap();
        assert_eq!(p.seconds, 1.5);
        assert_eq!(p.frames, Some(42));
        assert_eq!(p.output_bytes, Some(8192));
        assert_eq!(p.speed, Some(2.5));
        assert!(!p.finished);
        parser.line("out_time=01:02:03.125");
        let p = parser.line("progress=end\r").unwrap();
        assert_eq!(p.seconds, 3723.125);
        assert!(p.finished);
        assert!(p.speed.is_none());
        assert!(p.output_bytes.is_none());
    }
    #[test]
    fn progress_rejects_na_malformed_and_nonfinite_values() {
        let mut parser = ProgressParser::default();
        for line in [
            "out_time_us=-1",
            "out_time=oops",
            "out_time=00:99:00",
            "out_time_us=NaN",
            "speed=N/A",
            "total_size=N/A",
            "frame=invalid",
            "out_time=1e308:00:00",
            "ignored",
        ] {
            parser.line(line);
        }
        let p = parser.line("progress=end").unwrap();
        assert_eq!(p.seconds, 0.0);
        assert!(p.speed.is_none());
        assert!(p.output_bytes.is_none());
    }
    #[test]
    fn preparation_phases_and_eta_use_measured_output_time() {
        let initial =
            preparation_progress(FfmpegProgress::default(), 60.0, (1000, 3), Duration::ZERO);
        assert_eq!(initial.phase, "reading");
        assert!(initial.eta_seconds.is_none());
        let update = FfmpegProgress {
            seconds: 10.0,
            speed: Some(2.0),
            output_bytes: Some(100),
            ..Default::default()
        };
        let encoding = preparation_progress(update, 60.0, (1000, 3), Duration::from_secs(8));
        assert_eq!(encoding.phase, "encoding");
        assert_eq!(encoding.eta_seconds, Some(25.0));
        assert_eq!(encoding.input_bytes, 1000);
        assert_eq!(encoding.input_files, 3);
        assert_eq!(encoding.output_bytes, Some(100));
        let measured = preparation_progress(
            FfmpegProgress {
                speed: None,
                ..update
            },
            60.0,
            (1000, 3),
            Duration::from_secs(5),
        );
        assert_eq!(measured.eta_seconds, Some(25.0));
        let done = preparation_progress(
            FfmpegProgress {
                finished: true,
                ..update
            },
            60.0,
            (1000, 3),
            Duration::from_secs(5),
        );
        assert_eq!(done.phase, "finalizing");
        assert_eq!(done.fraction, 1.0);
        assert!(done.eta_seconds.is_none());
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
    WindowsWebm,
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
    pub fn is_webm(self) -> bool {
        matches!(self, Self::Linux | Self::WindowsWebm)
    }
    pub fn encoder(self) -> &'static str {
        match self {
            Self::Mac => "h264_videotoolbox",
            Self::Windows => "h264_mf",
            Self::WindowsWebm | Self::Linux => "libvpx",
        }
    }
    pub fn extension(self) -> &'static str {
        if self.is_webm() {
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
pub(crate) fn proxy_inputs<'a>(
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
pub fn proxy_source_summary(clip: &Clip, tracks: &[usize]) -> Result<(u64, usize)> {
    let (video, audio) = proxy_inputs(clip, tracks)?;
    Ok(video
        .into_iter()
        .chain(audio)
        .fold((0u64, 0), |(bytes, count), file| {
            (bytes.saturating_add(file.size), count + 1)
        }))
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
pub(crate) fn tc_seconds(file: &MediaFile) -> Option<f64> {
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
            platform.encoder(),
            "-b:v",
            "1800k",
            "-pix_fmt",
            "yuv420p",
        ]));
        if platform.is_webm() {
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
            if platform.is_webm() { "libopus" } else { "aac" },
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
    if !platform.is_webm() {
        args.extend(strings(&["-movflags", "+faststart"]));
    }
    args.extend(strings(&[
        "-f",
        if platform.is_webm() { "webm" } else { "mp4" },
    ]));
    args.push(output.as_os_str().into());
    Ok(args)
}
#[cfg(test)]
#[derive(Serialize)]
pub struct ProxyOutput {
    pub path: PathBuf,
    pub encoder: &'static str,
}
/// Only retry encoder initialization, never corrupt inputs, disk errors or cancellation.
pub fn mf_encoder_open_error(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("h264_mf")
        && [
            "could not find any mft",
            "error while opening encoder",
            "unknown encoder 'h264_mf'",
        ]
        .iter()
        .any(|message| text.contains(message))
}
fn encoder_choice_key(ffmpeg: &Ffmpeg) -> Result<String> {
    let stat = fs::metadata(&ffmpeg.binary)?;
    Ok(blake3::hash(
        format!(
            "{}:{}:{:?}",
            ffmpeg.binary.display(),
            stat.len(),
            stat.modified()?
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string())
}
impl PreviewCache {
    /// Persist only a binary fingerprint and container, never machine names or source paths.
    pub(crate) fn windows_choice(&self, ffmpeg: &Ffmpeg) -> ProxyPlatform {
        let path = self.root.join("windows-encoder.txt");
        if fs::symlink_metadata(&path).is_ok_and(|s| s.is_file() && s.len() < 128) {
            if let (Ok(key), Ok(choice)) = (encoder_choice_key(ffmpeg), fs::read_to_string(path)) {
                if choice == format!("{key}\nwebm\n") {
                    return ProxyPlatform::WindowsWebm;
                }
            }
        }
        ProxyPlatform::Windows
    }
    pub(crate) fn remember_windows_choice(
        &self,
        ffmpeg: &Ffmpeg,
        profile: ProxyPlatform,
    ) -> Result<()> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let temp = self.root.join(format!(
            ".windows-encoder-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            use std::io::Write;
            writeln!(
                output,
                "{}\n{}",
                encoder_choice_key(ffmpeg)?,
                profile.extension()
            )?;
            drop(output);
            fs::rename(&temp, self.root.join("windows-encoder.txt"))?;
            Ok(())
        })();
        let _ = fs::remove_file(temp);
        result
    }
    #[cfg(test)]
    fn with_fallback(
        &self,
        ffmpeg: &Ffmpeg,
        platform: ProxyPlatform,
        video: bool,
        cancel: &CancellationToken,
        mut run: impl FnMut(ProxyPlatform) -> Result<PathBuf>,
    ) -> Result<ProxyOutput> {
        ensure!(!cancel.is_cancelled(), "Preview cancelled");
        let mut profile = if platform == ProxyPlatform::Windows {
            self.windows_choice(ffmpeg)
        } else {
            platform
        };
        let path = match run(profile) {
            Ok(path) => path,
            Err(error)
                if profile == ProxyPlatform::Windows
                    && video
                    && !cancel.is_cancelled()
                    && mf_encoder_open_error(&error) =>
            {
                profile = ProxyPlatform::WindowsWebm;
                eprintln!(
                    "Windows preview: h264_mf unavailable; retrying libvpx/Opus WebM ({error})"
                );
                run(profile).map_err(|fallback| {
                    anyhow::anyhow!(
                        "H.264 encoder unavailable ({error}); WebM fallback failed: {fallback}"
                    )
                })?
            }
            Err(error) => return Err(error),
        };
        ensure!(!cancel.is_cancelled(), "Preview cancelled");
        if platform == ProxyPlatform::Windows && video {
            let _ = self.remember_windows_choice(ffmpeg, profile);
        }
        Ok(ProxyOutput {
            path,
            encoder: if profile.is_webm() {
                if video {
                    "libvpx (VP8) · WebM"
                } else {
                    "libopus · WebM"
                }
            } else if !video {
                "aac · MP4"
            } else if profile == ProxyPlatform::Mac {
                "h264_videotoolbox · MP4"
            } else {
                "h264_mf · MP4"
            },
        })
    }
}
impl PreviewCache {
    /// Upgrade from file proxies to memory streaming. Keep JPEGs and encoder preferences.
    /// Only recognized, regular cache outputs are removed; never follow symlinks.
    pub fn remove_legacy_proxies(&self) -> Result<()> {
        for (path, _, _) in self.entries()? {
            if path.extension().is_some_and(|e| e == "mp4" || e == "webm") {
                self.io("remove legacy proxy", &[&path], || {
                    retry_partial_cleanup(
                        || fs::remove_file(&path),
                        std::thread::sleep,
                        cfg!(windows),
                    )
                })?;
            }
        }
        Ok(())
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
                self.remove_partial(&entry.path())?;
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
        assert_eq!(proxy_source_summary(&clip, &[0, 1]).unwrap(), (30, 3));
        let out = Path::new("/cache/proxy.mp4");
        for (platform, encoder) in [
            (ProxyPlatform::Mac, "h264_videotoolbox"),
            (ProxyPlatform::Windows, "h264_mf"),
            (ProxyPlatform::Linux, "libvpx"),
            (ProxyPlatform::WindowsWebm, "libvpx"),
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
            assert_eq!(args.iter().any(|s| s == "+faststart"), !platform.is_webm());
        }
        assert_ne!(
            proxy_key(&clip, &[0], ProxyPlatform::Windows).unwrap(),
            proxy_key(&clip, &[0], ProxyPlatform::WindowsWebm).unwrap()
        );
        let webm = proxy_args(&clip, &[0], ProxyPlatform::WindowsWebm, out).unwrap();
        assert!(webm.iter().any(|s| s == "libopus"));
        assert!(!webm.iter().any(|s| s == "-hw_encoding"));
        assert_eq!(ProxyPlatform::WindowsWebm.extension(), "webm");
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
    fn windows_fallback_is_remembered_and_invalidated_with_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = Ffmpeg {
            binary: dir.path().join("ffmpeg"),
        };
        fs::write(&ffmpeg.binary, "binary-v1").unwrap();
        let cache = PreviewCache::open(&dir.path().join("previews")).unwrap();
        let token = CancellationToken::default();
        let mut attempts = Vec::new();
        let result = cache
            .with_fallback(&ffmpeg, ProxyPlatform::Windows, true, &token, |profile| {
                attempts.push(profile);
                if profile == ProxyPlatform::Windows {
                    anyhow::bail!("h264_mf: could not find any MFT for the given media type");
                }
                Ok(cache.root.join("fallback.webm"))
            })
            .unwrap();
        assert_eq!(
            attempts,
            [ProxyPlatform::Windows, ProxyPlatform::WindowsWebm]
        );
        assert!(result.encoder.contains("libvpx"));
        let reopened = PreviewCache::open(&cache.root).unwrap();
        reopened.clear().unwrap();
        reopened
            .with_fallback(&ffmpeg, ProxyPlatform::Windows, true, &token, |profile| {
                assert_eq!(profile, ProxyPlatform::WindowsWebm);
                Ok(reopened.root.join("another.webm"))
            })
            .unwrap();
        fs::write(&ffmpeg.binary, "replacement-binary-v2").unwrap();
        let result = reopened
            .with_fallback(&ffmpeg, ProxyPlatform::Windows, true, &token, |profile| {
                assert_eq!(profile, ProxyPlatform::Windows);
                Ok(reopened.root.join("native.mp4"))
            })
            .unwrap();
        assert!(result.encoder.contains("h264_mf"));
    }
    #[test]
    fn windows_retry_does_not_hide_other_errors_or_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        let ffmpeg = Ffmpeg {
            binary: dir.path().join("unused"),
        };
        for message in [
            "No space left on device",
            "Invalid input data",
            "h264_mf: error writing output",
            "h264_mf: could not initialize filter graph",
            "h264_mf configured; Unknown encoder 'aac'",
        ] {
            let mut calls = 0;
            assert!(cache
                .with_fallback(
                    &ffmpeg,
                    ProxyPlatform::Windows,
                    true,
                    &CancellationToken::default(),
                    |_| {
                        calls += 1;
                        anyhow::bail!("{message}")
                    }
                )
                .is_err());
            assert_eq!(calls, 1);
        }
        let token = CancellationToken::default();
        let mut calls = 0;
        assert!(cache
            .with_fallback(&ffmpeg, ProxyPlatform::Windows, true, &token, |_| {
                calls += 1;
                token.cancel();
                anyhow::bail!("h264_mf: Error while opening encoder")
            })
            .is_err());
        assert_eq!(calls, 1);
    }
    #[test]
    fn failed_fallback_is_not_remembered_and_macos_never_retries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        let ffmpeg = Ffmpeg {
            binary: dir.path().join("ffmpeg"),
        };
        fs::write(&ffmpeg.binary, "binary").unwrap();
        let token = CancellationToken::default();
        let mut calls = 0;
        let error = cache
            .with_fallback(&ffmpeg, ProxyPlatform::Windows, true, &token, |_| {
                calls += 1;
                if calls == 1 {
                    anyhow::bail!("h264_mf: Error while opening encoder");
                }
                anyhow::bail!("fallback disk error")
            })
            .err()
            .unwrap();
        assert_eq!(calls, 2);
        assert!(error.to_string().contains("fallback disk error"));
        assert_eq!(cache.windows_choice(&ffmpeg), ProxyPlatform::Windows);
        calls = 0;
        assert!(cache
            .with_fallback(&ffmpeg, ProxyPlatform::Mac, true, &token, |_| {
                calls += 1;
                anyhow::bail!("h264_mf: Error while opening encoder")
            })
            .is_err());
        assert_eq!(calls, 1);
    }
    #[test]
    fn windows_encoder_failure_retries_fresh_and_cached_outputs() {
        for cached in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let ffmpeg = Ffmpeg {
                binary: dir.path().join("ffmpeg"),
            };
            fs::write(&ffmpeg.binary, "binary").unwrap();
            let cache = PreviewCache::open(&dir.path().join("previews")).unwrap();
            let clip = grouped();
            let webm_key = proxy_key(&clip, &[0], ProxyPlatform::WindowsWebm).unwrap();
            let webm_path = cache.path(&webm_key, "webm").unwrap();
            if cached {
                fs::write(&webm_path, "completed webm").unwrap();
                OpenOptions::new()
                    .write(true)
                    .open(&webm_path)
                    .unwrap()
                    .set_modified(SystemTime::UNIX_EPOCH)
                    .unwrap();
                // This is the precise Windows API contract behind rc6's cache-hit failure.
                #[cfg(windows)]
                assert_eq!(
                    std::fs::File::open(&webm_path)
                        .unwrap()
                        .set_modified(SystemTime::now())
                        .unwrap_err()
                        .kind(),
                    std::io::ErrorKind::PermissionDenied
                );
            }
            let mut partials = Vec::new();
            let mut calls = Vec::new();
            let output = cache
                .with_fallback(
                    &ffmpeg,
                    ProxyPlatform::Windows,
                    true,
                    &CancellationToken::default(),
                    |profile| {
                        calls.push(profile);
                        let key = proxy_key(&clip, &[0], profile)?;
                        cache.generate(&key, profile.extension(), |partial| {
                            assert!(!partials.iter().any(|p: &PathBuf| p.exists()));
                            partials.push(partial.to_owned());
                            if profile == ProxyPlatform::Windows {
                                fs::write(partial, "failed mp4")?;
                                anyhow::bail!(
                                    "h264_mf: could not find any MFT for the given media type"
                                );
                            }
                            assert!(!cached, "cache hit must not re-encode");
                            fs::write(partial, "completed webm")?;
                            Ok(())
                        })
                    },
                )
                .unwrap();
            assert_eq!(calls, [ProxyPlatform::Windows, ProxyPlatform::WindowsWebm]);
            assert_eq!(output.path, webm_path);
            assert_eq!(fs::read(&webm_path).unwrap(), b"completed webm");
            assert!(fs::metadata(&webm_path).unwrap().modified().unwrap() > SystemTime::UNIX_EPOCH);
            assert!(partials.iter().all(|p| !p.exists()));
            assert_eq!(partials.len(), if cached { 1 } else { 2 });
        }
    }
    #[test]
    fn preview_publish_never_replaces_a_racing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        let key = "a".repeat(64);
        let output = cache.path(&key, "webm").unwrap();
        let mut partial = PathBuf::new();
        let error = cache
            .generate(&key, "webm", |temp| {
                partial = temp.into();
                fs::write(temp, "new output")?;
                fs::write(&output, "existing output")?;
                Ok(())
            })
            .unwrap_err()
            .to_string();
        assert_eq!(fs::read(output).unwrap(), b"existing output");
        assert!(!partial.exists());
        assert!(error.contains("publish without replacement"));
        assert!(error.contains(&format!("{key}.webm")));
        assert!(!error.contains(dir.path().to_str().unwrap()));
    }
    #[test]
    fn failed_lru_touch_does_not_block_a_completed_preview() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        let key = "b".repeat(64);
        let path = cache.path(&key, "webm").unwrap();
        fs::write(&path, "completed preview").unwrap();
        let hit = cache
            .existing_with_touch(&key, "webm", |p| {
                cache.io("update LRU timestamp", &[p], || {
                    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                })
            })
            .unwrap();
        assert_eq!(hit, Some(path.clone()));
        assert_eq!(fs::read(path).unwrap(), b"completed preview");
    }
    #[test]
    fn streaming_upgrade_removes_only_recognized_proxies_and_keeps_posters() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::open(dir.path()).unwrap();
        for extension in ["jpg", "mp4", "webm"] {
            fs::write(cache.path(&"a".repeat(64), extension).unwrap(), "data").unwrap();
        }
        fs::write(dir.path().join("unrelated.mp4"), "keep").unwrap();
        fs::write(dir.path().join("windows-encoder.txt"), "keep").unwrap();
        cache.remove_legacy_proxies().unwrap();
        cache.remove_legacy_proxies().unwrap();
        assert!(cache.path(&"a".repeat(64), "jpg").unwrap().is_file());
        assert!(!cache.path(&"a".repeat(64), "mp4").unwrap().exists());
        assert!(!cache.path(&"a".repeat(64), "webm").unwrap().exists());
        assert!(dir.path().join("unrelated.mp4").is_file());
        assert!(dir.path().join("windows-encoder.txt").is_file());
    }
    #[test]
    fn partial_cleanup_backoff_is_bounded_and_windows_only() {
        for windows in [false, true] {
            let mut calls = 0;
            let mut delays = Vec::new();
            let result = retry_partial_cleanup(
                || {
                    calls += 1;
                    Err(std::io::Error::from_raw_os_error(32))
                },
                |delay| delays.push(delay),
                windows,
            );
            assert!(result.is_err());
            assert_eq!(calls, if windows { 4 } else { 1 });
            assert_eq!(delays.len(), if windows { 3 } else { 0 });
        }
        let mut calls = 0;
        retry_partial_cleanup(
            || {
                calls += 1;
                if calls == 1 {
                    Err(std::io::Error::from_raw_os_error(5))
                } else {
                    Ok(())
                }
            },
            |_| {},
            true,
        )
        .unwrap();
        assert_eq!(calls, 2);
        retry_partial_cleanup(
            || Err(std::io::ErrorKind::NotFound.into()),
            |_| panic!("missing file needs no delay"),
            true,
        )
        .unwrap();
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
            OpenOptions::new()
                .write(true)
                .open(&path)
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
