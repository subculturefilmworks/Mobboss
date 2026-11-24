use crate::{mxf, pmr, MediaMetadata};
use anyhow::{ensure, Context, Result};
use directories::ProjectDirs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    Quick,
    Auto,
    Deep,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFile {
    pub path: PathBuf,
    pub folder: PathBuf,
    pub size: u64,
    pub modified_ns: String,
    pub metadata: MediaMetadata,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Diff {
    pub missing: Vec<PathBuf>,
    pub unlisted: Vec<PathBuf>,
    pub changed: Vec<PathBuf>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScanReport {
    pub files: usize,
    pub headers_read: usize,
    pub cached: usize,
    pub projects: BTreeMap<String, usize>,
    pub elapsed_seconds: f64,
    pub diff: Diff,
    pub warnings: Vec<String>,
}
pub struct Index {
    conn: Connection,
    path: PathBuf,
}
pub fn default_index() -> Result<PathBuf> {
    Ok(ProjectDirs::from("org", "MobBoss", "MobBoss")
        .context("no app data directory")?
        .data_local_dir()
        .join("index.sqlite"))
}
fn canonical_destination(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(
            ancestor
                .file_name()
                .context("invalid index path")?
                .to_owned(),
        );
        ancestor = ancestor.parent().context("invalid index parent")?;
    }
    let mut dest = ancestor.canonicalize()?;
    for part in tail.iter().rev() {
        dest.push(part)
    }
    Ok(dest)
}
impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        let path = canonical_destination(path)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS folders(path TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS projects(name TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY,folder TEXT NOT NULL,size INTEGER NOT NULL,modified_ns TEXT NOT NULL,project TEXT,metadata TEXT NOT NULL,present INTEGER NOT NULL DEFAULT 1); CREATE TABLE IF NOT EXISTS scan_runs(id INTEGER PRIMARY KEY,root TEXT,mode TEXT,started INTEGER,report TEXT);")?;
        Ok(Self { conn, path })
    }
    pub fn files(&self, project: Option<&str>) -> Result<Vec<MediaFile>> {
        let mut q=self.conn.prepare("SELECT path,folder,size,modified_ns,metadata FROM files WHERE present=1 AND (?1 IS NULL OR project=?1) ORDER BY path")?;
        let rows = q.query_map([project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (p, f, size, modified_ns, m) = row?;
            out.push(MediaFile {
                path: p.into(),
                folder: f.into(),
                size,
                modified_ns,
                metadata: serde_json::from_str(&m)?,
            });
        }
        Ok(out)
    }
    pub fn projects(&self) -> Result<BTreeMap<String, usize>> {
        let mut out = BTreeMap::new();
        for f in self.files(None)? {
            *out.entry(f.metadata.project.unwrap_or_else(|| "Unknown".into()))
                .or_default() += 1;
        }
        Ok(out)
    }
    pub fn scan(&mut self, root: &Path, mode: ScanMode) -> Result<ScanReport> {
        let started = Instant::now();
        let root = root.canonicalize()?;
        ensure!(root.is_dir(), "scan root must be a directory");
        ensure!(
            !self.path.starts_with(&root),
            "index must be outside the scanned media tree"
        );
        let mut report = ScanReport::default();
        let disk = discover(&root, &mut report.warnings);
        let folders: BTreeSet<_> = disk
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        // Reject indexes in any known media folder even if scanning a different root.
        for ancestor in self.path.ancestors() {
            ensure!(
                !ancestor.file_name().is_some_and(|s| s
                    .to_string_lossy()
                    .eq_ignore_ascii_case("Avid MediaFiles")
                    || s.to_string_lossy().eq_ignore_ascii_case("OMFI MediaFiles")),
                "index cannot be inside a media folder"
            );
        }
        let prior: BTreeMap<_, _> = self
            .files(None)?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        let mut database = BTreeMap::new();
        let mut db_folders = BTreeSet::new();
        for folder in &folders {
            for entry in fs::read_dir(folder)? {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        report.warnings.push(e.to_string());
                        continue;
                    }
                };
                let p = entry.path();
                if is_ext(&p, "pmr") {
                    db_folders.insert(folder.clone());
                    match fs::read(&p)
                        .map_err(anyhow::Error::from)
                        .and_then(|b| pmr::parse(&b))
                    {
                        Ok(entries) => {
                            for e in entries {
                                let name = Path::new(&e.filename);
                                if name.components().count() == 1 {
                                    database.insert(folder.join(name), e);
                                }
                            }
                        }
                        Err(e) => report.warnings.push(format!("{}: {e}", p.display())),
                    }
                } else if is_ext(&p, "mdb") {
                    db_folders.insert(folder.clone());
                    report.warnings.push(format!(
                        "{}: MDB inventory is unsupported; using PMR, index and headers",
                        p.display()
                    ));
                }
            }
        }
        let listing: BTreeSet<_> = disk.iter().cloned().collect();
        let mut expected: BTreeSet<_> = database.keys().cloned().collect();
        expected.extend(prior.keys().filter(|p| p.starts_with(&root)).cloned());
        report.diff.missing = expected.difference(&listing).cloned().collect();
        report.diff.unlisted = listing.difference(&expected).cloned().collect();
        let mut records = Vec::new();
        for path in disk {
            let stat = match fs::metadata(&path) {
                Ok(s) => s,
                Err(e) => {
                    report.warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let stamp = stat
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().to_string())
                .unwrap_or_default();
            let previous = prior.get(&path);
            let changed = previous.is_some_and(|p| p.size != stat.len() || p.modified_ns != stamp);
            if changed {
                report.diff.changed.push(path.clone());
            }
            let folder = path.parent().context("file without parent")?.to_path_buf();
            let deep =
                mode == ScanMode::Deep || (mode == ScanMode::Auto && !db_folders.contains(&folder));
            let metadata = if !deep && !changed && previous.is_some() {
                report.cached += 1;
                previous.context("missing cache")?.metadata.clone()
            } else {
                report.headers_read += 1;
                match mxf::read(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        report.warnings.push(format!("{}: {e:#}", path.display()));
                        continue;
                    }
                }
            };
            if let Some(e) = database.get(&path) {
                if metadata.project.as_deref() != Some(e.project.as_str()) {
                    report.warnings.push(format!(
                        "{}: PMR project differs from MXF header",
                        path.display()
                    ));
                }
            }
            *report
                .projects
                .entry(metadata.project.clone().unwrap_or_else(|| "Unknown".into()))
                .or_default() += 1;
            records.push(MediaFile {
                path,
                folder,
                size: stat.len(),
                modified_ns: stamp,
                metadata,
            });
        }
        report.files = records.len();
        report.elapsed_seconds = started.elapsed().as_secs_f64();
        let tx = self.conn.transaction()?;
        // Only mark rows in this exact discovered scope absent; path prefix matching is component-aware.
        for path in prior.keys().filter(|p| p.starts_with(&root)) {
            tx.execute(
                "UPDATE files SET present=0 WHERE path=?1",
                [path.to_string_lossy().as_ref()],
            )?;
        }
        for f in records {
            let project = f.metadata.project.as_deref();
            tx.execute(
                "INSERT OR IGNORE INTO folders VALUES(?1)",
                [f.folder.to_string_lossy().as_ref()],
            )?;
            if let Some(p) = project {
                tx.execute("INSERT OR IGNORE INTO projects VALUES(?1)", [p])?;
            }
            tx.execute("INSERT INTO files(path,folder,size,modified_ns,project,metadata,present) VALUES(?1,?2,?3,?4,?5,?6,1) ON CONFLICT(path) DO UPDATE SET folder=excluded.folder,size=excluded.size,modified_ns=excluded.modified_ns,project=excluded.project,metadata=excluded.metadata,present=1",params![f.path.to_string_lossy(),f.folder.to_string_lossy(),f.size,f.modified_ns,project,serde_json::to_string(&f.metadata)?])?;
        }
        tx.execute(
            "INSERT INTO scan_runs(root,mode,started,report) VALUES(?1,?2,?3,?4)",
            params![
                root.to_string_lossy(),
                format!("{mode:?}"),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                serde_json::to_string(&report)?
            ],
        )?;
        tx.commit()?;
        Ok(report)
    }
}
fn is_ext(p: &Path, ext: &str) -> bool {
    p.extension()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(ext))
}
/// Recursion also handles arbitrary MXF folders, nested workspaces, host.N and OMFI layouts.
pub fn discover(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in WalkDir::new(root).follow_links(false) {
        match e {
            Ok(e) if e.file_type().is_file() && is_ext(e.path(), "mxf") => out.push(e.into_path()),
            Ok(_) => {}
            Err(e) => warnings.push(e.to_string()),
        }
    }
    out.sort();
    out
}
