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
