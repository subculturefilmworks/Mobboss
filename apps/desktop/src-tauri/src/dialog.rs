use crate::shell::{err, Reply};
use serde::Deserialize;
use tauri_plugin_dialog::DialogExt;
#[derive(Deserialize)]
pub struct Filter {
    name: String,
    extensions: Vec<String>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    #[serde(default)]
    directory: bool,
    #[serde(default)]
    multiple: bool,
    title: Option<String>,
    default_path: Option<String>,
    #[serde(default)]
    filters: Vec<Filter>,
}
/// Use the native Rust API, whose selections do not grant asset-protocol access.
#[tauri::command]
pub async fn pick_path(
    options: Options,
    save: bool,
    window: tauri::WebviewWindow,
) -> Reply<Option<Vec<std::path::PathBuf>>> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = window.dialog().file().set_parent(&window);
        if let Some(title) = options.title {
            dialog = dialog.set_title(title);
        }
        if let Some(name) = options.default_path {
            dialog = dialog.set_file_name(name);
        }
        for filter in options.filters {
            let ext: Vec<_> = filter.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(filter.name, &ext);
        }
        let paths = if save {
            dialog.blocking_save_file().map(|p| vec![p])
        } else if options.directory {
            if options.multiple {
                dialog.blocking_pick_folders()
            } else {
                dialog.blocking_pick_folder().map(|p| vec![p])
            }
        } else if options.multiple {
            dialog.blocking_pick_files()
        } else {
            dialog.blocking_pick_file().map(|p| vec![p])
        };
        paths
            .map(|paths| {
                paths
                    .into_iter()
                    .map(|p| p.into_path().map_err(err))
                    .collect()
            })
            .transpose()
    })
    .await
    .map_err(err)?
}
