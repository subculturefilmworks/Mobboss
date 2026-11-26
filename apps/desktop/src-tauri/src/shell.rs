use crate::service::{self, Settings};
use mobboss_core::{CancellationToken, ScanMode, ScanOptions};
use std::{path::PathBuf, sync::Mutex};
use tauri::{Emitter, Manager, State};
pub(crate) struct AppState {
    pub(crate) settings: Mutex<Settings>,
    pub(crate) active: Mutex<Option<CancellationToken>>,
    pub(crate) preview_clearing: std::sync::atomic::AtomicBool,
    pub(crate) streams: mobboss_core::streaming::Streaming,
    pub(crate) thumbnails: Mutex<crate::preview::ThumbnailState>,
    planned: Mutex<Option<mobboss_core::plan::Plan>>,
    undo_log: Mutex<Option<PathBuf>>,
}
pub(crate) type Reply<T> = Result<T, String>;
pub(crate) fn err(e: impl std::fmt::Display) -> String {
    mobboss_core::paths::message(&e.to_string())
}
#[tauri::command]
fn settings(state: State<AppState>) -> Reply<Settings> {
    Ok(state.settings.lock().map_err(err)?.clone())
}
#[tauri::command]
async fn save_settings(mut value: Settings, app: tauri::AppHandle) -> Reply<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let active = state.active.lock().map_err(err)?;
        if active.is_some() {
            return Err("Wait for the current scan before changing settings".into());
        }
        value.index = mobboss_core::validate_destination(&value.index).map_err(err)?;
        value.roots = value
            .mounted_sources
            .iter()
            .chain(&value.custom_folders)
            .map(|row| row.path.clone())
            .collect();
        value.save().map_err(err)?;
        *state.settings.lock().map_err(err)? = value.clone();
        Ok(value)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
async fn files(app: tauri::AppHandle) -> Reply<Vec<mobboss_core::MediaFile>> {
    let settings = app
        .state::<AppState>()
        .settings
        .lock()
        .map_err(err)?
        .clone();
    tauri::async_runtime::spawn_blocking(move || service::files(&settings).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
fn missing_files(
    paths: Vec<PathBuf>,
    state: State<AppState>,
) -> Reply<Vec<mobboss_core::MediaFile>> {
    service::missing_files(&*state.settings.lock().map_err(err)?, &paths).map_err(err)
}
#[tauri::command]
async fn discover_sources(
    app: tauri::AppHandle,
    refresh: Option<bool>,
) -> Reply<crate::discovery::Discovery> {
    let settings = app
        .state::<AppState>()
        .settings
        .lock()
        .map_err(err)?
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::discovery::discover(settings.roots, refresh.unwrap_or(false))
    })
    .await
    .map_err(err)
}
#[tauri::command]
fn open_volume_permissions() -> Reply<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")
            .status()
            .map_err(err)?;
        if !status.success() {
            return Err("System Settings could not be opened".into());
        }
    }
    Ok(())
}
#[tauri::command]
fn cancel(state: State<AppState>) -> Reply<()> {
    if let Some(token) = state.active.lock().map_err(err)?.as_ref() {
        token.cancel();
    }
    Ok(())
}
#[tauri::command]
fn start_scan(mode: ScanMode, app: tauri::AppHandle, state: State<AppState>) -> Reply<()> {
    let mut active = state.active.lock().map_err(err)?;
    if active.is_some() {
        return Err("A scan is already running".into());
    }
    crate::preview::cancel_thumbnails(state.clone())?;
    state.streams.stop(None);
    let mut settings = state.settings.lock().map_err(err)?.clone();
    settings.roots = settings.checked_roots();
    let token = CancellationToken::default();
    *active = Some(token.clone());
    std::thread::spawn(move || {
        let result = service::scan(
            &settings,
            &ScanOptions {
                mode,
                concurrency: settings.concurrency,
            },
            &token,
            |progress| {
                let _ = app.emit("scan-progress", progress);
            },
        );
        let result = result.map_err(err);
        if let Ok(mut active) = app.state::<AppState>().active.lock() {
            *active = None;
        }
        let _ = app.emit("scan-finished", &result);
        if result.is_ok() && !token.is_cancelled() {
            let _ = crate::preview::start_thumbnails(app.clone(), app.state::<AppState>());
        }
    });
    Ok(())
}
#[tauri::command]
async fn plan_operation(
    op: mobboss_core::plan::Operation,
    project: String,
    paths: Vec<PathBuf>,
    destination: Option<PathBuf>,
    app: tauri::AppHandle,
) -> Reply<mobboss_core::plan::Plan> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let active = state.active.lock().map_err(err)?;
        if active.is_some() {
            return Err("Wait for the current operation".into());
        }
        let settings = state.settings.lock().map_err(err)?.clone();
        let index = mobboss_core::Index::open(&settings.index).map_err(err)?;
        let plan = mobboss_core::ops::prepare(&index, op, &project, &paths, destination.as_deref())
            .map_err(err)?;
        *state.planned.lock().map_err(err)? = Some(plan.clone());
        Ok(plan)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
