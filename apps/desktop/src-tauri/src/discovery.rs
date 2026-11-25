//! Read-only, top-level discovery. A stuck OS call keeps at most one worker per
//! path; future polls skip it instead of accumulating abandoned threads.
use crate::service::Volume;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant},
};
pub const TIMEOUT: Duration = Duration::from_secs(2);

fn start<T: Send + 'static>(
    key: String,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<mpsc::Receiver<T>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let flight = IN_FLIGHT.get_or_init(Default::default);
    if !flight.lock().ok()?.insert(key.clone()) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Remove the reservation even if a worker panics.
        struct Reservation(String);
        impl Drop for Reservation {
            fn drop(&mut self) {
                if let Ok(mut paths) = IN_FLIGHT.get().unwrap().lock() {
                    paths.remove(&self.0);
                }
            }
        }
        let _reservation = Reservation(key);
        let _ = tx.send(work());
    });
    Some(rx)
}
pub fn bounded<T: Send + 'static>(
    key: String,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    start(key, work)?.recv_timeout(TIMEOUT).ok()
}

#[derive(Clone, Debug, Serialize)]
pub struct FoundSource {
    #[serde(skip)]
    mount: PathBuf,
    #[serde(serialize_with = "mobboss_core::paths::serialize")]
    pub path: PathBuf,
    pub volume: String,
    pub available: u64,
    pub total: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct DeniedVolume {
    pub name: String,
    #[serde(serialize_with = "mobboss_core::paths::serialize")]
    pub path: PathBuf,
}
#[derive(Default, Serialize)]
pub struct Discovery {
    pub found: Vec<FoundSource>,
    pub denied: Vec<DeniedVolume>,
    #[serde(serialize_with = "mobboss_core::paths::serialize_many")]
    pub online: Vec<PathBuf>,
}

/// Read only the immediate children, preserving the actual directory spelling.
pub fn media_at_root(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = vec![];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name.eq_ignore_ascii_case("Avid MediaFiles")
            || name.eq_ignore_ascii_case("OMFI MediaFiles"))
            && entry.path().is_dir()
        {
            found.push(
                mobboss_core::paths::selected(&entry.path()).unwrap_or_else(|_| entry.path()),
            );
        }
    }
    found.sort();
    Ok(found)
}

/// Opening a cached directory handle alone may succeed on a dead share. Read
/// an entry too, so a stalled readdir is included in the availability deadline.
pub fn readable_root(path: &Path) -> bool {
    fs::read_dir(path)
        .and_then(|mut entries| entries.next().transpose().map(|_| ()))
        .is_ok()
}

#[derive(Default)]
pub struct VolumeCache {
    results: HashMap<PathBuf, Discovery>,
}
impl VolumeCache {
    fn check(
        &mut self,
        volumes: Vec<Volume>,
        refresh: bool,
        check: fn(&Path) -> std::io::Result<Vec<PathBuf>>,
    ) -> Discovery {
        self.results
            .retain(|path, _| volumes.iter().any(|v| &v.path == path));
        let new: Vec<_> = volumes
            .into_iter()
            .filter(|v| refresh || !self.results.contains_key(&v.path))
            .collect();
        // Probe concurrently under one deadline, including empty/denied/timed-out results.
        let result = probe(new.clone(), vec![], TIMEOUT, check);
        for volume in new {
            self.results.insert(
                volume.path.clone(),
                Discovery {
                    found: result
                        .found
                        .iter()
                        .filter(|s| s.mount == volume.path)
                        .cloned()
                        .collect(),
                    denied: result
                        .denied
                        .iter()
                        .filter(|v| v.path == volume.path)
                        .cloned()
                        .collect(),
                    online: vec![],
                },
            );
        }
        let mut result = Discovery::default();
        for cached in self.results.values() {
            result.found.extend(cached.found.clone());
            result.denied.extend(cached.denied.clone());
        }
        result.found.sort_by(|a, b| a.path.cmp(&b.path));
        result.denied.sort_by(|a, b| a.path.cmp(&b.path));
        result
    }
}

