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
    let root = Path::new("<sample-folder>");
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
