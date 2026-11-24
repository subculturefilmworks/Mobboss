use mobboss_core::{
    ops,
    plan::{Operation, Plan},
    CancellationToken, Index, MediaFile, MediaMetadata,
};
use std::{fs, path::Path};
use tempfile::TempDir;
struct Fixture {
    _dir: TempDir,
    root: std::path::PathBuf,
    dest: std::path::PathBuf,
    data: std::path::PathBuf,
    index: Index,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("drive");
        let dest = dir.path().join("shuttle");
        let data = dir.path().join("app-data");
        let mut index = Index::open(&data.join("index.sqlite")).unwrap();
        for n in 0..3 {
            let path = root.join(format!("Avid MediaFiles/MXF/1/{n}.mxf"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, format!("media {n}")).unwrap();
            let f = MediaFile {
                path: path.clone(),
                folder: path.parent().unwrap().into(),
                size: 7,
                modified_ns: ops::stamp(&fs::metadata(&path).unwrap()),
                metadata: MediaMetadata {
                    project: Some("Test".into()),
                    ..Default::default()
                },
            };
            index.record_transfer(&f, &path, &path, false).unwrap();
        }
        fs::write(
            root.join("Avid MediaFiles/MXF/1/msmFMID.pmr"),
            "database stays",
        )
        .unwrap();
        Self {
            _dir: dir,
            root,
            dest,
            data,
            index,
        }
    }
    fn plan(&self, op: Operation) -> Plan {
        ops::prepare(&self.index, op, "Test", &[], Some(&self.dest)).unwrap()
    }
    fn execute(&mut self, p: &Plan) -> ops::Outcome {
        ops::execute(
            &mut self.index,
            p,
            &self.data,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap()
    }
}
fn tree(root: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(root).unwrap().into(),
                ops::checksum(e.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}
#[test]
fn collect_and_undo_restore_exact_media_and_keep_databases() {
    let mut f = Fixture::new();
    let before = tree(&f.root);
    let p = f.plan(Operation::Collect);
    let out = f.execute(&p);
    assert_eq!(out.completed, 6);
    assert_eq!(f.index.files(None).unwrap().len(), 6);
    assert!(f.root.join("Avid MediaFiles/MXF/1/msmFMID.pmr").exists());
    let log = out.logs.iter().find(|p| p.starts_with(&f.data)).unwrap();
    let undo = ops::undo(
        &mut f.index,
        log,
        &f.data,
        &CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(undo.completed, 6, "{:?}", undo.warnings);
    let after: Vec<_> = tree(&f.root)
        .into_iter()
        .filter(|(p, _)| !p.to_string_lossy().ends_with(".csv"))
        .collect();
    assert_eq!(before, after);
    assert_eq!(f.index.files(None).unwrap().len(), 3);
}
#[test]
fn destination_race_never_overwrites_or_deletes_source() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Move);
    let a = &p.actions[0];
    fs::create_dir_all(a.dest.parent().unwrap()).unwrap();
    fs::write(&a.dest, "other content").unwrap();
    let out = f.execute(&p);
    assert_eq!(out.skipped, 1);
    assert!(a.source.exists());
    assert_eq!(fs::read_to_string(&a.dest).unwrap(), "other content");
}
#[test]
fn changed_source_is_skipped() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Move);
    fs::write(&p.actions[0].source, "changed").unwrap();
    let out = f.execute(&p);
    assert_eq!(out.skipped, 1);
    assert!(p.actions[0].source.exists());
    assert!(!p.actions[0].dest.exists());
}
#[test]
fn cancellation_finishes_current_file() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    let cancel = CancellationToken::default();
    let out = ops::execute(&mut f.index, &p, &f.data, &cancel, |_| cancel.cancel()).unwrap();
    assert_eq!(out.completed, 1);
    assert!(out.cancelled);
    assert_eq!(
        ops::checksum(&p.actions[0].source).unwrap(),
        ops::checksum(&p.actions[0].dest).unwrap()
    );
    assert!(!tree(&f.dest)
        .iter()
        .any(|(p, _)| p.to_string_lossy().contains("partial")));
}
#[test]
fn duplicate_move_keeps_source_and_logs_skip() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    f.execute(&p);
    let p = f.plan(Operation::Copy);
    assert!(p.actions.iter().all(|a| a.duplicate));
    let out = f.execute(&p);
    assert_eq!(out.skipped, 3);
    assert!(p.actions[0].source.exists());
}
#[test]
fn undo_refuses_modified_copy() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    let out = f.execute(&p);
    fs::write(&p.actions[0].dest, "edited!").unwrap();
    let out = ops::undo(
        &mut f.index,
        &out.logs[0],
        &f.data,
        &CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(out.skipped, 1);
    assert!(p.actions[0].dest.exists());
}
#[test]
fn unindexed_selection_and_sample_are_rejected() {
    let f = Fixture::new();
    assert!(ops::prepare(
        &f.index,
        Operation::Move,
        "Test",
        &[f.root.join("unknown.mxf")],
        None
    )
    .is_err());
    assert!(ops::safe_path(Path::new("<sample-folder>/new")).is_err());
}
#[test]
fn stale_partial_is_never_overwritten() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    let a = &p.actions[0];
    fs::create_dir_all(a.dest.parent().unwrap()).unwrap();
    let partial = a.dest.with_file_name(format!(
        "{}.mobboss-partial",
        a.dest.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&partial, "recover me").unwrap();
    let out = f.execute(&p);
    assert_eq!(out.skipped, 1);
    assert_eq!(fs::read_to_string(partial).unwrap(), "recover me");
    assert!(a.source.exists());
    assert!(!a.dest.exists());
}
#[test]
fn undo_move_never_overwrites_an_occupied_original_path() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Move);
    let out = f.execute(&p);
    fs::write(&p.actions[0].source, "new work").unwrap();
    let out = ops::undo(
        &mut f.index,
        &out.logs[0],
        &f.data,
        &CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(out.skipped, 1);
    assert_eq!(
        fs::read_to_string(&p.actions[0].source).unwrap(),
        "new work"
    );
    assert!(p.actions[0].dest.exists());
}
#[cfg(unix)]
#[test]
fn symlink_destinations_are_rejected() {
    let f = Fixture::new();
    let alias = f._dir.path().join("alias");
    std::os::unix::fs::symlink(&f.root, &alias).unwrap();
    assert!(ops::prepare(&f.index, Operation::Copy, "Test", &[], Some(&alias)).is_err());
}
#[cfg(unix)]
#[test]
fn cross_filesystem_move_verifies_before_delete_and_undo_restores() {
    use std::os::unix::fs::MetadataExt;
    let Some(second) = std::env::var_os("MOBBOSS_CROSS_FS") else {
        eprintln!("set MOBBOSS_CROSS_FS to exercise a second filesystem");
        return;
    };
    let other = tempfile::tempdir_in(second).unwrap();
    let mut f = Fixture::new();
    assert_ne!(
        fs::metadata(&f.root).unwrap().dev(),
        fs::metadata(other.path()).unwrap().dev()
    );
    let before = tree(&f.root);
    let p = ops::prepare(&f.index, Operation::Move, "Test", &[], Some(other.path())).unwrap();
    let out = f.execute(&p);
    assert_eq!(out.completed, 3, "{:?}", out.warnings);
    for a in &p.actions {
        assert!(!a.source.exists());
        assert_eq!(ops::checksum(&a.dest).unwrap(), a.checksum);
    }
    // A durable published (verified) entry precedes done (source removed) for every cross-device move.
    let log = out.logs.iter().find(|p| p.starts_with(&f.data)).unwrap();
    let rows = ops::read_log(log).unwrap();
    for a in &p.actions {
        let states: Vec<_> = rows
            .iter()
            .filter(|r| r.source == a.source)
            .map(|r| r.status.as_str())
            .collect();
        assert_eq!(states, vec!["intent", "published", "done"]);
    }
    let undo = ops::undo(
        &mut f.index,
        log,
        &f.data,
        &CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(undo.completed, 3, "{:?}", undo.warnings);
    assert_eq!(tree(&f.root), before);
}
#[test]
fn insufficient_space_refuses_before_any_destination_write() {
    let mut f = Fixture::new();
    let mut p = f.plan(Operation::Copy);
    p.drives[0].available = 0;
    assert!(ops::execute(
        &mut f.index,
        &p,
        &f.data,
        &CancellationToken::default(),
        |_| {}
    )
    .is_err());
    assert!(!f.dest.exists());
    assert!(p.actions.iter().all(|a| a.source.exists()));
}
#[test]
fn arbitrary_indexed_mxf_folders_can_copy_to_an_explicit_root() {
    let mut f = Fixture::new();
    let old = f.index.files(None).unwrap().remove(0);
    let source = f._dir.path().join("arbitrary/clip.mxf");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::copy(&old.path, &source).unwrap();
    f.index
        .record_transfer(&old, &old.path, &source, false)
        .unwrap();
    let p = ops::prepare(
        &f.index,
        Operation::Copy,
        "Selection",
        &[source],
        Some(&f.dest),
    )
    .unwrap();
    let out = f.execute(&p);
    assert_eq!(out.completed, 1);
}
#[test]
fn forged_undo_cannot_write_an_avid_database() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    let out = f.execute(&p);
    let mut row = ops::read_log(&out.logs[0])
        .unwrap()
        .into_iter()
        .find(|r| r.status == "done")
        .unwrap();
    row.op = "move".into();
    row.source = f.root.join("Avid MediaFiles/MXF/1/new.mdb");
    let log = f.data.join("forged.csv");
    let mut w = csv::Writer::from_path(&log).unwrap();
    w.serialize(&row).unwrap();
    w.flush().unwrap();
    assert!(ops::undo(
        &mut f.index,
        &log,
        &f.data,
        &CancellationToken::default(),
        |_| {}
    )
    .is_err());
    assert!(!row.source.exists());
    assert!(row.dest.exists());
}
