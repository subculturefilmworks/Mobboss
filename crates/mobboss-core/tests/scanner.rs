use mobboss_core::{Index, ScanMode};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "mobboss-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> &'static [u8] {
    include_bytes!("../../../tests/fixtures/video.header")
}
#[test]
fn incremental_and_corrupt_files() {
    let t = Temp::new();
    let media = t.0.join("workspace/avid mediafiles/mXf/Host.9");
    fs::create_dir_all(&media).unwrap();
    let a = media.join("Café 東京.MXF");
    fs::write(&a, fixture()).unwrap();
    let a = mobboss_core::paths::canonical(&a).unwrap();
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let r = index.scan(&t.0.join("workspace"), ScanMode::Auto).unwrap();
    assert_eq!((r.files, r.headers_read), (1, 1));
    assert_eq!(r.projects["BD Demo Reel"], 1);
    let r = index.scan(&t.0.join("workspace"), ScanMode::Quick).unwrap();
    assert_eq!((r.files, r.headers_read, r.cached), (1, 0, 1));
    let mut changed = fixture().to_vec();
    changed[128] ^= 1;
    fs::write(&a, changed).unwrap();
    fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1234567))
        .unwrap();
    let r = index.scan(&media, ScanMode::Quick).unwrap();
    assert_eq!(r.diff.changed.len(), 1);
    assert_eq!(r.headers_read, 1);
    fs::remove_file(&a).unwrap();
    fs::write(media.join("broken.mxf"), [0, 1, 2]).unwrap();
    let r = index.scan(&media, ScanMode::Quick).unwrap();
    assert_eq!(r.files, 0);
    assert_eq!(r.diff.missing, vec![a]);
    assert_eq!(r.diff.unlisted.len(), 1);
    assert!(!r.warnings.is_empty());
    assert!(index.files(None).unwrap().is_empty());
    let history = index.files_including_absent().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].metadata.project.as_deref(), Some("BD Demo Reel"));
}
#[test]
fn database_only_directory_reports_missing() {
    let t = Temp::new();
    let media = t.0.join("OMFI MediaFiles");
    fs::create_dir(&media).unwrap();
    fs::write(
        media.join("msmFMID.pmr"),
        include_bytes!("../../../tests/fixtures/msmFMID.pmr"),
    )
    .unwrap();
    let mut i = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let r = i.scan(&media, ScanMode::Quick).unwrap();
    assert_eq!(r.diff.missing.len(), 344);
}
#[test]
fn legacy_index_paths_migrate_to_canonical_storage_without_losing_clip_members() {
    assert_legacy_migration(|path, root| {
        #[cfg(unix)]
        let legacy = {
            let alias = root.join("legacy-system-parent");
            std::os::unix::fs::symlink(root, &alias).unwrap();
            alias.join(path.strip_prefix(root).unwrap())
        };
        #[cfg(windows)]
        let legacy = {
            let _ = root;
            mobboss_core::paths::display(&mobboss_core::paths::canonical(path).unwrap())
        };
        vec![(
            "legacy selected root".into(),
            legacy.to_string_lossy().into_owned(),
            legacy.parent().unwrap().to_string_lossy().into_owned(),
        )]
    });
}

#[cfg(windows)]
#[test]
fn windows_legacy_verbatim_short_names_and_mixed_separators_migrate() {
    assert_legacy_migration(|path, _| {
        let long = mobboss_core::paths::display(&mobboss_core::paths::canonical(path).unwrap());
        let short = windows_short_path(&long);
        eprintln!("Windows migration: long={long:?}, GetShortPathNameW={short:?}");
        if short == long {
            eprintln!("This volume has no 8.3 alias; testing the spelling returned by Windows");
        }
        let mut variants = Vec::new();
        for (label, path) in [("long", long), ("8.3", short)] {
            // Obtain the folder BEFORE introducing mixed verbatim separators.
            // Windows Path::parent() cannot split that historical string form.
            let folder = path.parent().unwrap().to_string_lossy().into_owned();
            for mixed in [false, true] {
                let legacy = |s: &str| {
                    let s = s.to_uppercase();
                    let s = if mixed { s.replace('\\', "/") } else { s };
                    format!(r"\\?\{s}")
                };
                variants.push((
                    format!("{label}, verbatim, mixed={mixed}"),
                    legacy(&path.to_string_lossy()),
                    legacy(&folder),
                ));
            }
        }
        variants
    });
}

#[cfg(windows)]
fn windows_short_path(path: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, size: u32) -> u32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both pointers remain valid throughout each call; size=0 queries capacity.
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    assert_ne!(
        needed,
        0,
        "GetShortPathNameW({path:?}): {}",
        std::io::Error::last_os_error()
    );
    let mut buffer = vec![0u16; needed as usize];
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), needed) };
    assert!(
        written > 0 && written < needed,
        "GetShortPathNameW({path:?}): {}",
        std::io::Error::last_os_error()
    );
    PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written as usize]))
}

