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

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub index: PathBuf,
    pub roots: Vec<PathBuf>,
    pub concurrency: usize,
}
impl Settings {
    pub fn load() -> Result<Self> {
        let index = default_index()?;
        let config = index.with_file_name("settings.json");
        if config.exists() {
            return Ok(serde_json::from_slice(&fs::read(config)?)?);
        }
        Ok(Self {
            index,
            roots: vec![],
            concurrency: 8,
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
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        ensure!(
            !dest.starts_with(&root) || root.parent().is_none(),
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
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<_>>()?;
    for (i, root) in roots.iter().enumerate() {
        if roots
            .iter()
            .enumerate()
            .any(|(j, parent)| i != j && root.starts_with(parent) && (root != parent || j < i))
        {
            continue;
        }
        reports.push(index.scan_with(root, options, token, &mut progress)?);
    }
    Ok(reports)
}
#[derive(Serialize)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub available: u64,
    pub total: u64,
}
pub fn volumes() -> Vec<Volume> {
    sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| {
            let mount = d.mount_point().to_string_lossy();
            let name = d.name().to_string_lossy();
            let filesystem = d.file_system().to_string_lossy();
            !["/proc", "/sys", "/run", "/snap", "/dev"]
                .iter()
                .any(|p| mount == *p || mount.starts_with(&format!("{p}/")))
                && !["systemd-1", "tmpfs", "overlay"].contains(&name.as_ref())
                && !name.starts_with("/dev/mapper")
                && !["tmpfs", "overlay", "squashfs", "proc", "sysfs", "devtmpfs"]
                    .contains(&filesystem.as_ref())
        })
        .map(|d| Volume {
            name: if cfg!(target_os = "windows") {
                format!(
                    "{} {}",
                    d.mount_point().display(),
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
        .collect()
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
    let path = path.canonicalize()?;
    ensure!(
        files(settings)?.iter().any(|f| f.path == path),
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
