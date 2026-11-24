use crate::{
    service,
    shell::{err, AppState, Reply},
};
use mobboss_core::{
    previews::{self, Ffmpeg, PreviewCache},
    CancellationToken,
};
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
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
    #[serde(serialize_with = "mobboss_core::paths::serialize_map")]
    thumbnails: BTreeMap<String, PathBuf>,
    thumbnail_bytes: u64,
}
#[tauri::command]
pub fn preview_status(app: tauri::AppHandle, state: State<AppState>) -> Reply<PreviewStatus> {
    let settings = state.settings.lock().map_err(err)?.clone();
    let files = service::files(&settings).map_err(err)?;
    let cache = cache(&app).map_err(err)?;
    let mut thumbnails = BTreeMap::new();
    for clip in mobboss_core::clips::group(files) {
        if let Some(file) = previews::poster_source(&clip) {
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
        thumbnail_bytes: cache.size().map_err(err)?,
    })
}
#[derive(Default)]
pub(crate) struct ThumbnailState {
    active: Option<CancellationToken>,
    queue: Option<Arc<previews::ThumbnailQueue>>,
    visible: Vec<String>,
    display: Vec<String>,
}
#[tauri::command]
pub fn prioritize(visible: Vec<String>, display: Vec<String>, state: State<AppState>) -> Reply<()> {
    let mut thumbnails = state.thumbnails.lock().map_err(err)?;
    if let Some(queue) = &thumbnails.queue {
        queue.prioritize(&visible, &display);
    }
    thumbnails.visible = visible;
    thumbnails.display = display;
    Ok(())
}
#[tauri::command]
pub fn cancel_thumbnails(state: State<AppState>) -> Reply<()> {
    if let Some(token) = state.thumbnails.lock().map_err(err)?.active.as_ref() {
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
    if active.active.is_some() {
        return Ok(());
    }
    let ffmpeg = ffmpeg(&app).ok_or("Previews unavailable · ffmpeg is missing")?;
    let cache = cache(&app).map_err(err)?;
    let settings = state.settings.lock().map_err(err)?.clone();
    let clips = mobboss_core::clips::group(service::files(&settings).map_err(err)?);
    service::validate_destination(&cache.root.join("cache-check"), &settings.roots).map_err(err)?;
    let token = CancellationToken::default();
    let queue = Arc::new(previews::ThumbnailQueue::new(&clips));
    queue.prioritize(&active.visible, &active.display);
    active.active = Some(token.clone());
    active.queue = Some(queue.clone());
    std::thread::spawn(move || {
        previews::thumbnails(&cache, &ffmpeg, &queue, &token, |p| {
            let _ = app.emit("thumbnail-progress", p);
        });
        if let Ok(mut active) = app.state::<AppState>().thumbnails.lock() {
            active.active = None;
            active.queue = None;
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
        if let Some(file) = previews::poster_source(&clip) {
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

#[tauri::command]
pub async fn clear_thumbnails(app: tauri::AppHandle) -> Reply<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state
            .preview_clearing
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("Thumbnails are already being cleared".into());
        }
        struct Reset<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _reset = Reset(&state.preview_clearing);
        cancel_thumbnails(state.clone())?;
        for _ in 0..100 {
            if state.thumbnails.lock().map_err(err)?.active.is_none() {
                return cache(&app).map_err(err)?.clear().map_err(err);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err("Thumbnail workers are stopping; retry shortly".into())
    })
    .await
    .map_err(err)?
}