fn migration_keys(db: &Path) -> String {
    let read = || -> rusqlite::Result<String> {
        let conn = rusqlite::Connection::open(db)?;
        let mut result = String::new();
        for sql in [
            "SELECT path,folder FROM files ORDER BY path",
            "SELECT path,clip_id FROM clip_members ORDER BY path",
            "SELECT root,CAST(id AS TEXT) FROM scan_runs ORDER BY id",
        ] {
            let rows = conn
                .prepare(sql)?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result.push_str(&format!("\n{sql}: {rows:#?}"));
        }
        Ok(result)
    };
    read().unwrap_or_else(|e| format!("cannot read stored keys: {e}"))
}

fn assert_legacy_migration(variants: impl FnOnce(&Path, &Path) -> Vec<(String, String, String)>) {
    let t = Temp::new();
    let media = t.0.join("drive/Avid MediaFiles/MXF/1");
    fs::create_dir_all(&media).unwrap();
    let path = media.join("a.mxf");
    fs::write(&path, fixture()).unwrap();
    let db = t.0.join("data/index.sqlite");
    let mut index = Index::open(&db).unwrap();
    index.scan(&t.0.join("drive"), ScanMode::Deep).unwrap();
    let original = index
        .files(None)
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("scan produced no row: {}", migration_keys(&db)));
    drop(index);
    for (label, legacy, folder) in variants(&path, &t.0) {
        for uid in [original.metadata.material_package_uid.clone(), None] {
            let mut metadata = original.metadata.clone();
            metadata.material_package_uid = uid;
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute("DELETE FROM files", []).unwrap();
            conn.execute("INSERT INTO files(path,folder,size,modified_ns,project,metadata,present) VALUES(?1,?2,?3,?4,?5,?6,1)", rusqlite::params![legacy, folder,original.size,original.modified_ns,metadata.project,serde_json::to_string(&metadata).unwrap()]).unwrap();
            conn.execute_batch("PRAGMA user_version=1").unwrap();
            drop(conn);
            let index = Index::open(&db).unwrap_or_else(|e| {
                panic!("{label}: migration failed: {e:#}; {}", migration_keys(&db))
            });
            let diagnostic = format!(
                "{label}, material UID={:?}; {}",
                metadata.material_package_uid,
                migration_keys(&db)
            );
            let files = index
                .files(None)
                .unwrap_or_else(|e| panic!("cannot read migrated files: {e:#}; {diagnostic}"));
            assert_eq!(files.len(), 1, "{diagnostic}");
            assert_eq!(
                files[0].path,
                mobboss_core::paths::canonical(&path).unwrap(),
                "{diagnostic}"
            );
            assert_eq!(
                files[0].folder,
                files[0].path.parent().unwrap(),
                "{diagnostic}"
            );
            let clips = index
                .clips()
                .unwrap_or_else(|e| panic!("cannot read migrated clips: {e:#}; {diagnostic}"));
            assert_eq!(clips.len(), 1, "{diagnostic}");
            assert_eq!(clips[0].files.len(), 1, "{diagnostic}");
            assert_eq!(clips[0].files[0].path, files[0].path, "{diagnostic}");
            let conn = rusqlite::Connection::open(&db).unwrap();
            let stored: Vec<(String, String)> = conn
                .prepare("SELECT path,clip_id FROM clip_members")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                stored,
                vec![(
                    files[0].path.to_string_lossy().into_owned(),
                    mobboss_core::clips::clip_id(&files[0])
                )],
                "{diagnostic}"
            );
            assert_eq!(
                conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                2,
                "{diagnostic}"
            );
            assert!(
                !serde_json::to_string(&files).unwrap().contains(r"\\\\?\\"),
                "{diagnostic}"
            );
        }
    }
}
#[test]
fn index_cannot_write_into_media() {
    let t = Temp::new();
    let media = t.0.join("arbitrary");
    fs::create_dir(&media).unwrap();
    fs::write(media.join("clip.mxf"), fixture()).unwrap();
    let dest = media.join("nested/index.sqlite");
    assert!(Index::open_for_scan(&dest, &media).is_err());
    assert!(Index::open(&dest).is_err());
    assert!(!media.join("nested").exists());
}
#[cfg(unix)]
#[test]
fn index_symlink_guard() {
    let t = Temp::new();
    let media = t.0.join("media");
    fs::create_dir(&media).unwrap();
    std::os::unix::fs::symlink(&media, t.0.join("alias")).unwrap();
    assert!(Index::open_for_scan(&t.0.join("alias/index.sqlite"), &media).is_err());
    assert!(!media.join("index.sqlite").exists());
}
#[test]
fn sample_folder_when_available() {
    // Set MOBBOSS_SAMPLE_DIR to the 344-file sample MXF folder to run this check.
    let Some(dir) = std::env::var_os("MOBBOSS_SAMPLE_DIR") else {
        eprintln!("MOBBOSS_SAMPLE_DIR unset; skipping external-data validation");
        return;
    };
    let root = Path::new(&dir);
    if !root.exists() {
        eprintln!("sample folder absent; skipping external-data validation");
        return;
    }
    let t = Temp::new();
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), root).unwrap();
    let deep = index.scan(root, ScanMode::Deep).unwrap();
    assert_eq!(deep.files, 344);
    assert_eq!(deep.projects.get("BD Demo Reel"), Some(&344));
    assert_eq!(deep.projects.len(), 1);
    for f in index.files(None).unwrap() {
        assert!(f.metadata.duration.unwrap() > 0);
        assert!(f.metadata.edit_rate.unwrap().denominator > 0);
        assert!(f.metadata.codec.is_some());
        assert!(f.metadata.file_package_uid.is_some());
        assert!(f.metadata.material_package_uid.is_some());
    }
    let quick = index.scan(root, ScanMode::Quick).unwrap();
    assert_eq!(
        (quick.files, quick.headers_read, quick.cached),
        (344, 0, 344)
    );
    assert!(quick.diff.missing.is_empty());
    assert!(quick.diff.unlisted.is_empty());
    assert!(quick.diff.changed.is_empty());
    println!(
        "Sample: deep {:.3}s, quick {:.3}s",
        deep.elapsed_seconds, quick.elapsed_seconds
    );
}

