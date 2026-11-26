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
    let log = out
        .logs
        .iter()
        .find(|p| p.starts_with(mobboss_core::paths::canonical(&f.data).unwrap()))
        .unwrap();
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
    assert!(out.rows.iter().all(|r| r.verified == "matched existing"
        && r.algorithm == "BLAKE3"
        && !r.checksum.is_empty()));
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
fn unindexed_selection_is_rejected() {
    let f = Fixture::new();
    assert!(ops::prepare(
        &f.index,
        Operation::Move,
        "Test",
        &[f.root.join("unknown.mxf")],
        None
    )
    .is_err());
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
fn symlinks_inside_destination_trees_are_rejected() {
    let f = Fixture::new();
    let outside = f._dir.path().join("outside");
    fs::create_dir_all(outside.join("Test")).unwrap();
    fs::create_dir_all(&f.dest).unwrap();
    std::os::unix::fs::symlink(&outside, f.dest.join("Media By Project")).unwrap();
    assert!(ops::prepare(&f.index, Operation::Copy, "Test", &[], Some(&f.dest)).is_err());
}
#[cfg(unix)]
#[test]
fn a_selected_volume_alias_is_resolved_before_copy_and_undo() {
    let mut f = Fixture::new();
    let actual = f._dir.path().join("private-volume");
    fs::create_dir(&actual).unwrap();
    let alias = f._dir.path().join("volume-alias");
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let p = ops::prepare(&f.index, Operation::Copy, "Test", &[], Some(&alias)).unwrap();
    let actual = mobboss_core::paths::canonical(&actual).unwrap();
    assert!(p.actions.iter().all(|a| a.dest.starts_with(&actual)));
    let out = f.execute(&p);
    assert_eq!(out.completed, 3);
    assert_eq!(
        ops::undo(
            &mut f.index,
            &out.logs[0],
            &f.data,
            &CancellationToken::default(),
            |_| {}
        )
        .unwrap()
        .completed,
        3
    );
    assert!(p
        .actions
        .iter()
        .all(|a| a.source.exists() && !a.dest.exists()));
}
#[cfg(unix)]
#[test]
fn a_symlink_inserted_inside_the_indexed_source_stays_prohibited() {
    let f = Fixture::new();
    let folder = f.root.join("Avid MediaFiles/MXF/1");
    let actual = folder.with_file_name("original");
    fs::rename(&folder, &actual).unwrap();
    std::os::unix::fs::symlink(&actual, &folder).unwrap();
    let error = ops::prepare(&f.index, Operation::Copy, "Test", &[], Some(&f.dest)).unwrap_err();
    assert!(error.to_string().contains("symlink paths prohibited"));
    assert!(!f.dest.exists());
}
#[test]
fn legacy_journal_spellings_resolve_against_canonical_index_paths() {
    let mut f = Fixture::new();
    let p = f.plan(Operation::Copy);
    let out = f.execute(&p);
    let row = ops::read_log(&out.logs[0])
        .unwrap()
        .into_iter()
        .find(|r| r.status == "done")
        .unwrap();
    #[cfg(unix)]
    let base = mobboss_core::paths::canonical(f._dir.path()).unwrap();
    #[cfg(unix)]
    let legacy_base = {
        let alias = base.join("legacy-parent");
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        alias
    };
    let legacy = |path: &Path| {
        #[cfg(windows)]
        {
            format!(
                r"\\?\{}",
                path.to_string_lossy().to_uppercase().replace('\\', "/")
            )
        }
        #[cfg(unix)]
        {
            legacy_base
                .join(path.strip_prefix(&base).unwrap())
                .to_string_lossy()
                .into_owned()
        }
    };
    let log = f.data.join("legacy.csv");
    let mut w = csv::Writer::from_path(&log).unwrap();
    w.write_record([
        "op", "source", "dest", "size", "checksum", "status", "guard",
    ])
    .unwrap();
    for (source, dest, status) in [
        (
            row.source.to_string_lossy().into_owned(),
            row.dest.to_string_lossy().into_owned(),
            "published",
        ),
        (legacy(&row.source), legacy(&row.dest), "done"),
    ] {
        w.write_record([
            row.op.clone(),
            source,
            dest,
            row.size.to_string(),
            row.checksum.clone(),
            status.into(),
            row.guard.clone(),
        ])
        .unwrap();
    }
    w.flush().unwrap();
    let legacy_rows = ops::read_log(&log).unwrap();
    assert!(legacy_rows
        .iter()
        .all(|r| r.verified.is_empty() && r.algorithm.is_empty()));
    let preview = ops::preview_undo(&f.index, &log).unwrap();
    assert_eq!(
        preview.rows.len(),
        1,
        "legacy forms must identify the same transfer"
    );
    assert!(!preview
        .warnings
        .iter()
        .any(|w| w.contains("Not currently indexed")));
    let undone = ops::undo(
        &mut f.index,
        &log,
        &f.data,
        &CancellationToken::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!((undone.completed, undone.skipped), (1, 0));
    assert!(row.source.exists() && !row.dest.exists());
    assert!(!fs::read_to_string(&out.logs[0]).unwrap().contains(r"\\?\"));
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
    let expected: std::collections::BTreeMap<_, _> = p
        .actions
        .iter()
        .map(|a| (a.dest.clone(), ops::checksum(&a.source).unwrap()))
        .collect();
    let out = f.execute(&p);
    assert_eq!(out.completed, 3, "{:?}", out.warnings);
    for a in &p.actions {
        assert!(!a.source.exists());
        assert_eq!(ops::checksum(&a.dest).unwrap(), expected[&a.dest]);
    }
    // A durable published (verified) entry precedes done (source removed) for every cross-device move.
    let log = out
        .logs
        .iter()
        .find(|p| p.starts_with(mobboss_core::paths::canonical(&f.data).unwrap()))
        .unwrap();
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

#[test]
fn move_and_backup_requires_destination_before_any_transfer() {
    let f = Fixture::new();
    let before = tree(&f.root);
    let error = ops::prepare(&f.index, Operation::Collect, "Test", &[], None).unwrap_err();
    assert!(error.to_string().contains("explicit destination"));
    assert_eq!(tree(&f.root), before);
    assert_eq!(f.plan(Operation::Move).actions.len(), 3);
}

#[test]
fn copy_and_same_drive_move_report_verification_and_append_columns() {
    for (op, expected) in [
        (Operation::Copy, "yes"),
        (Operation::Move, "not needed (same-drive rename)"),
    ] {
        let mut f = Fixture::new();
        let p = f.plan(op);
        let out = f.execute(&p);
        assert_eq!(out.rows.len(), 3);
        assert!(out.rows.iter().all(|r| r.verified == expected));
        for row in &out.rows {
            if op == Operation::Copy {
                assert_eq!(row.algorithm, "BLAKE3");
                assert_eq!(row.checksum, ops::checksum(&row.dest).unwrap());
            } else {
                assert!(row.checksum.is_empty() && row.algorithm.is_empty());
            }
        }
        let csv = fs::read_to_string(&out.logs[0]).unwrap();
        assert_eq!(
            csv.lines().next().unwrap(),
            "op,source,dest,size,checksum,status,guard,verified,algorithm"
        );
        let logged = ops::read_log(&out.logs[0]).unwrap();
        assert!(logged
            .iter()
            .filter(|r| r.status == "done")
            .all(|r| r.verified == expected));
    }
}
