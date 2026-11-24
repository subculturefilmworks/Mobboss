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
        if let Ok(mut active) = app.state::<AppState>().thumbnails.lock() {
            *active = None;
        }
        let _ = app.emit("thumbnails-finished", token.is_cancelled());
    });
    Ok(())
}