#[test]
fn progressive_cancel_rolls_back_and_parallel_matches() {
    use mobboss_core::{CancellationToken, ScanOptions, ScanPhase};
    let t = Temp::new();
    let media = t.0.join("media");
    fs::create_dir_all(&media).unwrap();
    for i in 0..12 {
        fs::write(media.join(format!("{i}.mxf")), fixture()).unwrap();
    }
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let token = CancellationToken::default();
    let result = index.scan_with(&media, &ScanOptions::default(), &token, |p| {
        if matches!(p.phase, ScanPhase::Enrichment) {
            token.cancel();
        }
    });
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(index.files(None).unwrap().is_empty());
    let mut serial = Vec::new();
    for concurrency in [1, 4, 8, 16] {
        let r = index
            .scan_with(
                &media,
                &ScanOptions {
                    mode: ScanMode::Deep,
                    concurrency,
                },
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();
        assert_eq!((r.files, r.headers_read), (12, 12));
        let rows = serde_json::to_string(&index.files(None).unwrap()).unwrap();
        if serial.is_empty() {
            serial = rows.as_bytes().to_vec();
        }
        assert_eq!(serial, rows.as_bytes());
    }
}

#[test]
fn pmr_inventory_precedes_headers_and_cancel_keeps_prior_index() {
    use mobboss_core::{CancellationToken, ScanOptions, ScanPhase};
    let t = Temp::new();
    let media = t.0.join("media");
    fs::create_dir(&media).unwrap();
    let pmr = include_bytes!("../../../tests/fixtures/msmFMID.pmr");
    let entry = mobboss_core::pmr::parse(pmr).unwrap().remove(0);
    fs::write(media.join("msmFMID.pmr"), pmr).unwrap();
    // This header need not have the same UID: provisional IDs must come from PMR.
    fs::write(media.join(&entry.filename), fixture()).unwrap();
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let token = CancellationToken::default();
    let mut inventory_seen = false;
    let result = index.scan_with(&media, &ScanOptions::default(), &token, |p| {
        if matches!(p.phase, ScanPhase::Inventory) {
            assert_eq!(p.files.len(), 1);
            assert_eq!(
                p.files[0].metadata.project.as_deref(),
                Some(entry.project.as_str())
            );
            assert_eq!(
                p.files[0].metadata.file_package_uid.as_deref(),
                Some(entry.file_package_uid.as_str())
            );
            assert!(p.files[0].metadata.codec.is_none());
            inventory_seen = true;
            token.cancel();
        }
        assert!(!matches!(p.phase, ScanPhase::Enrichment));
    });
    assert!(inventory_seen);
    assert!(result.is_err());
    assert!(index.files(None).unwrap().is_empty());
    assert!(mobboss_core::validate_destination(&t.0.join("new/../media/index.sqlite")).is_err());
    assert!(!t.0.join("new").exists());
}

#[test]
fn codec_profiles_and_rate_qualified_dnxhd() {
    use mobboss_core::{
        mxf::{codec_label, codec_name},
        Rational,
    };
    for (suffix, label) in [
        ("24", "DNxHR 444"),
        ("25", "DNxHR HQX"),
        ("26", "DNxHR HQ"),
        ("27", "DNxHR SQ"),
        ("28", "DNxHR LB"),
    ] {
        assert_eq!(
            codec_name(&format!("060e2b340401010d0401020271{suffix}0000")),
            label
        );
    }
    let ul = "060e2b340401010a0401020271030000";
    assert_eq!(
        codec_label(
            ul,
            Some(&Rational {
                numerator: 30000,
                denominator: 1001
            })
        ),
        "DNxHD 145"
    );
    assert_eq!(
        codec_label(
            ul,
            Some(&Rational {
                numerator: 25,
                denominator: 1
            })
        ),
        "DNxHD 120"
    );
    assert_eq!(codec_label(ul, None), "DNxHD 1080p 8-bit Medium");
    assert_eq!(
        codec_label("060e2b340401010a0401020271ff0000", None),
        "DNxHD / DNxHR"
    );
}

#[test]
fn reconciliation_counts_hidden_files_databases_and_failed_headers() {
    let t = Temp::new();
    let media = t.0.join("drive/Avid MediaFiles/MXF/1");
    fs::create_dir_all(&media).unwrap();
    fs::write(t.0.join("drive/unrelated.txt"), b"excluded").unwrap();
    fs::write(media.join("clip.mxf"), fixture()).unwrap();
    fs::write(media.join("bad.mxf"), b"bad").unwrap();
    fs::write(media.join("._clip.mxf"), b"apple").unwrap();
    fs::write(media.join(".DS_Store"), b"finder").unwrap();
    let pmr = include_bytes!("../../../tests/fixtures/msmFMID.pmr");
    fs::write(media.join("msmFMID.pmr"), pmr).unwrap();
    // Valid minimal Bento label; no invented object count.
    let mut mdb = vec![
        0xa4, 0x43, 0x4d, 0xa5, 0x48, 0x64, 0x72, 0xd7, 0, 0, b'I', b'I',
    ];
    mdb.resize(24, 0);
    fs::write(media.join("msmMMOB.mdb"), &mdb).unwrap();
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &t.0.join("drive")).unwrap();
    let report = index.scan(&t.0.join("drive"), ScanMode::Deep).unwrap();
    let r = report.reconciliation;
    assert_eq!(report.files, 1);
    assert_eq!(r.media_files, 2);
    assert_eq!(r.database_files.len(), 2);
    assert_eq!(r.other_files, 2);
    assert_eq!(r.folders, 3); // Avid MediaFiles, MXF, 1
    assert_eq!(
        r.total_bytes,
        (fixture().len() + 3 + 5 + 6 + pmr.len() + 24) as u64
    );
    assert!(r.other_names.iter().any(|p| p.ends_with("._clip.mxf")));
    assert!(r
        .database_files
        .iter()
        .all(|db| db.pmr_missing == Some(344) && db.pmr_extra == Some(2)));
    assert!(report.warnings.iter().any(|w| w.contains("PMR mismatch")));
    assert!(!report.warnings.iter().any(|w| w.contains("msmMMOB.mdb")));
}

