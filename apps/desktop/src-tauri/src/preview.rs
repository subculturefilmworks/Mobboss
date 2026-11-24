use crate::{
    service,
    shell::{err, AppState, Reply},
};
use mobboss_core::{
    previews::{self, Ffmpeg, PreviewCache},
    CancellationToken,
};
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf};
use tauri::{Emitter, Manager, State};

pub fn cache(app: &tauri::AppHandle) -> anyhow::Result<PreviewCache> {
    PreviewCache::open(&app.path().app_data_dir()?.join("previews"))
}
pub fn ffmpeg(app: &tauri::AppHandle) -> Option<Ffmpeg> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let mut bundled = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            bundled.push(parent.join(name));
        }
    }
    if let Ok(resources) = app.path().resource_dir() {
        bundled.push(resources.join(name));
    }
    if cfg!(debug_assertions) {
        let ext = if cfg!(windows) { ".exe" } else { "" };
        bundled.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("binaries/ffmpeg-{}{ext}", env!("MOBBOSS_TARGET"))),
        );
    }
    Ffmpeg::locate(&bundled)
}
#[derive(Serialize)]
pub struct PreviewStatus {
    available: bool,
    message: String,
    thumbnails: BTreeMap<String, PathBuf>,
    cache_bytes: u64,
}
#[tauri::command]
pub fn preview_status(app: tauri::AppHandle, state: State<AppState>) -> Reply<PreviewStatus> {
    let settings = state.settings.lock().map_err(err)?.clone();
    let files = service::files(&settings).map_err(err)?;
    let cache = cache(&app).map_err(err)?;
    let mut thumbnails = BTreeMap::new();
    for clip in mobboss_core::clips::group(files) {
        if let Some(file) = clip
            .files
            .iter()
            .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
        {
            if let Some(path) = cache
                .existing(&previews::cache_key(file), "jpg")
                .map_err(err)?
            {
                thumbnails.insert(clip.id, path);
            }
        }
    }
    let available = ffmpeg(&app).is_some();
    Ok(PreviewStatus {
        available,
        message: if available {
            "Previews ready".into()
        } else {
            "Previews unavailable · bundled ffmpeg was not found. Development builds can set MOBBOSS_FFMPEG.".into()
        },
        thumbnails,
        cache_bytes: cache.size().map_err(err)?,
    })
}
#[tauri::command]
pub fn cancel_thumbnails(state: State<AppState>) -> Reply<()> {
    if let Some(token) = state.thumbnails.lock().map_err(err)?.as_ref() {
        token.cancel();
    }
    Ok(())
}
#[tauri::command]
pub fn start_thumbnails(app: tauri::AppHandle, state: State<AppState>) -> Reply<()> {
    // Always acquire operation state before preview state.
    let operation = state.active.lock().map_err(err)?;
    if state
        .preview_clearing
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("Preview cache is being cleared".into());
    }
    if operation.is_some() {
        return Err("Wait for the current scan or operation".into());
    }
    let mut active = state.thumbnails.lock().map_err(err)?;
    if active.is_some() {
        return Ok(());
    }
    let ffmpeg = ffmpeg(&app).ok_or("Previews unavailable · ffmpeg is missing")?;
    let cache = cache(&app).map_err(err)?;
    let settings = state.settings.lock().map_err(err)?.clone();
    let clips = mobboss_core::clips::group(service::files(&settings).map_err(err)?);
    service::validate_destination(&cache.root.join("cache-check"), &settings.roots).map_err(err)?;
    let token = CancellationToken::default();
    *active = Some(token.clone());
    std::thread::spawn(move || {
        previews::thumbnails(&cache, &ffmpeg, &clips, &token, |p| {
            let _ = app.emit("thumbnail-progress", p);
        });
        let protected = app
            .state::<AppState>()
            .proxy
            .lock()
            .ok()
            .and_then(|p| p.playing.clone())
            .into_iter()
            .collect::<Vec<_>>();
        let _ = cache.prune(settings.preview_cache_bytes, &protected);
        if let Ok(mut active) = app.state::<AppState>().thumbnails.lock() {
            *active = None;
        }
        let _ = app.emit("thumbnails-finished", token.is_cancelled());
    });
    Ok(())
}

