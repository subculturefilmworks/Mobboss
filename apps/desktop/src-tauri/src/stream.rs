//! Memory-only transport. Requests identify an active session and sequence, never a path.
use crate::{
    preview, service,
    shell::{err, AppState, Reply},
};
use mobboss_core::streaming::{self, ReadResult};
use serde::Serialize;
use std::time::Duration;
use tauri::{Manager, State};

#[derive(Serialize)]
pub struct OpenedStream {
    session_id: String,
    duration: f64,
    start: f64,
    first_sequence: u64,
    reused: bool,
}
#[tauri::command]
pub async fn open_stream(
    clip_id: String,
    tracks: Vec<usize>,
    session_id: String,
    start: f64,
    transport: String,
    app: tauri::AppHandle,
) -> Reply<OpenedStream> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let operation = state.active.lock().map_err(err)?;
        if operation.is_some() {
            return Err("Wait for the current scan or operation".into());
        }
        let settings = state.settings.lock().map_err(err)?.clone();
        let clip = mobboss_core::clips::group(service::files(&settings).map_err(err)?)
            .into_iter()
            .find(|c| c.id == clip_id)
            .ok_or("Select an indexed clip")?;
        let ffmpeg = preview::ffmpeg(&app).ok_or("Previews unavailable · ffmpeg is missing")?;
        let cache = preview::cache(&app).map_err(err)?;
        preview::cancel_thumbnails(state.clone())?;
        let forced = match transport.as_str() {
            "fmp4" => None,
            "webm" if cfg!(target_os = "linux") => Some(streaming::Encoder::LinuxWebm),
            "blob" if cfg!(target_os = "linux") => Some(streaming::Encoder::LinuxBlob),
            _ => return Err("Unsupported preview transport".into()),
        };
        let session = state
            .streams
            .start(
                ffmpeg,
                cache,
                streaming::StartStream {
                    id: session_id,
                    clip,
                    tracks,
                    start,
                    encoder: forced,
                },
            )
            .map_err(err)?;
        Ok(OpenedStream {
            session_id: session.id.clone(),
            duration: session.duration,
            first_sequence: session.first_sequence,
            reused: session.reused,
            start,
        })
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub fn stream_tick(
    session_id: String,
    playhead: f64,
    state: State<AppState>,
) -> Reply<streaming::StreamStatus> {
    state
        .streams
        .session(&session_id)
        .ok_or("Preview session closed")?
        .tick(playhead)
        .map_err(err)
}
#[tauri::command]
pub async fn stop_stream(session_id: Option<String>, app: tauri::AppHandle) -> Reply<()> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>().streams.stop(session_id.as_deref())
    })
    .await
    .map_err(err)
}

pub fn protocol(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let allowed_window = ctx.webview_label() == "main";
    let origin = request
        .headers()
        .get("Origin")
        .and_then(|o| o.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let allowed_origin = matches!(
        origin.as_str(),
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) || (cfg!(debug_assertions)
        && matches!(
            origin.as_str(),
            "http://localhost:1420" | "http://127.0.0.1:1420"
        ));
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    std::thread::spawn(move || {
        let mut status = 404;
        let mut body = Vec::new();
        let mut mime = "application/octet-stream".to_string();
        let mut encoder = String::new();
        let mut mode = "mse";
        let mut start = String::new();
        let mut end = String::new();
        if !allowed_window || !allowed_origin || method != tauri::http::Method::GET {
            status = 403;
        } else if let Some((id, part)) = path.strip_prefix('/').and_then(|p| p.split_once('/')) {
            if let Some(session) = app.state::<AppState>().streams.session(id) {
                let sequence = if part == "init" {
                    Some(None)
                } else {
                    part.parse::<u64>().ok().map(Some)
                };
                if let Some(sequence) = sequence {
                    match session.read(sequence, Duration::from_secs(10)) {
                        ReadResult::Init(init, name) => {
                            status = 200;
                            body = init.bytes.clone();
                            mime = init.mime.clone();
                            encoder = name;
                        }
                        ReadResult::Media(chunk) => {
                            status = 200;
                            body = chunk.fragment.bytes.clone();
                            start = chunk.fragment.start.to_string();
                            end = chunk.fragment.end.to_string();
                        }
                        ReadResult::Blob(bytes, kind, name) => {
                            status = 200;
                            body = bytes;
                            mime = kind;
                            encoder = name;
                            mode = "blob";
                        }
                        ReadResult::End => status = 204,
                        ReadResult::Gone => status = 410,
                        ReadResult::Pending => status = 408,
                        ReadResult::Failed(error) => {
                            status = 500;
                            body = error.into_bytes();
                            mime = "text/plain".into();
                        }
                    }
                }
            }
        }
        responder.respond(
            tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", mime)
                .header("Cache-Control", "no-store")
                .header("X-Content-Type-Options", "nosniff")
                .header(
                    "Access-Control-Allow-Origin",
                    if allowed_origin {
                        origin.as_str()
                    } else {
                        "null"
                    },
                )
                .header(
                    "Access-Control-Expose-Headers",
                    "Content-Type, X-Preview-Encoder, X-Preview-Start, X-Preview-End, X-Preview-Mode",
                )
                .header("X-Preview-Encoder", encoder.replace(" · ", " / "))
                .header("X-Preview-Mode", mode)
            .header("X-Preview-Start", start)
                .header("X-Preview-End", end)
                .body(body)
                .unwrap(),
        );
    });
}