pub fn discover(roots: Vec<PathBuf>, refresh: bool) -> Discovery {
    static CACHE: OnceLock<Mutex<VolumeCache>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    // Disk enumeration itself can call a network filesystem. Bound it as well.
    let disks = start("volume-enumeration".into(), || {
        // On macOS list mount points without sysinfo's per-disk capacity refresh.
        // Supplemental enumeration reads /Volumes itself, never its child roots.
        #[cfg(target_os = "macos")]
        {
            Vec::new()
        }
        #[cfg(not(target_os = "macos"))]
        {
            crate::service::volumes()
        }
    });
    let extra = start(
        "supplemental-volumes".into(),
        crate::service::supplemental_volumes,
    );
    let deadline = Instant::now() + TIMEOUT;
    let disks = disks.and_then(|rx| {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    });
    let extra = extra.and_then(|rx| {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    });
    // An incomplete enumeration is not evidence of an unmount.
    let mut result = if let (Some(mut volumes), Some(extra)) = (disks, extra) {
        for v in extra {
            if !volumes
                .iter()
                .any(|existing| mobboss_core::paths::same(&existing.path, &v.path))
            {
                volumes.push(v);
            }
        }
        cache.check(volumes, refresh, media_at_root)
    } else {
        let mut result = Discovery::default();
        for cached in cache.results.values() {
            result.found.extend(cached.found.clone());
            result.denied.extend(cached.denied.clone());
        }
        result
    };
    // Existing selected sources retain their bounded availability checks, but
    // never retry a root belonging to a permission-denied volume.
    let selected_roots = roots.clone();
    let roots = roots
        .into_iter()
        .filter(|p| {
            !cache
                .results
                .keys()
                .any(|mount| mobboss_core::paths::same(mount, p))
                && !result.denied.iter().any(|v| p.starts_with(&v.path))
        })
        .collect();
    result.online = probe(vec![], roots, TIMEOUT, media_at_root).online;
    for source in &result.found {
        if !selected_roots
            .iter()
            .any(|p| mobboss_core::paths::same(p, &source.path))
            && !result.online.contains(&source.path)
        {
            result.online.push(source.path.clone());
        }
    }
    result
}
fn probe(
    volumes: Vec<Volume>,
    roots: Vec<PathBuf>,
    timeout: Duration,
    check: fn(&Path) -> std::io::Result<Vec<PathBuf>>,
) -> Discovery {
    let deadline = Instant::now() + timeout;
    let requests: Vec<_> = volumes
        .iter()
        .map(|v| {
            let path = v.path.clone();
            let (available, total) = (v.available, v.total);
            (
                v,
                start(format!("volume:{}", path.display()), move || {
                    let paths = check(&path)?;
                    // Supplementary mapped drives may be absent from sysinfo.
                    // Capacity calls share the root's deadline and worker limit.
                    let (available, total) = if total == 0 && !paths.is_empty() {
                        (
                            fs2::available_space(&path).unwrap_or(0),
                            fs2::total_space(&path).unwrap_or(0),
                        )
                    } else {
                        (available, total)
                    };
                    Ok::<_, std::io::Error>((paths, available, total))
                }),
            )
        })
        .collect();
    let source_requests: Vec<_> = roots
        .into_iter()
        .map(|p| {
            let path = p.clone();
            (
                p,
                start(format!("source:{}", path.display()), move || {
                    readable_root(&path)
                }),
            )
        })
        .collect();
    let mut found = vec![];
    let mut denied = vec![];
    for (v, rx) in requests {
        let response = rx.and_then(|rx| {
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()
        });
        if response
            .as_ref()
            .is_some_and(|r| r.as_ref().is_err_and(permission_denied))
        {
            denied.push(DeniedVolume {
                name: v.name.clone(),
                path: v.path.clone(),
            });
        }
        if let Some(Ok((paths, available, total))) = response {
            found.extend(paths.into_iter().map(|path| FoundSource {
                path,
                mount: v.path.clone(),
                volume: v.name.clone(),
                available,
                total,
            }));
        }
    }
    let mut online: Vec<PathBuf> = source_requests
        .into_iter()
        .filter_map(|(path, rx)| {
            rx.and_then(|rx| {
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
            })
            .filter(|ok| *ok)
            .map(|_| path)
        })
        .collect();
    for source in &found {
        if !online
            .iter()
            .any(|p| mobboss_core::paths::same(p, &source.path))
        {
            online.push(source.path.clone());
        }
    }
    Discovery {
        found,
        online,
        denied,
    }
}