#[test]
fn other_file_details_are_capped_without_losing_totals() {
    let t = Temp::new();
    let media = t.0.join("media");
    fs::create_dir(&media).unwrap();
    for i in 0..205 {
        fs::write(media.join(format!("other-{i}")), b"x").unwrap();
    }
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let r = index.scan(&media, ScanMode::Quick).unwrap().reconciliation;
    assert_eq!(r.other_files, 205);
    assert_eq!(r.other_names.len(), 200);
    assert_eq!(r.total_bytes, 205);
}

#[test]
fn pmr_agreement_is_information() {
    let t = Temp::new();
    let media = t.0.join("media");
    fs::create_dir(&media).unwrap();
    let pmr = include_bytes!("../../../tests/fixtures/msmFMID.pmr");
    let entries = mobboss_core::pmr::parse(pmr).unwrap();
    for entry in entries {
        fs::write(media.join(entry.filename), fixture()).unwrap();
    }
    fs::write(media.join("msmFMID.pmr"), pmr).unwrap();
    let mut index = Index::open_for_scan(&t.0.join("index.sqlite"), &media).unwrap();
    let r = index.scan(&media, ScanMode::Quick).unwrap();
    assert_eq!(r.reconciliation.database_files[0].pmr_missing, Some(0));
    assert_eq!(r.reconciliation.database_files[0].pmr_extra, Some(0));
    assert!(!r.warnings.iter().any(|w| w.contains("PMR mismatch")));
}
