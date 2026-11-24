use crate::service::{self, Settings};
use mobboss_core::{CancellationToken, ScanMode, ScanOptions};
use std::{path::PathBuf, sync::Mutex};
use tauri::{Emitter, Manager, State};
struct AppState {
    settings: Mutex<Settings>,
    active: Mutex<Option<CancellationToken>>,
}
type Reply<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[tauri::command]
fn settings(state: State<AppState>) -> Reply<Settings> {
    Ok(state.settings.lock().map_err(err)?.clone())
}
#[tauri::command]
fn save_settings(mut value: Settings, state: State<AppState>) -> Reply<Settings> {
    let active = state.active.lock().map_err(err)?;
    if active.is_some() {
        return Err("Wait for the current scan before changing settings".into());
    }
    value.index = mobboss_core::validate_destination(&value.index).map_err(err)?;
    value.roots = value
        .roots
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    value.save().map_err(err)?;
    *state.settings.lock().map_err(err)? = value.clone();
    Ok(value)
}
#[tauri::command]
fn files(state: State<AppState>) -> Reply<Vec<mobboss_core::MediaFile>> {
    service::files(&*state.settings.lock().map_err(err)?).map_err(err)
}
#[tauri::command]
fn missing_files(
    paths: Vec<PathBuf>,
    state: State<AppState>,
) -> Reply<Vec<mobboss_core::MediaFile>> {
    service::missing_files(&*state.settings.lock().map_err(err)?, &paths).map_err(err)
}
#[tauri::command]
fn volumes() -> Vec<service::Volume> {
    service::volumes()
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
    let settings = state.settings.lock().map_err(err)?.clone();
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
    });
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
        .setup(|app| {
            app.manage(AppState {
                settings: Mutex::new(Settings::load()?),
                active: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings,
            save_settings,
            files,
            missing_files,
            volumes,
            start_scan,
            cancel,
            export_report,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("Mob Boss could not start");
}