#[tauri::command]
pub fn thumbnail_data(
    clip_ids: Vec<String>,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Reply<BTreeMap<String, String>> {
    use base64::Engine;
    let settings = state.settings.lock().map_err(err)?.clone();
    let clips = mobboss_core::clips::group(service::files(&settings).map_err(err)?);
    let wanted: std::collections::BTreeSet<_> = clip_ids.into_iter().collect();
    let cache = cache(&app).map_err(err)?;
    let mut data = BTreeMap::new();
    for clip in clips.into_iter().filter(|c| wanted.contains(&c.id)) {
        if let Some(file) = clip
            .files
            .iter()
            .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
        {
            if let Some(path) = cache
                .existing(&previews::cache_key(file), "jpg")
                .map_err(err)?
            {
                if std::fs::metadata(&path).map_err(err)?.len() <= 256_000 {
                    let jpeg = std::fs::read(path).map_err(err)?;
                    if jpeg.starts_with(&[0xff, 0xd8]) {
                        data.insert(
                            clip.id,
                            format!(
                                "data:image/jpeg;base64,{}",
                                base64::engine::general_purpose::STANDARD.encode(jpeg)
                            ),
                        );
                    }
                }
            }
        }
    }
    Ok(data)
}

#[derive(Default)]
pub(crate) struct ProxyState {
    active: Option<(String, CancellationToken)>,
    pub(crate) playing: Option<PathBuf>,
    playing_request: Option<String>,
}
#[derive(Clone, Serialize)]
struct ProxyProgress {
    request_id: String,
    fraction: f64,
}
#[tauri::command]
pub fn cancel_proxy(request_id: Option<String>, state: State<AppState>) -> Reply<()> {
    let mut proxy = state.proxy.lock().map_err(err)?;
    if let Some((id, token)) = proxy.active.as_ref() {
        if request_id.as_ref().is_none_or(|r| r == id) {
            token.cancel();
        }
    }
    if request_id
        .as_ref()
        .is_some_and(|id| proxy.playing_request.as_ref() == Some(id))
    {
        proxy.playing = None;
        proxy.playing_request = None;
    }
    Ok(())
}
#[tauri::command]
pub async fn prepare_proxy(
    clip_id: String,
    tracks: Vec<usize>,
    request_id: String,
    app: tauri::AppHandle,
) -> Reply<PathBuf> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let token = CancellationToken::default();
        let (settings, clip, ffmpeg, cache) = {
            let operation = state.active.lock().map_err(err)?;
            if state
                .preview_clearing
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("Preview cache is being cleared".into());
            }
            if operation.is_some() {
                return Err("Wait for the current scan or operation".into());
            }
            let settings = state.settings.lock().map_err(err)?.clone();
            let clip = mobboss_core::clips::group(service::files(&settings).map_err(err)?)
                .into_iter()
                .find(|c| c.id == clip_id)
                .ok_or("Select an indexed clip")?;
            let ffmpeg = ffmpeg(&app).ok_or("Previews unavailable · ffmpeg is missing")?;
            let cache = cache(&app).map_err(err)?;
            service::validate_destination(&cache.root.join("cache-check"), &settings.roots)
                .map_err(err)?;
            cancel_thumbnails(state.clone())?;
            let mut proxy = state.proxy.lock().map_err(err)?;
            if let Some((_, old)) = proxy.active.take() {
                old.cancel();
            }
            proxy.active = Some((request_id.clone(), token.clone()));
            (settings, clip, ffmpeg, cache)
        };
        let duration = clip
            .files
            .iter()
            .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
            .or_else(|| clip.files.first())
            .map(previews::seconds)
            .unwrap_or(1.0)
            .max(0.001);
        let result = cache
            .proxy(
                &ffmpeg,
                &clip,
                &tracks,
                previews::ProxyPlatform::current(),
                &token,
                |seconds| {
                    let _ = app.emit(
                        "proxy-progress",
                        ProxyProgress {
                            request_id: request_id.clone(),
                            fraction: (seconds / duration).clamp(0.0, 1.0),
                        },
                    );
                },
            )
            .and_then(|path| {
                if std::fs::metadata(&path)?.len() > settings.preview_cache_bytes {
                    let _ = std::fs::remove_file(&path);
                    anyhow::bail!("Preview exceeds cache limit; increase it in Settings");
                }
                Ok(path)
            })
            .map_err(err);
        let protected = {
            let mut proxy = state.proxy.lock().map_err(err)?;
            if proxy
                .active
                .as_ref()
                .is_some_and(|(id, _)| id == &request_id)
            {
                proxy.active = None;
                if let Ok(path) = &result {
                    proxy.playing = Some(path.clone());
                    proxy.playing_request = Some(request_id.clone());
                }
            }
            proxy.playing.clone().into_iter().collect::<Vec<_>>()
        };
        let _ = cache.prune(settings.preview_cache_bytes, &protected);
        result
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub async fn clear_preview_cache(app: tauri::AppHandle) -> Reply<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let _operation = state.active.lock().map_err(err)?;
            if state
                .preview_clearing
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Err("Preview cache is already being cleared".into());
            }
            cancel_thumbnails(state.clone())?;
            cancel_proxy(None, state.clone())?;
        }
        struct Reset<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _reset = Reset(&state.preview_clearing);
        for _ in 0..100 {
            let idle = state.thumbnails.lock().map_err(err)?.is_none()
                && state.proxy.lock().map_err(err)?.active.is_none();
            if idle {
                state.proxy.lock().map_err(err)?.playing = None;
                return cache(&app).map_err(err)?.clear().map_err(err);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err("Preview workers are stopping; retry clearing the cache".into())
    })
    .await
    .map_err(err)?
}