fn permission_denied(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || cfg!(unix) && matches!(error.raw_os_error(), Some(1 | 13))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fake_volume(path: PathBuf) -> Volume {
        Volume {
            name: "Media-01".into(),
            path,
            available: 0,
            total: 0,
        }
    }
    #[test]
    fn caches_each_mount_until_refresh_or_unmount() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn check(path: &Path) -> std::io::Result<Vec<PathBuf>> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(vec![path.join("Avid MediaFiles")])
        }
        let volume = || {
            let mut v = fake_volume("Cache-Island".into());
            v.total = 100;
            v
        };
        let mut cache = VolumeCache::default();
        assert_eq!(cache.check(vec![volume()], false, check).found.len(), 1);
        for _ in 0..3 {
            assert_eq!(cache.check(vec![volume()], false, check).found.len(), 1);
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        cache.check(
            vec![volume(), fake_volume("Cache-Meadow".into())],
            false,
            check,
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
        cache.check(vec![volume()], true, check);
        assert_eq!(CALLS.load(Ordering::SeqCst), 3);
        assert!(cache.check(vec![], false, check).found.is_empty());
        cache.check(vec![volume()], false, check);
        assert_eq!(CALLS.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn denied_mount_is_not_retried_until_refresh() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn denied(_: &Path) -> std::io::Result<Vec<PathBuf>> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fake denial",
            ))
        }
        let mut cache = VolumeCache::default();
        for _ in 0..4 {
            let result = cache.check(vec![fake_volume("Denied-Island".into())], false, denied);
            assert_eq!(result.denied.len(), 1);
            assert!(result.found.is_empty());
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        cache.check(vec![fake_volume("Denied-Island".into())], true, denied);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
        #[cfg(unix)]
        for code in [1, 13] {
            assert!(permission_denied(&std::io::Error::from_raw_os_error(code)));
        }
    }
    #[test]
    fn root_names_and_nested_folders() {
        let root = std::env::temp_dir().join(format!("mobboss-discovery-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = mobboss_core::paths::canonical(&root).unwrap();
        fs::create_dir_all(root.join("Avid MediaFiles")).unwrap();
        fs::create_dir_all(root.join("OMFI MediaFiles")).unwrap();
        fs::create_dir_all(root.join("nested/Avid MediaFiles")).unwrap();
        fs::create_dir_all(root.join("Avid Media Files")).unwrap();
        let found = media_at_root(&root).unwrap();
        assert_eq!(
            found,
            vec![root.join("Avid MediaFiles"), root.join("OMFI MediaFiles")]
        );
        fs::rename(root.join("Avid MediaFiles"), root.join("avid mediafiles")).unwrap();
        assert!(media_at_root(&root)
            .unwrap()
            .contains(&root.join("avid mediafiles")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unavailable_roots() {
        let root = std::env::temp_dir().join("mobboss-unavailable-volume");
        let result = probe(
            vec![fake_volume(root.clone())],
            vec![root],
            TIMEOUT,
            media_at_root,
        );
        assert!(result.found.is_empty());
        assert!(result.online.is_empty());
    }
    #[test]
    fn slow_roots_are_bounded_and_not_restarted() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static RELEASE: AtomicBool = AtomicBool::new(false);
        struct Release;
        impl Drop for Release {
            fn drop(&mut self) {
                RELEASE.store(true, Ordering::SeqCst);
            }
        }
        let _release = Release;
        fn slow(path: &Path) -> std::io::Result<Vec<PathBuf>> {
            if path == Path::new("Media-02") {
                return Ok(vec![path.join("Avid MediaFiles")]);
            }
            CALLS.fetch_add(1, Ordering::SeqCst);
            while !RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(vec![PathBuf::from("Avid MediaFiles")])
        }
        let start = Instant::now();
        for _ in 0..2 {
            let result = probe(
                vec![
                    fake_volume("NEXIS-Workspace".into()),
                    fake_volume("Media-02".into()),
                ],
                vec![],
                Duration::from_millis(20),
                slow,
            );
            assert_eq!(result.found.len(), 1);
            assert_eq!(
                result.found[0].path,
                PathBuf::from("Media-02/Avid MediaFiles")
            );
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }
}
