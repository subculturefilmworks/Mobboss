use crate::{mxf, pmr, MediaMetadata};
use anyhow::{ensure, Context, Result};
use directories::ProjectDirs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
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
    #[serde(serialize_with = "crate::paths::serialize")]
    pub path: PathBuf,
    #[serde(serialize_with = "crate::paths::serialize")]
    pub folder: PathBuf,
    pub size: u64,
    pub modified_ns: String,
    pub metadata: MediaMetadata,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Diff {
    #[serde(serialize_with = "crate::paths::serialize_many")]
    pub missing: Vec<PathBuf>,
    #[serde(serialize_with = "crate::paths::serialize_many")]
    pub unlisted: Vec<PathBuf>,
    #[serde(serialize_with = "crate::paths::serialize_many")]
    pub changed: Vec<PathBuf>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Reconciliation {
    #[serde(serialize_with = "crate::paths::serialize")]
    pub root: PathBuf,
    pub media_files: usize,
    pub database_files: Vec<DatabaseFile>,
    pub other_files: usize,
    #[serde(serialize_with = "crate::paths::serialize_many")]
    pub other_names: Vec<PathBuf>,
    pub folders: usize,
    pub total_bytes: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseFile {
    #[serde(serialize_with = "crate::paths::serialize")]
    pub path: PathBuf,
    pub size: u64,
    pub modified_ns: String,
    pub description: String,
    pub pmr_missing: Option<usize>,
    pub pmr_extra: Option<usize>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScanReport {
    #[serde(default)]
    pub reconciliation: Reconciliation,
    pub files: usize,
    pub headers_read: usize,
    pub cached: usize,
    pub projects: BTreeMap<String, usize>,
    pub elapsed_seconds: f64,
    pub diff: Diff,
    pub warnings: Vec<String>,
}
/// Shared cancellation flag. Cancellation leaves the committed index unchanged.
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub mode: ScanMode,
    pub concurrency: usize,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            mode: ScanMode::Auto,
            concurrency: 8,
        }
    }
}
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Discovery,
    Inventory,
    Enrichment,
    Complete,
}
#[derive(Debug, Serialize, Clone)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub done: usize,
    pub total: usize,
    #[serde(serialize_with = "crate::paths::serialize")]
    pub current_folder: PathBuf,
    /// Inventory rows are provisional; enrichment rows are verified by MXF headers.
    pub files: Vec<MediaFile>,
}
pub struct Index {
    conn: Connection,
    path: PathBuf,
}
pub fn default_index() -> Result<PathBuf> {
    crate::paths::canonical(
        &ProjectDirs::from("org", "MobBoss", "MobBoss")
            .context("no app data directory")?
            .data_local_dir()
            .join("index.sqlite"),
    )
}
fn canonical_destination(path: &Path) -> Result<PathBuf> {
    crate::paths::canonical(path)
}
/// Canonicalize and reject destinations in known media directories before any writes.
pub fn validate_destination(path: &Path) -> Result<PathBuf> {
    let path = canonical_destination(path)?;
    for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
        ensure!(
            !ancestor.file_name().is_some_and(|n| n
                .to_string_lossy()
                .eq_ignore_ascii_case("Avid MediaFiles")
                || n.to_string_lossy().eq_ignore_ascii_case("OMFI MediaFiles")),
            "index cannot be inside a media folder"
        );
        if ancestor.is_dir() {
            let entries = fs::read_dir(ancestor)?;
            for entry in entries {
                let entry = entry?;
                let p = entry.path();
                ensure!(
                    !(is_ext(&p, "mxf") || is_ext(&p, "pmr") || is_ext(&p, "mdb")),
                    "index cannot be inside a media folder"
                );
            }
        }
    }
    Ok(path)
}
// Keep fallback clip IDs identical to the prefix-free path serialized to the UI.
fn clip_expression(path: &str, metadata: &str) -> String {
    format!(
        r"CASE WHEN length(json_extract({metadata},'$.material_package_uid'))>0 THEN 'uid:'||json_extract({metadata},'$.material_package_uid') ELSE 'file:'||CASE WHEN substr({path},1,8)='\\?\UNC\' THEN '\\'||substr({path},9) WHEN substr({path},1,4)='\\?\' THEN substr({path},5) ELSE {path} END END"
    )
}
impl Index {
    /// Validate the complete destination before any SQLite or directory writes.
    pub fn open_for_scan(path: &Path, root: &Path) -> Result<Self> {
        let root = crate::paths::selected(root)?;
        let dest = canonical_destination(path)?;
        ensure!(
            !crate::paths::contains(&dest, &root) || root.parent().is_none(),
            "index must be outside the scanned media tree"
        );
        Self::open(&dest)
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = canonical_destination(path)?;
        validate_destination(&path)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS folders(path TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS projects(name TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY,folder TEXT NOT NULL,size INTEGER NOT NULL,modified_ns TEXT NOT NULL,project TEXT,metadata TEXT NOT NULL,present INTEGER NOT NULL DEFAULT 1); CREATE TABLE IF NOT EXISTS scan_runs(id INTEGER PRIMARY KEY,root TEXT,mode TEXT,started INTEGER,report TEXT);")?;
        // Version 1: persistent grouping, backfilled without touching media. Triggers also
        // cover transfers, missing rows and old clients that only update files.
        if conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? < 1 {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS clip_members(path TEXT PRIMARY KEY REFERENCES files(path) ON DELETE CASCADE,clip_id TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS clip_members_id ON clip_members(clip_id);
            INSERT OR REPLACE INTO clip_members SELECT path, CASE WHEN length(json_extract(metadata,'$.material_package_uid'))>0 THEN 'uid:'||json_extract(metadata,'$.material_package_uid') ELSE 'file:'||path END FROM files;
            CREATE TRIGGER IF NOT EXISTS clip_insert AFTER INSERT ON files BEGIN INSERT OR REPLACE INTO clip_members VALUES(NEW.path,CASE WHEN length(json_extract(NEW.metadata,'$.material_package_uid'))>0 THEN 'uid:'||json_extract(NEW.metadata,'$.material_package_uid') ELSE 'file:'||NEW.path END); END;
            CREATE TRIGGER IF NOT EXISTS clip_update AFTER UPDATE OF metadata ON files BEGIN UPDATE clip_members SET clip_id=CASE WHEN length(json_extract(NEW.metadata,'$.material_package_uid'))>0 THEN 'uid:'||json_extract(NEW.metadata,'$.material_package_uid') ELSE 'file:'||NEW.path END WHERE path=NEW.path; END;
            CREATE TRIGGER IF NOT EXISTS clip_delete AFTER DELETE ON files BEGIN DELETE FROM clip_members WHERE path=OLD.path; END;
            PRAGMA user_version=1;")?;
        }
        let mut index = Self { conn, path };
        index.normalize_stored_paths()?;
        Ok(index)
    }
    fn normalize_stored_paths(&mut self) -> Result<()> {
        if self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?
            >= 2
        {
            return Ok(());
        }
        let rows = {
            let mut q = self.conn.prepare(
                "SELECT path,folder,size,modified_ns,project,metadata,present FROM files",
            )?;
            let rows = q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, bool>(6)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let roots = rows
            .iter()
            .map(|r| self.source_root(Path::new(&r.0)))
            .collect::<Result<Vec<_>>>()?;
        let tx = self.conn.transaction()?;
        for ((old, _folder, size, modified, project, metadata, present), source_root) in
            rows.into_iter().zip(roots)
        {
            // Leave unsafe/disconnected rows alone; operations still validate them.
            let Ok(path) = crate::paths::from_selected_root(Path::new(&old), &source_root) else {
                continue;
            };
            let folder = path.parent().context("no file parent")?;
            if path.as_os_str() != std::ffi::OsStr::new(&old) {
                tx.execute("INSERT INTO files(path,folder,size,modified_ns,project,metadata,present) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(path) DO UPDATE SET present=max(files.present,excluded.present)", params![path.to_string_lossy(),folder.to_string_lossy(),size,modified,project,metadata,present])?;
                tx.execute("DELETE FROM files WHERE path=?1", [&old])?;
            } else {
                tx.execute(
                    "UPDATE files SET folder=?1 WHERE path=?2",
                    params![folder.to_string_lossy(), old],
                )?;
            }
        }
        tx.execute("DELETE FROM folders", [])?;
        tx.execute("INSERT INTO folders SELECT DISTINCT folder FROM files", [])?;
        let roots = {
            let mut q = tx.prepare("SELECT id,root FROM scan_runs")?;
            let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, root) in roots {
            if let Ok(root) = crate::paths::canonical(Path::new(&root)) {
                tx.execute(
                    "UPDATE scan_runs SET root=?1 WHERE id=?2",
                    params![root.to_string_lossy(), id],
                )?;
            }
        }
        let current = clip_expression("path", "metadata");
        let inserted = clip_expression("NEW.path", "NEW.metadata");
        tx.execute_batch(&format!("DROP TRIGGER IF EXISTS clip_insert; DROP TRIGGER IF EXISTS clip_update;
            INSERT OR REPLACE INTO clip_members SELECT path,{current} FROM files;
            CREATE TRIGGER clip_insert AFTER INSERT ON files BEGIN INSERT OR REPLACE INTO clip_members VALUES(NEW.path,{inserted}); END;
            CREATE TRIGGER clip_update AFTER UPDATE OF metadata ON files BEGIN UPDATE clip_members SET clip_id={inserted} WHERE path=NEW.path; END;
            PRAGMA user_version=2;"))?;
        tx.commit()?;
        Ok(())
    }
    pub fn source_root(&self, path: &Path) -> Result<PathBuf> {
        let path = crate::paths::native(path);
        let path = path.as_path();
        let mut q = self.conn.prepare("SELECT DISTINCT root FROM scan_runs")?;
        let roots = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(roots
            .into_iter()
            .map(PathBuf::from)
            .filter(|r| crate::paths::contains(path, r))
            .max_by_key(|p| p.components().count())
            .unwrap_or_else(|| {
                crate::paths::tree_root(path)
                    .unwrap_or_else(|| path.parent().unwrap_or(path))
                    .to_path_buf()
            }))
    }
    pub fn clips(&self) -> Result<Vec<crate::clips::Clip>> {
        Ok(crate::clips::group(self.files(None)?))
    }
    pub fn files(&self, project: Option<&str>) -> Result<Vec<MediaFile>> {
        self.read_files(project, false)
    }
    /// Retained rows include files marked absent by prior scans.
    pub fn files_including_absent(&self) -> Result<Vec<MediaFile>> {
        self.read_files(None, true)
    }
    fn read_files(&self, project: Option<&str>, include_absent: bool) -> Result<Vec<MediaFile>> {
        let mut q=self.conn.prepare("SELECT path,folder,size,modified_ns,metadata FROM files WHERE (present=1 OR ?2) AND (?1 IS NULL OR project=?1) ORDER BY path")?;
        let rows = q.query_map(params![project, include_absent], |r| {
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
            let mut metadata: MediaMetadata = serde_json::from_str(&m)?;
            if let Some(ul) = &metadata.codec_ul {
                let label = mxf::codec_label(ul, metadata.edit_rate.as_ref());
                if label != "Unknown" {
                    metadata.codec = Some(label);
                }
            }
            out.push(MediaFile {
                path: p.into(),
                folder: f.into(),
                size,
                modified_ns,
                metadata,
            });
        }
        Ok(out)
    }
    /// Persist a verified media transfer without rescanning headers.
    pub fn record_transfer(
        &mut self,
        original: &MediaFile,
        source: &Path,
        dest: &Path,
        moved: bool,
    ) -> Result<()> {
        let dest = crate::paths::media(dest)?;
        let source = crate::paths::canonical(source)?;
        let stat = fs::metadata(&dest)?;
        let folder = dest.parent().context("no destination parent")?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO folders(path) VALUES(?1)",
            [folder.to_string_lossy()],
        )?;
        tx.execute("INSERT INTO files(path,folder,size,modified_ns,project,metadata,present) VALUES(?1,?2,?3,?4,?5,?6,1) ON CONFLICT(path) DO UPDATE SET folder=excluded.folder,size=excluded.size,modified_ns=excluded.modified_ns,project=excluded.project,metadata=excluded.metadata,present=1", params![dest.to_string_lossy(), folder.to_string_lossy(), stat.len(), crate::ops::stamp(&stat), original.metadata.project, serde_json::to_string(&original.metadata)?])?;
        if moved {
            tx.execute(
                "DELETE FROM files WHERE path=?1",
                [source.to_string_lossy()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn forget_path(&mut self, path: &Path) -> Result<()> {
        let path = crate::paths::canonical(path)?;
        self.conn
            .execute("DELETE FROM files WHERE path=?1", [path.to_string_lossy()])?;
        Ok(())
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
        self.scan_with(
            root,
            &ScanOptions {
                mode,
                ..ScanOptions::default()
            },
            &CancellationToken::default(),
            |_| {},
        )
    }
    pub fn scan_with(
        &mut self,
        root: &Path,
        options: &ScanOptions,
        cancel: &CancellationToken,
        mut progress: impl FnMut(ScanProgress),
    ) -> Result<ScanReport> {
        ensure!(
            (1..=64).contains(&options.concurrency),
            "concurrency must be between 1 and 64"
        );
        let mode = options.mode;
        let started = Instant::now();
        ensure!(!cancel.is_cancelled(), "scan cancelled");
        let root = crate::paths::selected(root)?;
        progress(ScanProgress {
            phase: ScanPhase::Discovery,
            done: 0,
            total: 0,
            current_folder: root.to_path_buf(),
            files: vec![],
        });
        ensure!(root.is_dir(), "scan root must be a directory");
        ensure!(
            !crate::paths::contains(&self.path, &root) || root.parent().is_none(),
            "index must be outside the scanned media tree"
        );
        let mut report = ScanReport::default();
        let disk = discover_with_cancel(&root, &mut report.warnings, cancel)?;
        let mut folders: BTreeSet<_> = disk
            .iter()
            .filter_map(|p| {
                p.parent()
                    .map(|p| crate::paths::storage(p).unwrap_or_else(|_| p.to_path_buf()))
            })
            .collect();
        let media_trees: Vec<_> = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_type().is_dir()
                    && e.file_name().to_str().is_some_and(|n| {
                        n.eq_ignore_ascii_case("Avid MediaFiles")
                            || n.eq_ignore_ascii_case("OMFI MediaFiles")
                    })
            })
            .map(|e| e.into_path())
            .collect();
        report.reconciliation.root = root.clone();
        // Count filesystem items independently of successful media header decoding.
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            ensure!(!cancel.is_cancelled(), "scan cancelled");
            if entry.file_type().is_symlink() {
                continue;
            }
            let canonical_entry = crate::paths::storage(entry.path())?;
            let p = canonical_entry.as_path();
            // A drive can contain unrelated documents and application state. Reconcile only
            // Avid trees when present; direct arbitrary media-folder scans retain their scope.
            if !media_trees.is_empty()
                && !media_trees
                    .iter()
                    .any(|tree| crate::paths::contains(p, tree))
                && !disk.contains(&p.to_path_buf())
            {
                continue;
            }
            if entry.file_type().is_dir() {
                if p != root {
                    report.reconciliation.folders += 1;
                }
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let stat = match entry.metadata() {
                Ok(stat) => stat,
                Err(e) => {
                    report.warnings.push(e.to_string());
                    continue;
                }
            };
            report.reconciliation.total_bytes += stat.len();
            if is_media(p) {
                report.reconciliation.media_files += 1;
            } else if !is_apple_metadata(p) && (is_ext(p, "pmr") || is_ext(p, "mdb")) {
                if let Some(folder) = p.parent() {
                    folders.insert(folder.to_path_buf());
                }
                report.reconciliation.database_files.push(DatabaseFile {
                    path: p.to_path_buf(), size: stat.len(), modified_ns: crate::ops::stamp(&stat),
                    description: if is_ext(p, "mdb") {
                        "Avid media database (msmMMOB.mdb) — recognized, not needed: Mob Boss reads every MXF header directly"
                    } else { "Avid file inventory (msmFMID.pmr) — recognized" }.into(),
                    pmr_missing: None, pmr_extra: None,
                });
            } else {
                report.reconciliation.other_files += 1;
                if report.reconciliation.other_names.len() < 200 {
                    report
                        .reconciliation
                        .other_names
                        .push(p.strip_prefix(&root).unwrap_or(p).to_path_buf());
                }
            }
        }
        let prior: BTreeMap<_, _> = self
            .files(None)?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        let mut database = BTreeMap::new();
        let mut db_folders = BTreeSet::new();
        let mut pmr_folders = BTreeSet::new();
        for folder in &folders {
            ensure!(!cancel.is_cancelled(), "scan cancelled");
            let entries = match fs::read_dir(folder) {
                Ok(entries) => entries,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("{}: {e}", crate::paths::display(folder).display()));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        report.warnings.push(e.to_string());
                        continue;
                    }
                };
                if entry.file_type()?.is_symlink() {
                    continue;
                }
                let p = crate::paths::storage(&entry.path())?;
                if !is_apple_metadata(&p) && is_ext(&p, "pmr") {
                    db_folders.insert(folder.clone());
                    match fs::read(&p)
                        .map_err(anyhow::Error::from)
                        .and_then(|b| pmr::parse(&b))
                    {
                        Ok(entries) => {
                            pmr_folders.insert(folder.clone());
                            for e in entries {
                                let name = Path::new(&e.filename);
                                if name.components().count() == 1 {
                                    database.insert(crate::paths::storage(&folder.join(name))?, e);
                                }
                            }
                        }
                        Err(e) => report
                            .warnings
                            .push(format!("{}: {e}", crate::paths::display(&p).display())),
                    }
                } else if !is_apple_metadata(&p) && is_ext(&p, "mdb") {
                    db_folders.insert(folder.clone());
                    match crate::mdb::inspect(&p) {
                        Ok(info) => {
                            if let Some(db) = report
                                .reconciliation
                                .database_files
                                .iter_mut()
                                .find(|db| db.path == p)
                            {
                                db.description.push_str(&format!(
                                    "; Bento TOC: {} bytes at {}",
                                    info.toc_length, info.toc_offset
                                ));
                            }
                        }
                        Err(e) => report
                            .warnings
                            .push(format!("{}: {e}", crate::paths::display(&p).display())),
                    }
                }
            }
        }
        let listing: BTreeSet<_> = disk.iter().cloned().collect();
        for folder in &pmr_folders {
            let expected: BTreeSet<_> = database
                .keys()
                .filter(|p| p.parent().is_some_and(|p| crate::paths::same(p, folder)))
                .collect();
            let actual: BTreeSet<_> = listing
                .iter()
                .filter(|p| p.parent().is_some_and(|p| crate::paths::same(p, folder)))
                .collect();
            let missing = expected.difference(&actual).count();
            let extra = actual.difference(&expected).count();
            for db in report
                .reconciliation
                .database_files
                .iter_mut()
                .filter(|db| {
                    db.path
                        .parent()
                        .is_some_and(|p| crate::paths::same(p, folder))
                })
            {
                db.pmr_missing = Some(missing);
                db.pmr_extra = Some(extra);
            }
            if missing > 0 || extra > 0 {
                report.warnings.push(format!(
                    "{}: PMR mismatch: {missing} missing from disk, {extra} extra on disk",
                    crate::paths::display(folder).display()
                ));
            }
        }
        let mut expected: BTreeSet<_> = database.keys().cloned().collect();
        expected.extend(
            prior
                .keys()
                .filter(|p| {
                    crate::paths::contains(p, &root)
                        && !p.parent().is_some_and(|f| pmr_folders.contains(f))
                })
                .cloned(),
        );
        let expected: BTreeMap<_, _> = expected
            .into_iter()
            .map(|p| (crate::paths::key(&p), p))
            .collect();
        let listing: BTreeMap<_, _> = listing
            .into_iter()
            .map(|p| (crate::paths::key(&p), p))
            .collect();
        report.diff.missing = expected
            .iter()
            .filter(|(key, _)| !listing.contains_key(*key))
            .map(|(_, p)| p.clone())
            .collect();
        report.diff.unlisted = listing
            .iter()
            .filter(|(key, _)| !expected.contains_key(*key))
            .map(|(_, p)| p.clone())
            .collect();
        let mut records = Vec::new();
        let mut jobs = Vec::new();
        let mut inventory = Vec::new();
        for path in disk {
            ensure!(!cancel.is_cancelled(), "scan cancelled");
            let stat = match fs::metadata(&path) {
                Ok(s) => s,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("{}: {e}", crate::paths::display(&path).display()));
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
            let unknown = previous.is_none_or(|p| {
                p.metadata.project.is_none()
                    || p.metadata.file_package_uid.is_none()
                    || p.metadata.material_package_uid.is_none()
                    || p.metadata.header_version != Some(1)
            });
            let db_changed = database.get(&path).zip(previous).is_some_and(|(e, p)| {
                p.metadata.file_package_uid.as_deref() != Some(e.file_package_uid.as_str())
                    || p.metadata.project.as_deref() != Some(e.project.as_str())
            });
            let mut file = MediaFile {
                path: path.clone(),
                folder,
                size: stat.len(),
                modified_ns: stamp,
                metadata: MediaMetadata::default(),
            };
            if !deep && !changed && !unknown && !db_changed {
                report.cached += 1;
                file.metadata = previous.context("missing cache")?.metadata.clone();
                inventory.push(file.clone());
                records.push(file);
            } else {
                if !deep {
                    if let Some(e) = database.get(&path) {
                        file.metadata.project = Some(e.project.clone());
                        file.metadata.file_package_uid = Some(e.file_package_uid.clone());
                        // PMR's source UID is not necessarily the material package UID.
                        inventory.push(file.clone());
                    }
                }
                jobs.push(file);
            }
        }
        let total = records.len() + jobs.len();
        progress(ScanProgress {
            phase: ScanPhase::Inventory,
            done: inventory.len(),
            total,
            current_folder: root.clone(),
            files: inventory,
        });
        let next = AtomicUsize::new(0);
        let mut done = records.len();
        std::thread::scope(|scope| {
            let (send, receive) = mpsc::sync_channel(options.concurrency * 2);
            for _ in 0..options.concurrency.min(jobs.len()) {
                let send = send.clone();
                let jobs = &jobs;
                let next = &next;
                scope.spawn(move || {
                    while !cancel.is_cancelled() {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = jobs.get(i) else { break };
                        let result = mxf::read(&file.path);
                        if send.send((i, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(send);
            for (i, result) in receive {
                report.headers_read += 1;
                done += 1;
                let mut file = jobs[i].clone();
                let mut update = Vec::new();
                match result {
                    Ok(metadata) => {
                        if let Some(e) = database.get(&file.path) {
                            if metadata.project.as_deref() != Some(e.project.as_str()) {
                                report.warnings.push(format!(
                                    "{}: PMR project differs from MXF header",
                                    crate::paths::display(&file.path).display()
                                ));
                            }
                        }
                        file.metadata = metadata;
                        update.push(file.clone());
                        records.push(file.clone());
                    }
                    Err(e) => report.warnings.push(format!(
                        "{}: {e:#}",
                        crate::paths::display(&file.path).display()
                    )),
                }
                progress(ScanProgress {
                    phase: ScanPhase::Enrichment,
                    done,
                    total,
                    current_folder: file.folder,
                    files: update,
                });
            }
        });
        ensure!(!cancel.is_cancelled(), "scan cancelled");
        records.sort_by(|a, b| a.path.cmp(&b.path));
        for file in &records {
            *report
                .projects
                .entry(
                    file.metadata
                        .project
                        .clone()
                        .unwrap_or_else(|| "Unknown".into()),
                )
                .or_default() += 1;
        }
        report.files = records.len();
        report.elapsed_seconds = started.elapsed().as_secs_f64();
        report.warnings = report
            .warnings
            .into_iter()
            .map(|m| crate::paths::message(&m))
            .collect();
        let tx = self.conn.transaction()?;
        // Only mark rows in this exact discovered scope absent; path prefix matching is component-aware.
        for path in prior.keys().filter(|p| crate::paths::contains(p, &root)) {
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
        progress(ScanProgress {
            phase: ScanPhase::Complete,
            done: report.files,
            total,
            current_folder: root,
            files: vec![],
        });
        Ok(report)
    }
}
fn is_ext(p: &Path, ext: &str) -> bool {
    p.extension()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(ext))
}
/// Recursion also handles arbitrary MXF folders, nested workspaces, host.N and OMFI layouts.
pub fn discover(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    discover_with_cancel(root, warnings, &CancellationToken::default()).unwrap_or_default()
}
fn discover_with_cancel(
    root: &Path,
    warnings: &mut Vec<String>,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in WalkDir::new(root).follow_links(false) {
        ensure!(!cancel.is_cancelled(), "scan cancelled");
        match e {
            Ok(e) if e.file_type().is_file() && is_media(e.path()) => {
                if e.path().to_str().is_some() {
                    out.push(crate::paths::storage(e.path())?)
                } else {
                    warnings.push(format!(
                        "{}: non-UTF-8 path cannot be indexed",
                        e.path().display()
                    ));
                }
            }
            Ok(_) => {}
            Err(e) => warnings.push(e.to_string()),
        }
    }
    out.sort();
    Ok(out)
}

fn is_apple_metadata(p: &Path) -> bool {
    p.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.starts_with("._") || n == ".DS_Store"
    })
}
fn is_media(p: &Path) -> bool {
    is_ext(p, "mxf") && !is_apple_metadata(p)
}

#[cfg(test)]
mod path_storage_tests {
    #[test]
    fn persisted_fallback_clip_ids_use_the_same_public_path_spelling() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for (path, expected) in [
            (r"\\?\C:\Sample Drive\a.mxf", r"file:C:\Sample Drive\a.mxf"),
            (r"\\?\UNC\server\share\a.mxf", r"file:\\server\share\a.mxf"),
        ] {
            let sql = format!("SELECT {}", super::clip_expression("?1", "?2"));
            let id: String = conn
                .query_row(&sql, rusqlite::params![path, "{}"], |r| r.get(0))
                .unwrap();
            assert_eq!(id, expected);
        }
    }
}