async fn plan_undo(path: PathBuf, app: tauri::AppHandle) -> Reply<mobboss_core::ops::UndoPreview> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let active = state.active.lock().map_err(err)?;
        if active.is_some() {
            return Err("Wait for the current operation".into());
        }
        let settings = state.settings.lock().map_err(err)?.clone();
        let index = mobboss_core::Index::open(&settings.index).map_err(err)?;
        let preview = mobboss_core::ops::preview_undo(&index, &path).map_err(err)?;
        *state.undo_log.lock().map_err(err)? = Some(path);
        Ok(preview)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
fn start_operation(
    confirmed: bool,
    undo: bool,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Reply<()> {
    if !confirmed {
        return Err("Explicit confirmation required".into());
    }
    let mut active = state.active.lock().map_err(err)?;
    if active.is_some() {
        return Err("An operation is already running".into());
    }
    let settings = state.settings.lock().map_err(err)?.clone();
    let plan = if undo {
        None
    } else {
        Some(
            state
                .planned
                .lock()
                .map_err(err)?
                .take()
                .ok_or("Preview a plan first")?,
        )
    };
    let log = if undo {
        Some(
            state
                .undo_log
                .lock()
                .map_err(err)?
                .take()
                .ok_or("Preview an undo log first")?,
        )
    } else {
        None
    };
    crate::preview::cancel_thumbnails(state.clone())?;
    state.streams.stop(None);
    let token = CancellationToken::default();
    *active = Some(token.clone());
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<mobboss_core::ops::Outcome> {
            let mut index = mobboss_core::Index::open(&settings.index)?;
            let data = mobboss_core::default_index()?
                .parent()
                .unwrap()
                .to_path_buf();
            let progress = |p| {
                let _ = app.emit("operation-progress", p);
            };
            if let Some(log) = log {
                mobboss_core::ops::undo(&mut index, &log, &data, &token, progress)
            } else {
                mobboss_core::ops::execute(
                    &mut index,
                    plan.as_ref().unwrap(),
                    &data,
                    &token,
                    progress,
                )
            }
        })()
        .map_err(err);
        if let Ok(mut active) = app.state::<AppState>().active.lock() {
            *active = None;
        }
        let _ = app.emit("operation-finished", &result);
    });
    Ok(())
}
#[tauri::command]
fn reveal_log(path: PathBuf) -> Reply<()> {
    if !path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("collect-log-"))
        || path.extension().is_none_or(|e| e != "csv")
    {
        return Err("Choose an operation log".into());
    }
    mobboss_core::ops::read_log(&path).map_err(err)?;
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe")
        .arg(path.parent().ok_or("no log parent")?)
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().ok_or("no log parent")?)
        .status();
    result.map_err(err)?;
    Ok(())
}
#[tauri::command]
fn export_report(path: PathBuf, content: String, state: State<AppState>) -> Reply<()> {
    let settings = state.settings.lock().map_err(err)?;
    service::export(&path, &content, &settings.roots, &settings.index).map_err(err)
}
#[tauri::command]
fn reveal(path: PathBuf, state: State<AppState>) -> Reply<()> {
    service::reveal(&path, &*state.settings.lock().map_err(err)?).map_err(err)
}
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("preview", crate::stream::protocol)
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                let state = window.state::<AppState>();
                state.streams.shutdown();
                let _ = crate::preview::cancel_thumbnails(state);
            }
        })
        .setup(|app| {
            let cache = crate::preview::cache(app.handle())?;
            cache.remove_legacy_proxies()?;
            app.asset_protocol_scope()
                .allow_directory(&cache.root, true)?;
            app.manage(AppState {
                settings: Mutex::new(Settings::load()?),
                active: Mutex::new(None),
                preview_clearing: std::sync::atomic::AtomicBool::new(false),
                streams: mobboss_core::streaming::Streaming::default(),
                thumbnails: Mutex::new(crate::preview::ThumbnailState::default()),
                planned: Mutex::new(None),
                undo_log: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::dialog::pick_path,
            crate::preview::preview_status,
            crate::preview::thumbnail_data,
            crate::stream::open_stream,
            crate::stream::stream_tick,
            crate::stream::stop_stream,
            crate::preview::clear_thumbnails,
            crate::preview::start_thumbnails,
            crate::preview::prioritize,
            crate::preview::cancel_thumbnails,
            settings,
            save_settings,
            files,
            missing_files,
            discover_sources,
            open_volume_permissions,
            start_scan,
            plan_operation,
            plan_undo,
            start_operation,
            reveal_log,
            cancel,
            export_report,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("Mob Boss could not start");
}
