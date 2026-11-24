//! Verified transfers with no-clobber publication and durable CSV journals.
use crate::{
    plan::{self, Action, Existing, Input, Operation, Plan},
    CancellationToken, Index, MediaFile,
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn stamp(meta: &fs::Metadata) -> String {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}
pub fn checksum(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn regular(path: &Path) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.file_type().is_file(),
        "not a regular file: {}",
        path.display()
    );
    Ok(meta)
}
/// Reject symlink ancestors, traversal, and deployment-configured protected paths.
pub fn safe_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "absolute paths required");
    ensure!(
        !path
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "parent traversal prohibited"
    );
    if let Some(protected) = std::env::var_os("MOBBOSS_PROTECTED_PATHS") {
        check_protected(
            path,
            std::env::split_paths(&protected).filter(|p| !p.as_os_str().is_empty()),
        )?;
    }
    for ancestor in path.ancestors() {
        if let Ok(m) = fs::symlink_metadata(ancestor) {
            ensure!(
                !m.file_type().is_symlink(),
                "symlink paths prohibited: {}",
                ancestor.display()
            );
        }
    }
    Ok(())
}
fn check_protected(path: &Path, roots: impl IntoIterator<Item = PathBuf>) -> Result<()> {
    for root in roots {
        ensure!(root.is_absolute(), "protected paths must be absolute");
        let root = root.canonicalize().unwrap_or(root);
        for ancestor in path.ancestors() {
            let candidate = ancestor
                .canonicalize()
                .unwrap_or_else(|_| ancestor.to_path_buf());
            #[cfg(windows)]
            let (candidate, root) = (
                PathBuf::from(candidate.to_string_lossy().to_lowercase()),
                PathBuf::from(root.to_string_lossy().to_lowercase()),
            );
            ensure!(
                !candidate.starts_with(&root),
                "path is protected by MOBBOSS_PROTECTED_PATHS"
            );
        }
    }
    Ok(())
}
fn existing_ancestor(path: &Path) -> Result<&Path> {
    path.ancestors()
        .find(|p| p.exists())
        .context("no destination ancestor")
}
/// An Avid tree's parent is a drive/workspace root, including temporary simulated drives.
pub fn media_root(file: &MediaFile) -> Result<PathBuf> {
    for p in file.path.ancestors() {
        if p.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("Avid MediaFiles"))
        {
            let root = p.parent().context("invalid media root")?;
            // Already collected media continues to belong to the original drive.
            if root
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == plan::MEDIA_BY_PROJECT)
            {
                return Ok(root
                    .parent()
                    .and_then(Path::parent)
                    .context("invalid collection root")?
                    .into());
            }
            return Ok(root.into());
        }
    }
    bail!(
        "No Avid MediaFiles ancestor for {}; choose an explicit destination root",
        file.path.display()
    )
}

