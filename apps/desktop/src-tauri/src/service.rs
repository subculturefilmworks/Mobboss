use anyhow::{ensure, Context, Result};
use mobboss_core::{
    default_index, CancellationToken, Index, MediaFile, ScanOptions, ScanProgress, ScanReport,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRow {
    #[serde(serialize_with = "mobboss_core::paths::serialize")]
    pub path: PathBuf,
    pub checked: bool,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub volume: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "SettingsData")]
pub struct Settings {
    #[serde(serialize_with = "mobboss_core::paths::serialize")]
    pub index: PathBuf,
    #[serde(serialize_with = "mobboss_core::paths::serialize_many")]
    pub roots: Vec<PathBuf>,
    pub concurrency: usize,
    pub mounted_sources: Vec<SourceRow>,
    pub mounted_checks: std::collections::BTreeMap<String, bool>,
    pub custom_folders: Vec<SourceRow>,
}
#[derive(Deserialize)]
struct SettingsData {
    index: PathBuf,
    roots: Vec<PathBuf>,
    concurrency: usize,
    #[serde(default)]
    mounted_sources: Vec<SourceRow>,
    #[serde(default)]
    mounted_checks: std::collections::BTreeMap<String, bool>,
    custom_folders: Option<Vec<SourceRow>>,
}
impl From<SettingsData> for Settings {
    fn from(data: SettingsData) -> Self {
        let custom_folders = data.custom_folders.unwrap_or_else(|| {
            data.roots
                .iter()
                .map(|path| SourceRow {
                    path: path.clone(),
                    checked: true,
                    offline: false,
                    volume: String::new(),
                })
                .collect()
        });
        Self {
            index: data.index,
            roots: data.roots,
            concurrency: data.concurrency,
            mounted_sources: data.mounted_sources,
            mounted_checks: data.mounted_checks,
            custom_folders,
        }
    }
}
impl Settings {
    pub fn checked_roots(&self) -> Vec<PathBuf> {
        let roots: Vec<_> = self
            .mounted_sources
            .iter()
            .chain(&self.custom_folders)
            .filter(|row| row.checked && !row.offline)
            .map(|row| row.path.clone())
            .collect();
        roots
            .iter()
            .enumerate()
            .filter(|(i, root)| {
                !roots.iter().enumerate().any(|(j, parent)| {
                    *i != j
                        && mobboss_core::paths::contains(root, parent)
                        && (!mobboss_core::paths::same(root, parent) || j < *i)
                })
            })
            .map(|(_, root)| root.clone())
            .collect()
    }
    pub fn load() -> Result<Self> {
        let index = default_index()?;
        let config = index.with_file_name("settings.json");
        if config.exists() {
            let mut settings: Self = serde_json::from_slice(&fs::read(config)?)?;
            settings.index = mobboss_core::paths::canonical(&settings.index)?;
            return Ok(settings);
        }
        Ok(Self {
            index,
            roots: vec![],
            concurrency: 8,
            mounted_sources: vec![],
            mounted_checks: Default::default(),
            custom_folders: vec![],
        })
    }
    pub fn save(&self) -> Result<()> {
        validate_destination(&self.index, &self.roots)?;
        ensure!(
            (1..=64).contains(&self.concurrency),
            "concurrency must be 1–64"
        );
        let config = default_index()?.with_file_name("settings.json");
        fs::create_dir_all(config.parent().context("invalid settings path")?)?;
        fs::write(config, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}
/// Reports and indexes must live outside media trees, including symlink aliases.
pub fn validate_destination(path: &Path, roots: &[PathBuf]) -> Result<()> {
    let dest = mobboss_core::validate_destination(path)?;
    for root in roots {
        ensure!(root.is_absolute(), "source paths must be absolute");
        // Disconnected shares should not prevent opening the existing library.
        let path = root.clone();
        let root = crate::discovery::bounded(format!("canonical:{}", path.display()), move || {
            mobboss_core::paths::canonical(&path)
        })
        .and_then(Result::ok)
        .unwrap_or_else(|| root.clone());
        ensure!(
            !mobboss_core::paths::contains(&dest, &root) || root.parent().is_none(),
            "destination must be outside scanned sources"
        );
    }
    Ok(())
}
pub fn files(settings: &Settings) -> Result<Vec<MediaFile>> {
    validate_destination(&settings.index, &settings.roots)?;
    Index::open(&settings.index)?.files(None)
}
pub fn missing_files(settings: &Settings, paths: &[PathBuf]) -> Result<Vec<MediaFile>> {
    validate_destination(&settings.index, &settings.roots)?;
    let paths: std::collections::BTreeSet<_> = paths.iter().collect();
    Ok(Index::open(&settings.index)?
        .files_including_absent()?
        .into_iter()
        .filter(|f| paths.contains(&f.path))
        .collect())
}
pub fn scan(
    settings: &Settings,
    options: &ScanOptions,
    token: &CancellationToken,
    mut progress: impl FnMut(ScanProgress),
) -> Result<Vec<ScanReport>> {
    ensure!(!settings.roots.is_empty(), "choose at least one source");
    validate_destination(&settings.index, &settings.roots)?;
    let mut index = Index::open(&settings.index)?;
    let mut reports = vec![];
    // Roots are deduplicated, and children covered by another root are omitted.
    let roots: Vec<_> = settings
        .roots
        .iter()
        .filter_map(|p| {
            let path = p.clone();
            crate::discovery::bounded(format!("scan-root:{}", path.display()), move || {
                if !crate::discovery::readable_root(&path) {
                    return None;
                }
                mobboss_core::paths::selected(&path).ok()
            })
            .flatten()
        })
        .collect();
    for (i, root) in roots.iter().enumerate() {
        if roots.iter().enumerate().any(|(j, parent)| {
            i != j
                && mobboss_core::paths::contains(root, parent)
                && (!mobboss_core::paths::same(root, parent) || j < i)
        }) {
            continue;
        }
        reports.push(index.scan_with(root, options, token, &mut progress)?);
    }
    Ok(reports)
}
#[derive(Clone, Serialize)]
pub struct Volume {
    pub name: String,
    #[serde(serialize_with = "mobboss_core::paths::serialize")]
    pub path: PathBuf,
    pub available: u64,
    pub total: u64,
}
pub fn volumes() -> Vec<Volume> {
    let volumes: Vec<_> = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| {
            let mount = d.mount_point().to_string_lossy();
            let name = d.name().to_string_lossy();
            let filesystem = d.file_system().to_string_lossy();
            !["/proc", "/sys", "/run", "/snap", "/dev"].iter().any(|p| {
                (mount == *p || mount.starts_with(&format!("{p}/")))
                    && !mount.starts_with("/run/media/")
            }) && !["systemd-1", "tmpfs", "overlay"].contains(&name.as_ref())
                && !name.starts_with("/dev/mapper")
                && !["tmpfs", "overlay", "squashfs", "proc", "sysfs", "devtmpfs"]
                    .contains(&filesystem.as_ref())
        })
        .map(|d| Volume {
            name: if cfg!(target_os = "windows") {
                format!(
                    "{} {}",
                    mobboss_core::paths::display(d.mount_point()).display(),
                    d.name().to_string_lossy()
                )
            } else if d.mount_point().starts_with("/Volumes") {
                d.mount_point()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                d.mount_point()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "System volume".into())
            },
            path: d.mount_point().into(),
            available: d.available_space(),
            total: d.total_space(),
        })
        .collect();
    volumes
}
pub fn supplemental_volumes() -> Vec<Volume> {
    let mut volumes = vec![];
    for path in extra_mounts() {
        volumes.push(Volume {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path,
            available: 0,
            total: 0,
        });
    }
    volumes
}
fn extra_mounts() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // GetLogicalDrives includes mapped SMB/NEXIS drive letters. Do not ask
        // for capacity here: a disconnected remote drive can stall that call.
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let mask = unsafe { GetLogicalDrives() };
        (0..26)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        std::iter::once(PathBuf::from("/"))
            .chain(
                fs::read_dir("/Volumes")
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok().map(|e| e.path())),
            )
            .collect()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // The mount table lists actual mount points, including per-user mounts,
        // without traversing arbitrary directory trees or contacting shares.
        fs::read_to_string("/proc/self/mounts")
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let device = fields.next()?;
                if device.starts_with("/dev/mapper")
                    || ["systemd-1", "tmpfs", "overlay"].contains(&device)
                {
                    return None;
                }
                let path = fields.next()?;
                let filesystem = fields.next()?;
                if ["tmpfs", "overlay", "squashfs", "proc", "sysfs", "devtmpfs"]
                    .contains(&filesystem)
                {
                    return None;
                }
                let path = path
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\");
                ["/", "/media", "/mnt", "/run/media"]
                    .iter()
                    .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
                    .then(|| PathBuf::from(path))
            })
            .collect()
    }
}
pub fn export(path: &Path, content: &str, roots: &[PathBuf], index: &Path) -> Result<()> {
    validate_destination(path, roots)?;
    ensure!(
        path.extension().is_some_and(|e| e == "csv" || e == "html"),
        "export must be CSV or HTML"
    );
    ensure!(
        mobboss_core::validate_destination(path)? != mobboss_core::validate_destination(index)?,
        "cannot overwrite the index"
    );
    fs::write(path, content)?;
    Ok(())
}
pub fn reveal(path: &Path, settings: &Settings) -> Result<()> {
    let path = mobboss_core::paths::canonical(path)?;
    ensure!(
        files(settings)?
            .iter()
            .any(|f| mobboss_core::paths::same(&f.path, &path)),
        "select an indexed file"
    );
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg("-R").arg(&path).status()?;
    #[cfg(target_os = "windows")]
    let shell_path = {
        let p = path.to_string_lossy();
        if let Some(unc) = p.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            p.strip_prefix(r"\\?\").unwrap_or(&p).to_owned()
        }
    };
    #[cfg(target_os = "windows")]
    let status = Command::new("explorer.exe")
        .arg(format!("/select,{shell_path}"))
        .status()?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = Command::new("xdg-open")
        .arg(path.parent().context("no parent")?)
        .status()?;
    ensure!(status.success(), "file manager failed");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_folder_scan_is_recursive_and_does_not_follow_symlinks() {
        let dir =
            std::env::temp_dir().join(format!("mobboss-custom-recursive-{}", std::process::id()));
        let root = dir.join("Project-Island");
        fs::create_dir_all(root.join("Backup/Media/Nested")).unwrap();
        fs::create_dir_all(dir.join("Outside-Island")).unwrap();
        let header = include_bytes!("../../../../tests/fixtures/video.header");
        for path in [
            "direct.mxf",
            "Backup/child.mxf",
            "Backup/Media/Nested/deep.mxf",
        ] {
            fs::write(root.join(path), header).unwrap();
        }
        fs::write(dir.join("Outside-Island/external.mxf"), header).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("Outside-Island"), root.join("linked-folder"))
                .unwrap();
            std::os::unix::fs::symlink(
                dir.join("Outside-Island/external.mxf"),
                root.join("linked-file.mxf"),
            )
            .unwrap();
        }
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "index": dir.join("catalog.sqlite"), "roots": [], "concurrency": 2,
            "custom_folders": [{"path": root, "checked": true}]
        }))
        .unwrap();
        settings.roots = settings.checked_roots();
        for mode in [
            mobboss_core::ScanMode::Quick,
            mobboss_core::ScanMode::Auto,
            mobboss_core::ScanMode::Deep,
        ] {
            let reports = scan(
                &settings,
                &ScanOptions {
                    mode,
                    concurrency: 2,
                },
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();
            assert_eq!(reports[0].files, 3);
            let indexed = files(&settings).unwrap();
            assert_eq!(indexed.len(), 3);
            assert!(indexed
                .iter()
                .any(|file| file.path.ends_with("Backup/Media/Nested/deep.mxf")));
            assert!(indexed
                .iter()
                .all(|file| !file.path.to_string_lossy().contains("linked")));
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn mounted_tick_memory_survives_settings_roundtrip_without_rows() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "index": "catalog.sqlite", "roots": [], "concurrency": 8,
            "mounted_sources": [], "mounted_checks": {"e:/avid mediafiles/mxf/1": false},
            "custom_folders": []
        }))
        .unwrap();
        let saved = serde_json::to_value(settings).unwrap();
        let restored: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(
            restored.mounted_checks.get("e:/avid mediafiles/mxf/1"),
            Some(&false)
        );
        assert!(restored.checked_roots().is_empty());
    }
    #[test]
    fn only_checked_rows_are_scan_roots() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "index": "catalog.sqlite", "roots": ["/Old-Island"], "concurrency": 8,
            "mounted_sources": [
                {"path": "/Media-Island/MXF/1", "checked": true, "offline": true},
                {"path": "/Media-Island/MXF/2", "checked": false}
            ],
            "custom_folders": [
                {"path": "/Project-Island/Backup", "checked": false},
                {"path": "/Project-Island/Media", "checked": true},
                {"path": "/Project-Island/Media/Nested", "checked": true},
                {"path": "/Project-Island/Media", "checked": true}
            ]
        }))
        .unwrap();
        assert_eq!(
            settings.checked_roots(),
            vec![PathBuf::from("/Project-Island/Media")]
        );
    }
    #[test]
    fn legacy_roots_migrate_to_checked_custom_folders() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "index": "catalog.sqlite", "roots": ["/Media-Island/MXF/1"], "concurrency": 8
        }))
        .unwrap();
        assert_eq!(settings.custom_folders.len(), 1);
        assert!(settings.custom_folders[0].checked);
        assert_eq!(settings.custom_folders[0].path, settings.roots[0]);
        assert!(settings.mounted_sources.is_empty());
        let mut saved = serde_json::to_value(settings).unwrap();
        saved["custom_folders"] = serde_json::json!([]);
        let reloaded: Settings = serde_json::from_value(saved).unwrap();
        assert!(reloaded.custom_folders.is_empty());
    }
    #[test]
    fn old_proxy_limit_is_ignored_and_never_saved_again() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "index": "catalog.sqlite", "roots": [], "concurrency": 8,
            "preview_cache_bytes": 5_000_000_000u64,
        }))
        .unwrap();
        let saved = serde_json::to_value(settings).unwrap();
        assert!(saved.get("preview_cache_bytes").is_none());
    }
    #[test]
    fn source_scans_are_incremental_read_only_and_keep_offline_index() {
        let dir = std::env::temp_dir().join(format!("mobboss-auto-scan-{}", std::process::id()));
        let root = dir.join("Media-01/Avid MediaFiles");
        fs::create_dir_all(root.join("MXF/1")).unwrap();
        let media = root.join("MXF/1/clip.mxf");
        let header = include_bytes!("../../../../tests/fixtures/video.header");
        fs::write(&media, header).unwrap();
        let settings = Settings {
            index: dir.join("catalog.sqlite"),
            roots: vec![root.clone()],
            concurrency: 2,
            mounted_sources: vec![],
            mounted_checks: Default::default(),
            custom_folders: vec![],
        };
        let options = ScanOptions {
            mode: mobboss_core::ScanMode::Quick,
            concurrency: 2,
        };
        let token = CancellationToken::default();
        let first = scan(&settings, &options, &token, |_| {}).unwrap();
        assert_eq!((first[0].files, first[0].headers_read), (1, 1));
        let second = scan(&settings, &options, &token, |_| {}).unwrap();
        assert_eq!(
            (second[0].files, second[0].headers_read, second[0].cached),
            (1, 0, 1)
        );
        assert_eq!(fs::read(&media).unwrap(), header);
        fs::rename(&root, dir.join("disconnected-media")).unwrap();
        assert!(scan(&settings, &options, &token, |_| {})
            .unwrap()
            .is_empty());
        assert_eq!(files(&settings).unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_media_exports_and_index_overwrite() {
        let dir = std::env::temp_dir().join(format!("mobboss-service-{}", std::process::id()));
        fs::create_dir_all(dir.join("media")).unwrap();
        fs::write(dir.join("media/file.mxf"), []).unwrap();
        let roots = vec![dir.join("media")];
        assert!(validate_destination(&dir.join("media/report.csv"), &roots).is_err());
        assert!(validate_destination(&dir.join("media/report.csv"), &[]).is_err());
        assert!(export(
            &dir.join("index.csv"),
            "test",
            &roots,
            &dir.join("index.csv")
        )
        .is_err());
        export(
            &dir.join("report.csv"),
            "test",
            &roots,
            &dir.join("index.sqlite"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("report.csv")).unwrap(), "test");
        #[cfg(unix)]
        assert!(validate_destination(&dir.join("drive-report.csv"), &[PathBuf::from("/")]).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
}
