//! Ephemeral playback: one owned child, bounded RAM fragments, no proxy files.
use crate::{
    clips::Clip,
    fmp4::{Fragment, Init, Segment, Splitter},
    previews::{self, Ffmpeg, FfmpegProgress, PreviewCache, ProxyPlatform},
};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const AHEAD_SECONDS: f64 = 60.0;
pub const BEHIND_SECONDS: f64 = 30.0;
// Reserve the rest of the ~300 MB encoded-data budget for the client and one pipe box.
pub const RETAINED_BYTES: usize = 240_000_000;
pub const RETAINED_CLIPS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    Mac,
    Windows,
    Vp9,
    LinuxWebm,
    LinuxBlob,
}
impl Encoder {
    pub fn description(self) -> &'static str {
        match self {
            Self::Mac => "h264_videotoolbox / AAC · fragmented MP4",
            Self::Windows => "h264_mf / AAC · fragmented MP4",
            Self::Vp9 => "libvpx-vp9 / Opus · fragmented MP4",
            Self::LinuxWebm => "libvpx-vp9 / Vorbis · WebM (Linux MSE)",
            Self::LinuxBlob => "libvpx-vp9 / Opus · WebM (Linux compatibility)",
        }
    }
}
pub fn duration(clip: &Clip) -> f64 {
    clip.files
        .iter()
        .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
        .map(previews::seconds)
        .unwrap_or_else(|| clip.files.iter().map(previews::seconds).fold(0.0, f64::max))
}
pub fn stream_args(
    clip: &Clip,
    tracks: &[usize],
    encoder: Encoder,
    start: f64,
) -> Result<Vec<OsString>> {
    let total = duration(clip);
    ensure!(
        start.is_finite() && start >= 0.0 && start < total,
        "Invalid preview seek time"
    );
    let platform = match encoder {
        Encoder::Mac => ProxyPlatform::Mac,
        Encoder::Windows => ProxyPlatform::Windows,
        Encoder::Vp9 | Encoder::LinuxWebm | Encoder::LinuxBlob => ProxyPlatform::WindowsWebm,
    };
    // Share channel selection, TC alignment, alpha composition and scaling with posters/proxies.
    let mut original = previews::proxy_args(clip, tracks, platform, Path::new("pipe:1"))?;
    let (video_input, audio_inputs) = previews::proxy_inputs(clip, tracks)?;
    let origin = video_input
        .or_else(|| audio_inputs.first().copied())
        .and_then(previews::tc_seconds);
    let mut seeks = Vec::new();
    if video_input.is_some() {
        seeks.push(start);
    }
    for audio in &audio_inputs {
        let delta = previews::tc_seconds(audio)
            .zip(origin)
            .map(|(a, v)| a - v)
            .unwrap_or(0.0);
        let delta = if delta > 43200.0 {
            delta - 86400.0
        } else if delta < -43200.0 {
            delta + 86400.0
        } else {
            delta
        };
        seeks.push((start - delta).max(0.0));
        // Preserve TC sync after a seek: pre-seek past an offset, delay only a
        // track whose beginning is still ahead. Don't reapply a full clip delay.
        for argument in &mut original {
            if argument.to_string_lossy().contains("atrim=start=") {
                let prefix = format!("[{}:a:0]", seeks.len() - 1);
                *argument = argument
                    .to_string_lossy()
                    .split(';')
                    .map(|part| {
                        if !part.starts_with(&prefix) {
                            return part.to_string();
                        }
                        part.replace(
                            &format!("atrim=start={:.6}", (-delta).max(0.0)),
                            "atrim=start=0.000000",
                        )
                        .replace(
                            &format!("adelay={}:all=1", (delta.max(0.0) * 1000.0).round() as u64),
                            &format!(
                                "adelay={}:all=1",
                                ((delta - start).max(0.0) * 1000.0).round() as u64
                            ),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(";")
                    .into();
            }
        }
    }
    let mut input_index = 0;
    let mut args = Vec::new();
    let mut i = 0;
    while i < original.len() {
        let value = original[i].to_string_lossy();
        match value.as_ref() {
            "-i" => {
                args.extend([
                    "-ss".into(),
                    format!("{:.6}", seeks[input_index]).into(),
                    original[i].clone(),
                    original[i + 1].clone(),
                ]);
                input_index += 1;
                i += 2;
            }
            "-movflags" | "-f" => i += 2,
            "-t" => {
                args.extend(["-t".into(), format!("{:.6}", total - start).into()]);
                i += 2;
            }
            "libvpx" => {
                args.push("libvpx-vp9".into());
                i += 1;
            }
            "libopus" if encoder == Encoder::LinuxWebm => {
                args.push("libvorbis".into());
                i += 1;
            }
            "-cpu-used" => {
                args.extend(["-cpu-used".into(), "8".into()]);
                i += 2;
            }
            "pipe:1" => i += 1,
            _ => {
                args.push(original[i].clone());
                i += 1;
            }
        }
    }
    let video = clip
        .files
        .iter()
        .find(|f| f.metadata.track_kind.as_deref() == Some("video"));
    if let Some(video) = video {
        let fps = video
            .metadata
            .edit_rate
            .as_ref()
            .filter(|r| r.denominator > 0)
            .map(|r| f64::from(r.numerator) / f64::from(r.denominator))
            .unwrap_or(24.0);
        args.extend([
            "-g".into(),
            format!("{}", fps.round().max(1.0) as u32).into(),
            "-bf".into(),
            "0".into(),
        ]);
        if matches!(
            encoder,
            Encoder::Vp9 | Encoder::LinuxWebm | Encoder::LinuxBlob
        ) {
            args.extend(
                ["-lag-in-frames", "0", "-row-mt", "1", "-auto-alt-ref", "0"].map(Into::into),
            );
        }
        args.extend(["-maxrate", "2400k", "-bufsize", "2400k"].map(Into::into));
    } else if !matches!(encoder, Encoder::LinuxWebm | Encoder::LinuxBlob) {
        args.extend(["-frag_duration", "1000000"].map(Into::into));
    }
    if matches!(encoder, Encoder::LinuxWebm | Encoder::LinuxBlob) {
        args.extend(
            [
                "-map_metadata",
                "-1",
                "-cluster_time_limit",
                "1000",
                "-cluster_size_limit",
                "2000000",
                "-live",
                "1",
                "-avoid_negative_ts",
                "make_non_negative",
                "-output_ts_offset",
            ]
            .map(Into::into),
        );
        args.push(format!("{start:.6}").into());
        args.extend(["-flush_packets", "1", "-f", "webm", "pipe:1"].map(Into::into));
    } else {
        args.extend(
            [
                "-map_metadata",
                "-1",
                "-write_tmcd",
                "0",
                "-movflags",
                // frag_discont prevents movenc subtracting the seek offset from each tfdt.
                "frag_keyframe+empty_moov+default_base_moof+frag_discont",
                "-use_editlist",
                "0",
                "-avoid_negative_ts",
                "make_non_negative",
                "-output_ts_offset",
            ]
            .map(Into::into),
        );
        args.push(format!("{start:.6}").into());
        args.extend(["-flush_packets", "1", "-f", "mp4", "pipe:1"].map(Into::into));
    }
    Ok(args)
}

#[derive(Clone)]
pub struct Chunk {
    pub sequence: u64,
    pub fragment: Arc<Fragment>,
}
struct BufferedClip {
    key: String,
    clip_id: String,
    id: String,
    init: Option<Arc<Init>>,
    chunks: VecDeque<Chunk>,
    next: u64,
    complete: bool,
    blob: bool,
    encoder: Encoder,
}
impl BufferedClip {
    fn bytes(&self) -> usize {
        self.init.as_ref().map_or(0, |i| i.bytes.len())
            + self
                .chunks
                .iter()
                .map(|c| c.fragment.bytes.len())
                .sum::<usize>()
    }
}
pub struct MemoryStore {
    clips: VecDeque<BufferedClip>,
    cap: usize,
}
impl Default for MemoryStore {
    fn default() -> Self {
        Self::new(RETAINED_BYTES)
    }
}
impl MemoryStore {
    pub fn new(cap: usize) -> Self {
        Self {
            clips: VecDeque::new(),
            cap,
        }
    }
    pub fn bytes(&self) -> usize {
        self.clips.iter().map(BufferedClip::bytes).sum()
    }
    fn begin(&mut self, key: String, id: String) {
        self.clips.retain(|c| c.key != key);
        self.clips.push_back(BufferedClip {
            clip_id: key.clone(),
            key,
            id,
            init: None,
            chunks: VecDeque::new(),
            next: 0,
            complete: false,
            blob: false,
            encoder: Encoder::Vp9,
        });
        self.evict();
    }
    fn current(&self, id: &str) -> Option<&BufferedClip> {
        self.clips.back().filter(|c| c.id == id)
    }
    fn reuse(&mut self, key: &str, id: &str, start: f64) -> bool {
        let found = self.clips.iter().position(|c| {
            c.key == key
                && c.complete
                && c.init.is_some()
                && c.chunks
                    .iter()
                    .any(|c| c.fragment.start <= start + 0.05 && c.fragment.end > start)
        });
        if let Some(index) = found {
            let mut clip = self.clips.remove(index).unwrap();
            clip.id = id.into();
            self.clips.push_back(clip);
            true
        } else {
            false
        }
    }
    fn current_mut(&mut self, id: &str) -> Option<&mut BufferedClip> {
        self.clips.back_mut().filter(|c| c.id == id)
    }
    fn evict(&mut self) {
        while self.clips.len() > RETAINED_CLIPS || (self.bytes() > self.cap && self.clips.len() > 1)
        {
            self.clips.pop_front();
        }
        while self.bytes() > self.cap {
            let Some(last) = self.clips.back_mut() else {
                break;
            };
            if last.chunks.pop_front().is_none() {
                last.init = None;
                break;
            }
        }
    }
    fn trim(&mut self, id: &str, playhead: f64) {
        if let Some(c) = self.current_mut(id) {
            while c
                .chunks
                .front()
                .is_some_and(|c| c.fragment.end < playhead - BEHIND_SECONDS)
            {
                c.chunks.pop_front();
            }
        }
    }
}

#[derive(Clone, Serialize)]
pub struct StreamStatus {
    pub phase: &'static str,
    pub seconds: f64,
    pub duration: f64,
    pub input_bytes: u64,
    pub input_files: usize,
    pub output_bytes: u64,
    pub eta_seconds: Option<f64>,
    pub encoder: String,
    pub buffered_end: f64,
    pub memory_bytes: usize,
    pub backpressure: bool,
}
struct State {
    memory: MemoryStore,
    playhead: f64,
    end: f64,
    error: Option<String>,
    progress: FfmpegProgress,
    encoder: Encoder,
    output_bytes: u64,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Default)]
struct Process {
    child: Mutex<Option<Child>>,
}
impl Process {
    fn stop(&self) {
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut child) = child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn wait(&self) -> Result<bool> {
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        Ok(child
            .take()
            .context("Preview process stopped")?
            .wait()?
            .success())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}
pub struct Session {
    pub id: String,
    pub start: f64,
    pub duration: f64,
    pub first_sequence: u64,
    pub reused: bool,
    source: (u64, usize),
    started: Instant,
    remember_windows: bool,
    shared: Arc<Shared>,
    process: Process,
    cancelled: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}
pub enum ReadResult {
    Init(Arc<Init>, String),
    Blob(Vec<u8>, String, String),
    Media(Chunk),
    End,
    Gone,
    Pending,
    Failed(String),
}
impl Session {
    pub fn process_id(&self) -> Option<u32> {
        self.process
            .child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(Child::id)
    }
    pub fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        self.process.stop();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.stopped = true;
        self.shared.changed.notify_all();
    }
    pub fn tick(&self, playhead: f64) -> Result<StreamStatus> {
        ensure!(
            playhead.is_finite() && playhead >= 0.0 && playhead <= self.duration + 0.1,
            "Invalid preview playhead"
        );
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(
            !self.cancelled.load(Ordering::SeqCst) && state.memory.current(&self.id).is_some(),
            "Preview session closed"
        );
        state.playhead = playhead;
        state.memory.trim(&self.id, playhead);
        self.shared.changed.notify_all();
        let p = &state.progress;
        let seconds = (p.seconds - self.start).max(0.0);
        let speed = p
            .speed
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| seconds / self.started.elapsed().as_secs_f64().max(0.001));
        let complete = state.memory.current(&self.id).is_some_and(|c| c.complete);
        Ok(StreamStatus {
            phase: if complete {
                "ready"
            } else if state.end > self.start {
                "encoding"
            } else {
                "reading"
            },
            seconds,
            duration: self.duration,
            input_bytes: self.source.0,
            input_files: self.source.1,
            output_bytes: state.output_bytes,
            eta_seconds: (speed > 0.0 && !complete)
                .then(|| ((self.duration - self.start - seconds) / speed).max(0.0)),
            encoder: state.encoder.description().into(),
            buffered_end: state.end,
            memory_bytes: state.memory.bytes(),
            backpressure: !can_read(state.end, playhead),
        })
    }
    pub fn read(&self, sequence: Option<u64>, wait: Duration) -> ReadResult {
        let deadline = Instant::now() + wait;
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if self.cancelled.load(Ordering::SeqCst) || state.stopped {
                return ReadResult::Gone;
            }
            if let Some(error) = &state.error {
                return ReadResult::Failed(error.clone());
            }
            let Some(clip) = state.memory.current(&self.id) else {
                return ReadResult::Gone;
            };
            if let Some(sequence) = sequence {
                if let Some(chunk) = clip.chunks.iter().find(|c| c.sequence == sequence) {
                    return ReadResult::Media(chunk.clone());
                }
                if sequence < clip.next {
                    return ReadResult::Gone;
                }
                if clip.complete {
                    return ReadResult::End;
                }
            } else if clip.blob && clip.complete {
                if let Some(init) = &clip.init {
                    let mut bytes = init.bytes.clone();
                    for chunk in &clip.chunks {
                        bytes.extend_from_slice(&chunk.fragment.bytes);
                    }
                    return ReadResult::Blob(
                        bytes,
                        init.mime.clone(),
                        state.encoder.description().into(),
                    );
                }
            } else if !clip.blob && !clip.chunks.is_empty() {
                if let Some(init) = &clip.init {
                    return ReadResult::Init(init.clone(), state.encoder.description().into());
                }
            }
            if Instant::now() >= deadline {
                return ReadResult::Pending;
            }
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|p| p.into_inner());
            state = next;
        }
    }
    fn run(
        self: &Arc<Self>,
        ffmpeg: &Ffmpeg,
        clip: &Clip,
        tracks: &[usize],
        mut encoder: Encoder,
        cache: &PreviewCache,
    ) {
        let result = loop {
            let result = self.encode(ffmpeg, clip, tracks, encoder, cache);
            if self.cancelled.load(Ordering::SeqCst) {
                return;
            }
            if let Err(error) = &result {
                let state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
                if encoder == Encoder::Windows
                    && state.end == self.start
                    && previews::mf_encoder_open_error(error)
                {
                    drop(state);
                    encoder = Encoder::Vp9;
                    let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
                    state.encoder = encoder;
                    if let Some(c) = state.memory.current_mut(&self.id) {
                        c.init = None;
                        c.encoder = encoder;
                    }
                    continue;
                }
            }
            break result;
        };
        if result.is_ok() && self.remember_windows {
            let profile = if encoder == Encoder::Windows {
                ProxyPlatform::Windows
            } else {
                ProxyPlatform::WindowsWebm
            };
            let _ = cache.remember_windows_choice(ffmpeg, profile);
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(error) = result {
            state.error = Some(error.to_string());
        } else if let Some(c) = state.memory.current_mut(&self.id) {
            if c.chunks.is_empty() {
                state.error = Some("No decodable media at this seek point".into());
            } else {
                c.complete = true;
            }
        }
        self.shared.changed.notify_all();
    }
    fn encode(
        self: &Arc<Self>,
        ffmpeg: &Ffmpeg,
        clip: &Clip,
        tracks: &[usize],
        encoder: Encoder,
        cache: &PreviewCache,
    ) -> Result<()> {
        let args = stream_args(clip, tracks, encoder, self.start)?;
        let mut command = Command::new(&ffmpeg.binary);
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-stats_period",
                "0.25",
                "-progress",
                "pipe:2",
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
        let (stdout, stderr) = {
            let mut owner = self.process.child.lock().unwrap_or_else(|p| p.into_inner());
            ensure!(!self.cancelled.load(Ordering::SeqCst), "Preview stopped");
            let mut child = command
                .spawn()
                .context("Could not start streaming ffmpeg")?;
            let stdout = child.stdout.take().context("missing preview pipe")?;
            let stderr = child.stderr.take().context("missing preview diagnostics")?;
            *owner = Some(child);
            (stdout, stderr)
        };
        let shared = self.shared.clone();
        let diagnostics = thread::spawn(move || {
            let mut parser = previews::ProgressParser::default();
            let mut errors = VecDeque::new();
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(progress) = parser.line(&line) {
                    shared
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .progress = progress;
                    shared.changed.notify_all();
                } else if !line.contains('=') {
                    errors.push_back(line);
                    if errors.len() > 20 {
                        errors.pop_front();
                    }
                }
            }
            errors.into_iter().collect::<Vec<_>>().join("\n")
        });
        let mut remembered = false;
        let result = (|| {
            let video = clip
                .files
                .iter()
                .any(|f| f.metadata.track_kind.as_deref() == Some("video"));
            let mut mp4;
            let mut webm;
            let mut next: Box<dyn FnMut() -> Result<Option<Segment>>> =
                if matches!(encoder, Encoder::LinuxWebm | Encoder::LinuxBlob) {
                    webm = crate::webm::Splitter::new(stdout, video, encoder == Encoder::LinuxBlob);
                    Box::new(move || webm.next_segment())
                } else {
                    mp4 = Splitter::new(stdout);
                    Box::new(move || mp4.next_segment())
                };
            loop {
                {
                    let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
                    while encoder != Encoder::LinuxBlob
                        && !can_read(state.end, state.playhead)
                        && !self.cancelled.load(Ordering::SeqCst)
                    {
                        state = self
                            .shared
                            .changed
                            .wait(state)
                            .unwrap_or_else(|p| p.into_inner());
                    }
                }
                ensure!(!self.cancelled.load(Ordering::SeqCst), "Preview stopped");
                let Some(segment) = next()? else {
                    break;
                };
                let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
                if let Segment::Media(fragment) = &segment {
                    state.end = fragment.end;
                    state.output_bytes += fragment.bytes.len() as u64;
                    ensure!(encoder != Encoder::LinuxBlob || state.output_bytes <= 40_000_000,
                        "Linux compatibility preview exceeds its 40 MB memory limit; use an MSE-capable webview");
                }
                if let Some(c) = state.memory.current_mut(&self.id) {
                    match segment {
                        Segment::Init(init) => c.init = Some(Arc::new(init)),
                        Segment::Media(fragment) => {
                            c.chunks.push_back(Chunk {
                                sequence: c.next,
                                fragment: Arc::new(fragment),
                            });
                            c.next += 1;
                        }
                    }
                }
                state.memory.evict();
                self.shared.changed.notify_all();
                let proven = state.output_bytes > 0;
                drop(state);
                if proven && self.remember_windows && !remembered {
                    remembered = true;
                    let profile = if encoder == Encoder::Windows {
                        ProxyPlatform::Windows
                    } else {
                        ProxyPlatform::WindowsWebm
                    };
                    let _ = cache.remember_windows_choice(ffmpeg, profile);
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.process.stop();
        }
        let success = if result.is_ok() {
            self.process.wait()
        } else {
            Ok(false)
        };
        let errors = diagnostics.join().unwrap_or_default();
        if !errors.is_empty() && (!success.as_ref().is_ok_and(|s| *s) || result.is_err()) {
            bail!("Streaming preview failed: {errors}");
        }
        result?;
        ensure!(success?, "Streaming encoder exited unsuccessfully");
        Ok(())
    }
}
pub fn can_read(encoded_end: f64, playhead: f64) -> bool {
    encoded_end < playhead + AHEAD_SECONDS
}

pub struct StartStream {
    pub id: String,
    pub clip: Clip,
    pub tracks: Vec<usize>,
    pub start: f64,
    pub encoder: Option<Encoder>,
}
pub struct Streaming {
    shared: Arc<Shared>,
    active: Mutex<Option<Arc<Session>>>,
    lifecycle: Mutex<()>,
    closed: AtomicBool,
}
impl Default for Streaming {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    memory: MemoryStore::default(),
                    playhead: 0.0,
                    end: 0.0,
                    error: None,
                    progress: FfmpegProgress::default(),
                    encoder: Encoder::Vp9,
                    output_bytes: 0,
                    stopped: false,
                }),
                changed: Condvar::new(),
            }),
            active: Mutex::new(None),
            lifecycle: Mutex::new(()),
            closed: AtomicBool::new(false),
        }
    }
}
impl Streaming {
    pub fn start(
        &self,
        ffmpeg: Ffmpeg,
        cache: PreviewCache,
        request: StartStream,
    ) -> Result<Arc<Session>> {
        let StartStream {
            id,
            clip,
            tracks,
            start,
            encoder: forced,
        } = request;
        ensure!(
            (8..=80).contains(&id.len())
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid preview session"
        );
        let total = duration(&clip);
        ensure!(
            start.is_finite() && start >= 0.0 && start < total,
            "Invalid preview seek time"
        );
        let source = previews::proxy_source_summary(&clip, &tracks)?;
        let encoder = forced.unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                Encoder::Mac
            } else if cfg!(windows) && cache.windows_choice(&ffmpeg) == ProxyPlatform::Windows {
                Encoder::Windows
            } else {
                Encoder::Vp9
            }
        });
        stream_args(&clip, &tracks, encoder, start)?;
        let transport = match encoder {
            Encoder::LinuxWebm => "webm-vorbis",
            Encoder::LinuxBlob => "blob-opus",
            _ => "fmp4",
        };
        let key = format!(
            "{}-{transport}",
            previews::proxy_key(&clip, &tracks, ProxyPlatform::current())?
        );
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(!self.closed.load(Ordering::SeqCst), "Preview window closed");
        self.stop_inner(None);
        let (reused, first_sequence) = {
            let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            s.memory.cap = if encoder == Encoder::LinuxBlob {
                160_000_000
            } else {
                RETAINED_BYTES
            };
            let reused = s.memory.reuse(&key, &id, start);
            if !reused {
                // Track/codec variants of one clip don't displace three other clips.
                s.memory.clips.retain(|c| c.clip_id != clip.id);
                s.memory.begin(key, id.clone());
                let c = s.memory.current_mut(&id).unwrap();
                c.blob = encoder == Encoder::LinuxBlob;
                c.encoder = encoder;
                c.clip_id = clip.id.clone();
            }
            s.memory.evict();
            let c = s.memory.current(&id).unwrap();
            let end = if reused {
                c.chunks.back().map_or(start, |c| c.fragment.end)
            } else {
                start
            };
            let first = c
                .chunks
                .iter()
                .find(|c| c.fragment.end > start)
                .map_or(0, |c| c.sequence);
            let actual_encoder = c.encoder;
            let output_bytes = c.chunks.iter().map(|c| c.fragment.bytes.len() as u64).sum();
            s.playhead = start;
            s.end = end;
            s.error = None;
            s.progress = FfmpegProgress::default();
            s.encoder = actual_encoder;
            s.output_bytes = output_bytes;
            s.stopped = false;
            (reused, first)
        };
        let session = Arc::new(Session {
            id: id.clone(),
            start,
            duration: total,
            first_sequence,
            reused,
            source,
            started: Instant::now(),
            remember_windows: cfg!(windows) && forced.is_none(),
            shared: self.shared.clone(),
            process: Process::default(),
            cancelled: AtomicBool::new(false),
            worker: Mutex::new(None),
        });
        *self.active.lock().unwrap_or_else(|p| p.into_inner()) = Some(session.clone());
        if !reused {
            let worker = session.clone();
            *session.worker.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(thread::spawn(move || {
                    worker.run(&ffmpeg, &clip, &tracks, encoder, &cache)
                }));
        }
        Ok(session)
    }
    pub fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|s| s.id == id)
            .cloned()
    }
    pub fn stop(&self, id: Option<&str>) {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        self.stop_inner(id);
    }
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.stop(None);
    }
    fn stop_inner(&self, id: Option<&str>) {
        let old = {
            let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
            if id.is_none_or(|id| active.as_ref().is_some_and(|s| s.id == id)) {
                active.take()
            } else {
                None
            }
        };
        if let Some(old) = old {
            old.stop();
        }
    }
}
impl Drop for Streaming {
    fn drop(&mut self) {
        self.stop(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(sequence: u64, start: f64, bytes: usize) -> Chunk {
        Chunk {
            sequence,
            fragment: Arc::new(Fragment {
                bytes: vec![0; bytes],
                start,
                end: start + 1.0,
            }),
        }
    }
    fn push(store: &mut MemoryStore, start: f64, bytes: usize) {
        let c = store.clips.back_mut().unwrap();
        c.chunks.push_back(chunk(c.next, start, bytes));
        c.next += 1;
        store.evict();
    }
    #[test]
    fn backpressure_follows_the_playhead_including_seek() {
        assert!(can_read(59.9, 0.0));
        assert!(!can_read(60.0, 0.0));
        assert!(can_read(60.0, 1.0));
        assert!(!can_read(180.0, 120.0));
        assert!(can_read(179.0, 120.0));
    }
    #[test]
    fn memory_evicts_oldest_clips_then_old_fragments_under_hard_cap() {
        let mut memory = MemoryStore::new(100);
        for i in 0..5 {
            memory.begin(format!("clip{i}"), format!("session{i}"));
            push(&mut memory, 0.0, 10);
        }
        assert_eq!(memory.clips.len(), 4);
        assert_eq!(memory.clips.front().unwrap().key, "clip1");
        push(&mut memory, 1.0, 80);
        assert!(memory.bytes() <= 100);
        assert_eq!(memory.clips.len(), 2);
        assert_eq!(memory.clips.front().unwrap().key, "clip3");
        push(&mut memory, 2.0, 80);
        assert!(memory.bytes() <= 100);
        assert_eq!(memory.clips.len(), 1);
        assert_eq!(memory.clips[0].chunks.front().unwrap().sequence, 2);
    }
    #[test]
    fn trims_only_more_than_thirty_seconds_behind_and_replaces_seek_generation() {
        let mut memory = MemoryStore::new(1000);
        memory.begin("clip".into(), "old".into());
        push(&mut memory, 0.0, 10);
        push(&mut memory, 20.0, 10);
        push(&mut memory, 40.0, 10);
        memory.trim("stale-session", 100.0);
        assert_eq!(memory.clips[0].chunks.len(), 3);
        memory.trim("old", 52.0);
        assert_eq!(memory.clips[0].chunks.front().unwrap().fragment.start, 40.0);
        memory.begin("clip".into(), "new".into());
        assert!(memory.current("old").is_none());
        assert!(memory.current("new").unwrap().chunks.is_empty());
    }
    #[test]
    fn completed_memory_hit_promotes_recency_but_incomplete_or_missing_ranges_restart() {
        let mut memory = MemoryStore::new(1000);
        memory.begin("clip".into(), "old".into());
        push(&mut memory, 30.0, 10);
        assert!(!memory.reuse("clip", "new", 30.5));
        let c = memory.clips.back_mut().unwrap();
        c.complete = true;
        c.init = Some(Arc::new(Init {
            bytes: vec![0],
            tracks: vec![],
            mime: "video/mp4".into(),
        }));
        assert!(!memory.reuse("clip", "new", 0.0));
        memory.begin("other".into(), "other-session".into());
        assert!(memory.reuse("clip", "new", 30.5));
        assert_eq!(memory.clips.back().unwrap().key, "clip");
        assert!(memory.current("old").is_none());
    }
    #[test]
    fn builders_preseek_each_input_and_keep_offsets_after_seek() {
        let mut files = Vec::new();
        for (i, delta) in [0, 25, -25].into_iter().enumerate() {
            files.push(crate::MediaFile {
                path: format!("/media/track-{i}.mxf").into(),
                folder: "/media".into(),
                size: 10,
                modified_ns: "1".into(),
                metadata: crate::MediaMetadata {
                    material_package_uid: Some("clip".into()),
                    file_package_uid: Some(format!("track-{i}")),
                    track_kind: Some(if i == 0 { "video" } else { "audio" }.into()),
                    duration: Some(250),
                    edit_rate: Some(crate::Rational {
                        numerator: 25,
                        denominator: 1,
                    }),
                    timecode_fps: Some(25),
                    start_timecode: Some(2500 + delta),
                    ..Default::default()
                },
            });
        }
        let clip = crate::clips::group(files).remove(0);
        for encoder in [Encoder::Mac, Encoder::Windows, Encoder::Vp9] {
            let args = stream_args(&clip, &[0, 1], encoder, 3.0).unwrap();
            let seeks: Vec<_> = args
                .windows(2)
                .filter(|a| a[0] == "-ss")
                .map(|a| a[1].to_string_lossy().into_owned())
                .collect();
            assert_eq!(seeks, ["3.000000", "2.000000", "4.000000"]);
            let graph = args
                .iter()
                .find(|a| a.to_string_lossy().contains("atrim="))
                .unwrap()
                .to_string_lossy();
            assert!(graph.contains("[1:a:0]atrim=start=0.000000"));
            assert!(!graph.contains("adelay=1000"));
            assert!(args
                .iter()
                .any(|a| a == "frag_keyframe+empty_moov+default_base_moof+frag_discont"));
            assert!(!args.iter().any(|a| a == "+faststart"));
            assert_eq!(args.last().unwrap(), "pipe:1");
        }
        assert!(stream_args(&clip, &[], Encoder::Vp9, f64::NAN).is_err());
        assert!(stream_args(&clip, &[], Encoder::Vp9, 10.0).is_err());
    }
    // Spawn this test binary as a real child without needing ffmpeg or shell scripts.
    #[test]
    #[ignore]
    fn child_wait_fixture() {
        thread::sleep(Duration::from_secs(30));
    }
    #[test]
    fn owned_process_is_killed_and_reaped_on_drop() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "streaming::tests::child_wait_fixture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        #[cfg(windows)]
        let handle = unsafe { OpenProcess(0x100000, 0, pid) };
        let process = Process {
            child: Mutex::new(Some(child)),
        };
        assert!(process
            .child
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none());
        let began = Instant::now();
        drop(process);
        assert!(began.elapsed() < Duration::from_secs(2));
        #[cfg(unix)]
        {
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        #[cfg(windows)]
        unsafe {
            assert!(!handle.is_null());
            assert_eq!(WaitForSingleObject(handle, 0), 0);
            CloseHandle(handle);
        }
    }
    #[cfg(windows)]
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, millis: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
}