pub fn prepare(
    index: &Index,
    op: Operation,
    project: &str,
    paths: &[PathBuf],
    destination: Option<&Path>,
) -> Result<Plan> {
    prepare_with(index, op, project, paths, destination, checksum)
}
fn prepare_with(
    index: &Index,
    op: Operation,
    project: &str,
    paths: &[PathBuf],
    destination: Option<&Path>,
    mut hash_file: impl FnMut(&Path) -> Result<String>,
) -> Result<Plan> {
    let all = index.files(None)?;
    let wanted: BTreeSet<_> = paths.iter().collect();
    ensure!(
        wanted.iter().all(|p| all.iter().any(|f| &f.path == *p)),
        "selection contains an unindexed file"
    );
    let files: Vec<_> = all
        .into_iter()
        .filter(|f| {
            if destination.is_some_and(|d| f.path.starts_with(plan::project_dir(d, project))) {
                return false;
            }
            if paths.is_empty() {
                f.metadata.project.as_deref().unwrap_or("Unknown") == project
            } else {
                wanted.contains(&f.path)
            }
        })
        .collect();
    let mut inputs = vec![];
    let mut roots = BTreeSet::new();
    if let Some(dest) = destination {
        safe_path(dest)?;
        roots.insert(dest.to_path_buf());
    }
    for file in files {
        safe_path(&file.path)?;
        let meta = regular(&file.path)?;
        ensure!(
            meta.len() == file.size && stamp(&meta) == file.modified_ns,
            "file changed since scan: {}; rescan",
            file.path.display()
        );
        let root = match media_root(&file) {
            Ok(root) => root,
            Err(error) => {
                let dest = destination.with_context(|| error.to_string())?;
                // Explicit Collect for an arbitrary folder collects directly into this root.
                dest.to_path_buf()
            }
        };
        roots.insert(root.clone());
        let volume = root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let hash = String::new();
        inputs.push(Input {
            file,
            root,
            volume,
            checksum: hash,
        });
    }
    let mut existing = BTreeMap::new();
    let mut space = BTreeMap::new();
    for root in roots {
        safe_path(&root)?;
        space.insert(
            root.clone(),
            fs2::available_space(existing_ancestor(&root)?)?,
        );
        let dir = plan::project_dir(&root, project);
        if dir.exists() {
            for entry in walkdir::WalkDir::new(&dir).follow_links(false) {
                let entry = entry?;
                ensure!(
                    !entry.file_type().is_symlink(),
                    "destination contains a symlink"
                );
                if entry.file_type().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("mxf"))
                {
                    existing.insert(
                        entry.path().into(),
                        Existing {
                            size: entry.metadata()?.len(),
                            checksum: String::new(),
                        },
                    );
                }
            }
        }
    }
    loop {
        let preview = plan::build(op, project, &inputs, destination, &existing, &space)?;
        if preview.checksum_requests.is_empty() {
            return Ok(preview);
        }
        for (source, dest) in &preview.checksum_requests {
            let input = inputs
                .iter_mut()
                .find(|i| i.file.path == *source)
                .context("unknown checksum source")?;
            if input.checksum.is_empty() {
                input.checksum = hash_file(source)?;
                restat(&input.file, source, true)?;
            }
            let entry = existing
                .get_mut(dest)
                .context("unknown checksum destination")?;
            if entry.checksum.is_empty() {
                entry.checksum = hash_file(dest)?;
            }
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRow {
    pub op: String,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub size: u64,
    pub checksum: String,
    pub status: String,
    #[serde(default)]
    pub guard: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub source: PathBuf,
}
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub logs: Vec<PathBuf>,
    pub completed: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub warnings: Vec<String>,
    pub touched: Vec<PathBuf>,
}
struct Journal {
    writers: Vec<(PathBuf, csv::Writer<File>)>,
}
impl Journal {
    fn new(dirs: &[PathBuf], app_data: &Path) -> Result<Self> {
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let name = format!("collect-log-{id}.csv");
        let mut dirs: BTreeSet<_> = dirs.iter().cloned().collect();
        dirs.insert(app_data.into());
        let mut writers = vec![];
        for dir in dirs {
            safe_path(&dir)?;
            fs::create_dir_all(&dir)?;
            let path = dir.join(&name);
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            writers.push((path, csv::Writer::from_writer(file)));
        }
        Ok(Self { writers })
    }
    fn row(&mut self, row: &LogRow) -> Result<()> {
        for (_, w) in &mut self.writers {
            w.serialize(row)?;
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        Ok(())
    }
    fn paths(&self) -> Vec<PathBuf> {
        self.writers.iter().map(|(p, _)| p.clone()).collect()
    }
}
fn restat(file: &MediaFile, source: &Path, check_stamp: bool) -> Result<fs::Metadata> {
    let meta = regular(source)?;
    ensure!(
        meta.len() == file.size && (!check_stamp || stamp(&meta) == file.modified_ns),
        "size/mtime changed since scan"
    );
    Ok(meta)
}
fn same_stat(before: &fs::Metadata, path: &Path) -> Result<()> {
    let after = regular(path)?;
    ensure!(
        before.len() == after.len() && stamp(before) == stamp(&after),
        "source size/mtime changed during transfer"
    );
    Ok(())
}
/// Atomic no-replace rename on supported desktop platforms. No check-then-rename race.
fn publish(source: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source_c = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let dest_c = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        // SAFETY: CString pointers remain live and are NUL terminated.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source_c.as_ptr(),
                libc::AT_FDCWD,
                dest_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        // SAFETY: CString pointers remain live and are NUL terminated.
        let rc = unsafe { libc::renamex_np(source_c.as_ptr(), dest_c.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            if rc == 0 {
                return Ok(());
            }
            Err(std::io::Error::last_os_error())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            fs::hard_link(source, dest)?;
            fs::remove_file(source)
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileW(source: *const u16, dest: *const u16) -> i32;
        }
        let source_w: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let dest_w: Vec<_> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: NUL terminated UTF-16 buffers remain live. MoveFileW refuses existing destinations.
        if unsafe { MoveFileW(source_w.as_ptr(), dest_w.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameMethod {
    Atomic,
    HardLink,
    Weak,
}
impl RenameMethod {
    fn guard(self) -> &'static str {
        match self {
            Self::Atomic => "atomic-noreplace",
            Self::HardLink => "hard-link-no-replace",
            Self::Weak => "check-then-rename (weaker guard)",
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
enum RenameResult {
    Renamed,
    Linked,
    CopyRequired,
}
fn cross_device(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(17)
    } // ERROR_NOT_SAME_DEVICE
}
fn unsupported_rename(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOSYS]
            .iter()
            .any(|code| error.raw_os_error() == Some(*code))
    }
    #[cfg(windows)]
    {
        [1, 50, 87]
            .iter()
            .any(|code| error.raw_os_error() == Some(*code))
    }
}
fn different_devices(source: &Path, destination_parent: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(regular(source)?.dev() != fs::metadata(destination_parent)?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination_parent);
        Ok(false)
    }
}
/// Injectable decision tree: errors other than EXDEV/device differences never trigger a copy.
fn rename_with(
    different: bool,
    mut attempt: impl FnMut(RenameMethod) -> std::io::Result<()>,
) -> std::io::Result<RenameResult> {
    match attempt(RenameMethod::Atomic) {
        Ok(()) => return Ok(RenameResult::Renamed),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(e) if cross_device(&e) || different => return Ok(RenameResult::CopyRequired),
        Err(e) if unsupported_rename(&e) => {}
        Err(e) => return Err(e),
    }
    match attempt(RenameMethod::HardLink) {
        Ok(()) => return Ok(RenameResult::Linked),
        Err(e) if cross_device(&e) => return Ok(RenameResult::CopyRequired),
        // Some SMB shares report EPERM for unsupported hard links.
        Err(e) if unsupported_rename(&e) || e.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(e) => return Err(e),
    }
    match attempt(RenameMethod::Weak) {
        Ok(()) => Ok(RenameResult::Renamed),
        Err(e) if cross_device(&e) => Ok(RenameResult::CopyRequired),
        Err(e) => Err(e),
    }
}
fn attempt_rename(method: RenameMethod, source: &Path, dest: &Path) -> std::io::Result<()> {
    match method {
        RenameMethod::Atomic => publish(source, dest),
        RenameMethod::HardLink => fs::hard_link(source, dest),
        RenameMethod::Weak => {
            if dest.try_exists()? {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
            fs::rename(source, dest)
        }
    }
}
fn rename_logged(
    source: &Path,
    dest: &Path,
    journal: &mut Journal,
    row: &mut LogRow,
) -> Result<RenameResult> {
    rename_logged_with(source, dest, journal, row, attempt_rename)
}
fn rename_logged_with(
    source: &Path,
    dest: &Path,
    journal: &mut Journal,
    row: &mut LogRow,
    mut attempt: impl FnMut(RenameMethod, &Path, &Path) -> std::io::Result<()>,
) -> Result<RenameResult> {
    let different = different_devices(source, dest.parent().context("no destination parent")?)?;
    let expected = regular(source)?;
    let result = rename_with(different, |method| {
        row.guard = method.guard().into();
        if method == RenameMethod::Weak {
            journal
                .row(row)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        same_stat(&expected, source).map_err(|e| std::io::Error::other(e.to_string()))?;
        attempt(method, source, dest)
    })?;
    Ok(result)
}
fn stream_copy(source: &mut impl Read, output: &mut impl Write) -> Result<(u64, String)> {
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut size = 0;
    loop {
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
        size += n as u64;
    }
    Ok((size, hash.finalize().to_hex().to_string()))
}
fn transfer(
    action: &Action,
    journal: &mut Journal,
    row: &mut LogRow,
    force_copy: bool,
) -> Result<()> {
    transfer_with(action, journal, row, force_copy, |_| Ok(()))
}
fn transfer_with(
    action: &Action,
    journal: &mut Journal,
    row: &mut LogRow,
    force_copy: bool,
    after_copy: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    safe_path(&action.source)?;
    safe_path(&action.dest)?;
    ensure!(
        [&action.source, &action.dest]
            .iter()
            .all(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxf"))),
        "transfers can only read and write MXF media"
    );
    let before = restat(
        &action.file,
        &action.source,
        action.source == action.file.path,
    )?;
    if action.duplicate {
        ensure!(
            !action.checksum.is_empty(),
            "duplicate has no verified checksum"
        );
        ensure!(
            checksum(&action.source)? == action.checksum
                && regular(&action.dest)?.len() == action.file.size
                && checksum(&action.dest)? == action.checksum,
            "duplicate source or destination changed"
        );
        same_stat(&before, &action.source)?;
        row.status = "duplicate".into();
        journal.row(row)?;
        return Ok(());
    }
    ensure!(
        !action.dest.try_exists()?,
        "destination appeared after planning; replan"
    );
    let parent = action.dest.parent().context("no parent")?;
    fs::create_dir_all(parent)?;
    row.status = "intent".into();
    // A rename does not need to read media. Empty checksum explicitly records that.
    if action.op == Operation::Move {
        row.checksum.clear();
    }
    journal.row(row)?;
    if action.op == Operation::Move && !force_copy {
        same_stat(&before, &action.source)?;
        match rename_logged(&action.source, &action.dest, journal, row)? {
            RenameResult::Renamed => {
                row.status = "done".into();
                journal.row(row)?;
                return Ok(());
            }
            RenameResult::Linked => {
                row.status = "published".into();
                journal.row(row)?;
                same_stat(&before, &action.source)?;
                fs::remove_file(&action.source)?;
                row.status = "done".into();
                journal.row(row)?;
                return Ok(());
            }
            RenameResult::CopyRequired => {}
        }
    }
    row.guard = "verified-copy".into();
    let partial = parent.join(format!(
        "{}.mobboss-partial",
        action
            .dest
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .context("partial file exists; inspect it before retrying")?;
    let result = (|| -> Result<()> {
        let mut source = File::open(&action.source)?;
        let (size, hash) = stream_copy(&mut source, &mut output)?;
        ensure!(size == action.file.size, "source size changed during copy");
        // If a collision/undo supplied a prior checksum, streaming also validates it.
        ensure!(
            action.checksum.is_empty() || hash == action.checksum,
            "source checksum changed"
        );
        output.flush()?;
        output.sync_all()?;
        after_copy(&partial)?;
        ensure!(
            regular(&partial)?.len() == size && checksum(&partial)? == hash,
            "copy verification failed; source retained"
        );
        same_stat(&before, &action.source)?;
        row.checksum = hash;
        match rename_logged(&partial, &action.dest, journal, row)? {
            RenameResult::Renamed => {}
            RenameResult::Linked => {
                fs::remove_file(&partial)?;
            }
            RenameResult::CopyRequired => bail!("partial unexpectedly on another filesystem"),
        }
        // Keep the publication guard while identifying this as a verified copy.
        row.guard = format!("verified-copy; {}", row.guard);
        row.status = "published".into();
        journal.row(row)?;
        if action.op == Operation::Move {
            same_stat(&before, &action.source)?;
            fs::remove_file(&action.source)?;
        }
        row.status = "done".into();
        journal.row(row)?;
        Ok(())
    })();
    drop(output);
    let _ = fs::remove_file(partial);
    result
}
pub fn execute(
    index: &mut Index,
    plan: &Plan,
    app_data: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(Progress),
) -> Result<Outcome> {
    ensure!(
        plan.drives.iter().all(|d| d.available >= d.required),
        "insufficient free space; cannot execute"
    );
    // Validate every source against the current index, never trust a serialized GUI plan.
    let indexed = index.files(None)?;
    for a in &plan.actions {
        ensure!(
            indexed.iter().any(|f| f.path == a.file.path
                && f.size == a.file.size
                && f.modified_ns == a.file.modified_ns),
            "plan no longer matches the index"
        );
        safe_path(&a.source)?;
        safe_path(&a.dest)?;
    }
    for d in &plan.drives {
        ensure!(
            fs2::available_space(existing_ancestor(&d.root)?)? >= d.required,
            "free space changed; replan"
        );
    }
    let mut journal = Journal::new(&plan.log_dirs, app_data)?;
    let mut outcome = Outcome {
        logs: journal.paths(),
        completed: 0,
        skipped: 0,
        cancelled: false,
        warnings: vec!["Avid will rebuild its media databases on next launch".into()],
        touched: vec![],
    };
    let mut failed = BTreeSet::new();
    for (n, a) in plan.actions.iter().enumerate() {
        if cancel.is_cancelled() {
            outcome.cancelled = true;
            break;
        }
        progress(Progress {
            done: n,
            total: plan.actions.len(),
            source: a.source.clone(),
        });
        let mut row = LogRow {
            op: if a.op == Operation::Move {
                "move"
            } else {
                "copy"
            }
            .into(),
            source: a.source.clone(),
            dest: a.dest.clone(),
            size: a.file.size,
            checksum: a.checksum.clone(),
            status: String::new(),
            guard: String::new(),
        };
        let result = if a.step > 0 && failed.contains(&a.file.path) {
            Err(anyhow::anyhow!("dependent collect step skipped"))
        } else {
            transfer(a, &mut journal, &mut row, false)
        };
        if row.guard.contains("weaker guard") {
            outcome.warnings.push(format!(
                "{}: used check-then-rename with a weaker overwrite guard",
                a.dest.display()
            ));
        }
        match result {
            Ok(()) if a.duplicate => outcome.skipped += 1,
            Ok(()) => {
                outcome.completed += 1;
                index.record_transfer(&a.file, &a.source, &a.dest, a.op == Operation::Move)?;
                for p in [&a.source, &a.dest] {
                    let folder = p.parent().unwrap().to_path_buf();
                    if !outcome.touched.contains(&folder) {
                        outcome.touched.push(folder);
                    }
                }
            }
            Err(e) => {
                failed.insert(a.file.path.clone());
                outcome.skipped += 1;
                outcome
                    .warnings
                    .push(format!("{}: {e}", a.source.display()));
                if row.status == "published" || row.status == "done" {
                    // Preserve recovery information if a failure happens after publication.
                    let verified = if row.checksum.is_empty() {
                        regular(&a.dest).is_ok_and(|m| {
                            m.len() == a.file.size && stamp(&m) == a.file.modified_ns
                        })
                    } else {
                        checksum(&a.dest).is_ok_and(|hash| hash == row.checksum)
                    };
                    if verified {
                        if let Err(error) =
                            index.record_transfer(&a.file, &a.source, &a.dest, !a.source.exists())
                        {
                            outcome
                                .warnings
                                .push(format!("Could not reconcile index: {error}"));
                        }
                    }
                } else {
                    row.status = format!("skipped: {e}");
                }
                journal.row(&row)?;
            }
        }
    }
    progress(Progress {
        done: outcome.completed + outcome.skipped,
        total: plan.actions.len(),
        source: PathBuf::new(),
    });
    Ok(outcome)
}
pub fn read_log(path: &Path) -> Result<Vec<LogRow>> {
    Ok(csv::Reader::from_path(path)?
        .deserialize()
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
#[derive(Debug, Clone, Serialize)]
pub struct UndoPreview {
    pub rows: Vec<LogRow>,
    pub drives: Vec<plan::DriveSummary>,
    pub warnings: Vec<String>,
}
fn undo_rows(log: &Path) -> Result<Vec<LogRow>> {
    let mut latest = BTreeMap::new();
    for (n, row) in read_log(log)?.into_iter().enumerate() {
        latest.insert((row.source.clone(), row.dest.clone()), (n, row));
    }
    let mut rows: Vec<_> = latest.into_values().collect();
    rows.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    Ok(rows
        .into_iter()
        .map(|(_, r)| r)
        .filter(|r| r.status == "done" || r.status == "published")
        .collect())
}
/// Preview undo in execution order, including conservative move-back space budgets.
pub fn preview_undo(index: &Index, log: &Path) -> Result<UndoPreview> {
    let rows = undo_rows(log)?;
    let indexed = index.files(None)?;
    let mut warnings = vec![
        "Quit Media Composer before moving media".into(),
        "Copies are deleted only if their checksum still matches".into(),
    ];
    let mut roots: BTreeMap<PathBuf, (usize, u64)> = BTreeMap::new();
    for row in &rows {
        safe_path(&row.source)?;
        safe_path(&row.dest)?;
        ensure!(
            [&row.source, &row.dest]
                .iter()
                .all(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxf"))),
            "undo logs must contain only MXF paths"
        );
        ensure!(
            row.op == "copy" || row.op == "move",
            "this log is not a reversible transfer log"
        );
        if !indexed.iter().any(|f| f.path == row.dest) {
            warnings.push(format!(
                "Not currently indexed; will skip unless restored by a preceding undo step: {}",
                row.dest.display()
            ));
        }
        if row.op == "move" {
            let root = row
                .source
                .parent()
                .context("no original folder")?
                .to_path_buf();
            let summary = roots.entry(root).or_default();
            summary.0 += 1;
            summary.1 = summary.1.saturating_add(row.size);
            if row.source.exists() {
                warnings.push(format!(
                    "Original path occupied; move-back will be skipped: {}",
                    row.source.display()
                ));
            }
        }
    }
    let mut drives = vec![];
    for (root, (files, bytes)) in roots {
        let available = fs2::available_space(existing_ancestor(&root)?)?;
        let required = bytes
            .saturating_add(bytes / 20)
            .saturating_add(plan::SPACE_MARGIN);
        if available < required {
            warnings.push(format!(
                "Insufficient free space for undo at {}",
                root.display()
            ));
        }
        drives.push(plan::DriveSummary {
            root,
            files,
            bytes,
            required,
            available,
        });
    }
    Ok(UndoPreview {
        rows,
        drives,
        warnings,
    })
}
/// Undo only verified media still represented in this index. Reverse order handles Collect.
pub fn undo(
    index: &mut Index,
    log: &Path,
    app_data: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(Progress),
) -> Result<Outcome> {
    let preview = preview_undo(index, log)?;
    ensure!(
        preview.drives.iter().all(|d| d.available >= d.required),
        "insufficient free space for undo"
    );
    let rows = preview.rows;
    let mut log_dirs = BTreeSet::new();
    for row in &rows {
        if let Some(avid) = row.dest.ancestors().find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("Avid MediaFiles"))
        }) {
            if let Some(project) = avid.parent() {
                if project
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == plan::MEDIA_BY_PROJECT)
                {
                    log_dirs.insert(project.to_path_buf());
                }
            }
        }
    }
    let mut journal = Journal::new(&log_dirs.into_iter().collect::<Vec<_>>(), app_data)?;
    let mut outcome = Outcome {
        logs: journal.paths(),
        completed: 0,
        skipped: 0,
        cancelled: false,
        warnings: vec!["Avid will rebuild its media databases on next launch".into()],
        touched: vec![],
    };
    let total = rows.len();
    for (n, row) in rows.into_iter().enumerate() {
        if cancel.is_cancelled() {
            outcome.cancelled = true;
            break;
        }
        if row.status != "done" && row.status != "published" {
            continue;
        }
        progress(Progress {
            done: n,
            total,
            source: row.dest.clone(),
        });
        let result = (|| -> Result<()> {
            safe_path(&row.source)?;
            safe_path(&row.dest)?;
            ensure!(
                row.dest
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("mxf")),
                "undo requires MXF media"
            );
            let file = index
                .files(None)?
                .into_iter()
                .find(|f| f.path == row.dest)
                .context("undo destination is not indexed")?;
            let before = restat(&file, &row.dest, true)?;
            if row.op == "copy" {
                ensure!(
                    !row.checksum.is_empty() && checksum(&row.dest)? == row.checksum,
                    "copy checksum changed"
                );
                same_stat(&before, &row.dest)?;
            }
            let mut undo_row = LogRow {
                op: "undo".into(),
                source: row.dest.clone(),
                dest: row.source.clone(),
                size: row.size,
                checksum: row.checksum.clone(),
                status: "intent".into(),
                guard: String::new(),
            };
            if row.op == "copy" {
                journal.row(&undo_row)?;
                fs::remove_file(&row.dest)?;
                index.forget_path(&row.dest)?;
            } else if row.op == "move" {
                ensure!(
                    !row.source.try_exists()?,
                    "original path occupied; undo refuses overwrite"
                );
                let action = Action {
                    op: Operation::Move,
                    source: row.dest.clone(),
                    dest: row.source.clone(),
                    file: file.clone(),
                    checksum: row.checksum.clone(),
                    duplicate: false,
                    step: 1,
                };
                transfer(&action, &mut journal, &mut undo_row, false)?;
                index.record_transfer(&file, &row.dest, &row.source, true)?;
            } else {
                bail!("unsupported log operation");
            }
            undo_row.status = "done".into();
            journal.row(&undo_row)?;
            for p in [&row.source, &row.dest] {
                let folder = p.parent().unwrap().to_path_buf();
                if !outcome.touched.contains(&folder) {
                    outcome.touched.push(folder);
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => outcome.completed += 1,
            Err(e) => {
                outcome.skipped += 1;
                outcome
                    .warnings
                    .push(format!("{}: {e}", row.dest.display()));
            }
        }
    }
    Ok(outcome)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_checksum_never_publishes_or_removes_source() {
        assert_corrupt_copy_retains_source(None);
    }
    #[cfg(unix)]
    #[test]
    fn cross_filesystem_corrupt_copy_retains_source() {
        let Some(second) = std::env::var_os("MOBBOSS_CROSS_FS") else {
            eprintln!("set MOBBOSS_CROSS_FS to exercise cross-filesystem verification failure");
            return;
        };
        assert_corrupt_copy_retains_source(Some(Path::new(&second)));
    }
    fn assert_corrupt_copy_retains_source(destination_root: Option<&Path>) {
        let dir = tempfile::tempdir().unwrap();
        let other = destination_root.map(|p| tempfile::tempdir_in(p).unwrap());
        let source = dir.path().join("source.mxf");
        let dest = other.as_ref().unwrap_or(&dir).path().join("dest.mxf");
        #[cfg(unix)]
        if let Some(other) = &other {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(
                fs::metadata(dir.path()).unwrap().dev(),
                fs::metadata(other.path()).unwrap().dev()
            );
        }
        fs::write(&source, "original").unwrap();
        let file = MediaFile {
            path: source.clone(),
            folder: dir.path().into(),
            size: 8,
            modified_ns: stamp(&fs::metadata(&source).unwrap()),
            metadata: Default::default(),
        };
        let action = Action {
            op: Operation::Move,
            source: source.clone(),
            dest: dest.clone(),
            file,
            checksum: checksum(&source).unwrap(),
            duplicate: false,
            step: 0,
        };
        let mut journal = Journal::new(&[], &dir.path().join("logs")).unwrap();
        let mut row = LogRow {
            op: "move".into(),
            source: source.clone(),
            dest: dest.clone(),
            size: 8,
            checksum: action.checksum.clone(),
            status: String::new(),
            guard: String::new(),
        };
        assert!(transfer_with(
            &action,
            &mut journal,
            &mut row,
            other.is_none(),
            |partial| {
                // The source must still exist while the destination awaits verification.
                assert_eq!(checksum(&source)?, action.checksum);
                assert_eq!(checksum(partial)?, action.checksum);
                assert!(!dest.exists());
                // Same size, different bytes: this must fail the checksum, not just size.
                fs::write(partial, "bad copy")?;
                Ok(())
            }
        )
        .is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "original");
        assert!(!dest.exists());
        assert!(!dest.with_file_name("dest.mxf.mobboss-partial").exists());
        assert_eq!(row.status, "intent");
        assert!(read_log(&journal.paths()[0])
            .unwrap()
            .iter()
            .all(|r| r.status != "published" && r.status != "done"));
    }
    #[test]
    fn atomic_publication_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a");
        let dest = dir.path().join("b");
        fs::write(&source, "source").unwrap();
        fs::write(&dest, "destination").unwrap();
        assert!(publish(&source, &dest).is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(fs::read_to_string(dest).unwrap(), "destination");
    }
    #[test]
    fn protected_roots_are_configurable_without_a_developer_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("protected");
        fs::create_dir(&root).unwrap();
        assert!(check_protected(&root.join("file.mxf"), [root.clone()]).is_err());
        assert!(
            check_protected(&dir.path().join("protected-other/file.mxf"), [root.clone()]).is_ok()
        );
        #[cfg(unix)]
        {
            let alias = dir.path().join("alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            assert!(check_protected(&alias.join("not-yet-created/file.mxf"), [root]).is_err());
        }
    }
    fn indexed_fixture(dir: &Path) -> (Index, MediaFile) {
        let path = dir.join("drive/Avid MediaFiles/MXF/1/a.mxf");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "original media").unwrap();
        let file = MediaFile {
            path: path.clone(),
            folder: path.parent().unwrap().into(),
            size: 14,
            modified_ns: stamp(&fs::metadata(&path).unwrap()),
            metadata: crate::MediaMetadata {
                project: Some("Film".into()),
                ..Default::default()
            },
        };
        let mut index = Index::open(&dir.join("data/index.sqlite")).unwrap();
        index.record_transfer(&file, &path, &path, false).unwrap();
        (index, file)
    }
    #[test]
    fn planning_reads_no_media_when_no_destination_conflicts_exist() {
        let dir = tempfile::tempdir().unwrap();
        let (index, _) = indexed_fixture(dir.path());
        let p = prepare_with(
            &index,
            Operation::Copy,
            "Film",
            &[],
            Some(&dir.path().join("shuttle")),
            |_| panic!("plan read media without a collision"),
        )
        .unwrap();
        assert_eq!(p.actions.len(), 1);
        assert!(p.actions[0].checksum.is_empty());
    }
    #[test]
    fn planning_hashes_only_a_matching_size_collision_pair() {
        let dir = tempfile::tempdir().unwrap();
        let (index, file) = indexed_fixture(dir.path());
        let dest = dir.path().join("shuttle");
        let target = plan::project_dir(&dest, "Film").join("Avid MediaFiles/MXF/1/a.mxf");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(&file.path, &target).unwrap();
        fs::write(target.with_file_name("unrelated.mxf"), "do not hash this").unwrap();
        let mut reads = vec![];
        let p = prepare_with(&index, Operation::Copy, "Film", &[], Some(&dest), |p| {
            reads.push(p.to_path_buf());
            checksum(p)
        })
        .unwrap();
        assert!(p.actions[0].duplicate);
        assert_eq!(reads, vec![file.path, target]);
    }
    #[test]
    fn unknown_avid_ancestry_requires_destination_for_every_operation() {
        let dir = tempfile::tempdir().unwrap();
        let (mut index, file) = indexed_fixture(dir.path());
        let arbitrary = dir.path().join("arbitrary/a.mxf");
        fs::create_dir_all(arbitrary.parent().unwrap()).unwrap();
        fs::rename(&file.path, &arbitrary).unwrap();
        index
            .record_transfer(&file, &file.path, &arbitrary, true)
            .unwrap();
        for op in [Operation::Move, Operation::Copy, Operation::Collect] {
            let error = prepare(&index, op, "Film", &[], None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("explicit destination"), "{error}");
            let p = prepare(&index, op, "Film", &[], Some(&dir.path().join("chosen"))).unwrap();
            assert!(p
                .actions
                .iter()
                .all(|a| a.dest.starts_with(dir.path().join("chosen"))));
        }
    }
    #[test]
    fn streaming_copy_reads_source_once_and_hashes_the_written_bytes() {
        struct Once {
            data: std::io::Cursor<Vec<u8>>,
            bytes: usize,
        }
        impl Read for Once {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.data.read(buf)?;
                self.bytes += n;
                Ok(n)
            }
        }
        let data = vec![7; 3 * 1024 * 1024 + 17];
        let mut source = Once {
            data: std::io::Cursor::new(data.clone()),
            bytes: 0,
        };
        let mut output = vec![];
        let (size, hash) = stream_copy(&mut source, &mut output).unwrap();
        assert_eq!(source.bytes, data.len());
        assert_eq!(size, data.len() as u64);
        assert_eq!(output, data);
        assert_eq!(hash, blake3::hash(&output).to_hex().to_string());
    }
    #[cfg(unix)]
    #[test]
    fn rename_fallbacks_distinguish_unsupported_cross_device_and_fatal_errors() {
        use std::io::{Error, ErrorKind};
        for code in [libc::EINVAL, libc::ENOTSUP, libc::ENOSYS] {
            let mut calls = vec![];
            let result = rename_with(false, |method| {
                calls.push(method);
                if method == RenameMethod::Atomic {
                    Err(Error::from_raw_os_error(code))
                } else {
                    Ok(())
                }
            })
            .unwrap();
            assert_eq!(result, RenameResult::Linked);
            assert_eq!(calls, vec![RenameMethod::Atomic, RenameMethod::HardLink]);
        }
        let mut calls = vec![];
        assert_eq!(
            rename_with(false, |method| {
                calls.push(method);
                Err(Error::from_raw_os_error(libc::EXDEV))
            })
            .unwrap(),
            RenameResult::CopyRequired
        );
        assert_eq!(calls, vec![RenameMethod::Atomic]);
        assert_eq!(
            rename_with(true, |_| Err(Error::from_raw_os_error(libc::EINVAL))).unwrap(),
            RenameResult::CopyRequired
        );
        for code in [libc::EACCES, libc::EIO, libc::EEXIST] {
            let mut calls = vec![];
            assert!(rename_with(false, |method| {
                calls.push(method);
                Err(Error::from_raw_os_error(code))
            })
            .is_err());
            assert_eq!(calls, vec![RenameMethod::Atomic]);
        }
        let mut calls = vec![];
        assert!(rename_with(false, |method| {
            calls.push(method);
            if method == RenameMethod::Atomic {
                Err(Error::from_raw_os_error(libc::EINVAL))
            } else {
                Err(Error::from(ErrorKind::AlreadyExists))
            }
        })
        .is_err());
        assert_eq!(calls, vec![RenameMethod::Atomic, RenameMethod::HardLink]);
    }
    #[cfg(unix)]
    #[test]
    fn weaker_rename_guard_is_journaled_before_rename_and_never_copies() {
        let dir = tempfile::tempdir().unwrap();
        let (_, file) = indexed_fixture(dir.path());
        let dest = dir.path().join("dest.mxf");
        let mut journal = Journal::new(&[], &dir.path().join("logs")).unwrap();
        let log = journal.paths()[0].clone();
        let mut row = LogRow {
            op: "move".into(),
            source: file.path.clone(),
            dest: dest.clone(),
            size: file.size,
            checksum: String::new(),
            status: "intent".into(),
            guard: String::new(),
        };
        let result = rename_logged_with(
            &file.path,
            &dest,
            &mut journal,
            &mut row,
            |method, source, dest| {
                if method != RenameMethod::Weak {
                    return Err(std::io::Error::from_raw_os_error(libc::ENOTSUP));
                }
                let rows = read_log(&log).unwrap();
                assert!(rows.last().unwrap().guard.contains("weaker guard"));
                assert!(source.exists());
                assert!(!dest.exists());
                attempt_rename(method, source, dest)
            },
        )
        .unwrap();
        assert_eq!(result, RenameResult::Renamed);
        assert!(!file.path.exists());
        assert_eq!(fs::read_to_string(dest).unwrap(), "original media");
    }
    #[cfg(unix)]
    #[test]
    fn same_device_rename_succeeds_without_read_permission_or_a_checksum() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (_, file) = indexed_fixture(dir.path());
        let dest = dir.path().join("dest.mxf");
        fs::set_permissions(&file.path, fs::Permissions::from_mode(0o000)).unwrap();
        let action = Action {
            op: Operation::Move,
            source: file.path.clone(),
            dest: dest.clone(),
            file: file.clone(),
            checksum: String::new(),
            duplicate: false,
            step: 0,
        };
        let mut row = LogRow {
            op: "move".into(),
            source: file.path,
            dest: dest.clone(),
            size: file.size,
            checksum: String::new(),
            status: String::new(),
            guard: String::new(),
        };
        let mut journal = Journal::new(&[], &dir.path().join("logs")).unwrap();
        transfer(&action, &mut journal, &mut row, false).unwrap();
        assert!(row.checksum.is_empty());
        assert_eq!(row.guard, "atomic-noreplace");
        fs::set_permissions(dest, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn legacy_six_column_logs_remain_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.csv");
        fs::write(
            &path,
            "op,source,dest,size,checksum,status\ncopy,/source.mxf,/dest.mxf,1,abc,done\n",
        )
        .unwrap();
        let rows = read_log(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].guard.is_empty());
        assert_eq!(rows[0].checksum, "abc");
    }
}
