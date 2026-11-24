//! One filesystem spelling, separate from the prefix-free spelling shown to users.
use anyhow::{ensure, Context, Result};
use serde::{Serialize, Serializer};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// Resolve existing ancestors (including system aliases and Windows 8.3 names),
/// then append an uncreated tail. dunce retains extended syntax only when needed.
pub fn canonical(path: &Path) -> Result<PathBuf> {
    let path = native(path);
    let path = path.as_path();
    ensure!(
        !path
            .components()
            .any(|c| c == Component::ParentDir || c.as_os_str() == ".."),
        "parent traversal prohibited"
    );
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut tail = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tail.push(ancestor.file_name().context("invalid path")?.to_owned());
                ancestor = ancestor.parent().context("no existing path ancestor")?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let mut resolved = dunce::canonicalize(ancestor)?;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    // Appending can take a short Windows path beyond MAX_PATH. std filesystem
    // operations add extended syntax when needed; canonicalize once it exists.
    Ok(resolved)
}

/// WalkDir children already have canonical spelling on Unix. Windows needs a
/// file-level pass to expand short names and choose the same extended-path form.
pub(crate) fn storage(path: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        canonical(path)
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_path_buf())
    }
}
/// Selection resolves root aliases before this guard. Check original components
/// at/below that root, so canonicalization cannot hide a link inserted inside it.
pub fn within(path: &Path, root: &Path) -> Result<PathBuf> {
    let path = native(path);
    let root = native(root);
    let path = path.as_path();
    let root = root.as_path();
    ensure!(
        path.is_absolute() && root.is_absolute(),
        "absolute paths required"
    );
    ensure!(
        !path
            .components()
            .any(|c| c == Component::ParentDir || c.as_os_str() == ".."),
        "parent traversal prohibited"
    );
    ensure!(contains(path, root), "path outside selected root");
    let relative = path.components().skip(root.components().count());
    let resolved = canonical(root)?;
    if let Ok(m) = fs::symlink_metadata(root) {
        ensure!(
            !m.file_type().is_symlink(),
            "symlink paths prohibited: {}",
            display(root).display()
        );
    }
    let mut candidate = resolved;
    for component in relative {
        ensure!(
            matches!(component, Component::Normal(_) | Component::CurDir),
            "invalid relative path"
        );
        candidate.push(component);
        match fs::symlink_metadata(&candidate) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "symlink paths prohibited: {}",
                display(&candidate).display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    canonical(&candidate)
}

/// Transfer roots include the whole collection tree, not just its MXF subfolder.
pub fn tree_root(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(crate::plan::MEDIA_BY_PROJECT))
        })
        .and_then(Path::parent)
        .or_else(|| {
            path.ancestors()
                .find(|p| {
                    p.file_name().is_some_and(|n| {
                        n.eq_ignore_ascii_case("Avid MediaFiles")
                            || n.eq_ignore_ascii_case("OMFI MediaFiles")
                    })
                })
                .and_then(Path::parent)
        })
}
pub fn media(path: &Path) -> Result<PathBuf> {
    let path = native(path);
    let path = path.as_path();
    if let Some(root) = tree_root(path) {
        within(path, root)
    } else {
        from_selected_root(path, path.parent().unwrap_or(path))
    }
}
pub fn selected(path: &Path) -> Result<PathBuf> {
    let path = native(path);
    if tree_root(&path).is_some() {
        media(&path)
    } else {
        canonical(&path)
    }
}
pub(crate) fn native(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    path.to_path_buf()
}
/// Migration of a previously selected root may itself need to resolve an alias.
pub(crate) fn from_selected_root(path: &Path, root: &Path) -> Result<PathBuf> {
    let path = native(path);
    let root = native(root);
    ensure!(contains(&path, &root), "path outside selected root");
    let resolved = canonical(&root)?;
    let tail: PathBuf = path.components().skip(root.components().count()).collect();
    within(&resolved.join(tail), &resolved)
}

/// Cosmetic only. Keep required extended syntax in stored/internal paths; never
/// expose it in UI/CSV. Filesystem access uses canonical(), not this formatter.
pub fn display(path: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(display_windows(&path.to_string_lossy()));
    #[cfg(not(windows))]
    let path = path.to_path_buf();
    path
}
pub fn message(message: &str) -> String {
    #[cfg(windows)]
    {
        message.replace(r"\\?\UNC\", r"\\").replace(r"\\?\", "")
    }
    #[cfg(not(windows))]
    message.to_owned()
}
#[cfg(any(windows, test))]
fn display_windows(path: &str) -> String {
    let path = path.replace('/', "\\");
    if path
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        format!(r"\\{}", &path[8..])
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
    }
}
pub fn serialize<P: AsRef<Path>, S: Serializer>(
    path: &P,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    display(path.as_ref()).serialize(serializer)
}
pub fn serialize_many<S: Serializer>(
    paths: &[PathBuf],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    paths
        .iter()
        .map(|p| display(p))
        .collect::<Vec<_>>()
        .serialize(serializer)
}

pub fn serialize_optional<S: Serializer>(
    path: &Option<PathBuf>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    path.as_ref().map(|p| display(p)).serialize(serializer)
}
pub fn serialize_map<S: Serializer>(
    paths: &std::collections::BTreeMap<String, PathBuf>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    paths
        .iter()
        .map(|(k, p)| (k, display(p)))
        .collect::<std::collections::BTreeMap<_, _>>()
        .serialize(serializer)
}
/// Comparison only; retains the portable case-insensitive reservation policy.
pub fn key(path: &Path) -> String {
    key_for(&path.to_string_lossy(), cfg!(windows))
}
pub fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}
pub fn contains(path: &Path, root: &Path) -> bool {
    let path = key(path);
    let root = key(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| root.ends_with('/') || tail.starts_with('/'))
}
pub(crate) fn key_for(path: &str, windows: bool) -> String {
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
    .to_lowercase();
    let path = if windows {
        if let Some(unc) = path.strip_prefix("//?/unc/") {
            format!("//{unc}")
        } else {
            path.strip_prefix("//?/").unwrap_or(&path).to_owned()
        }
    } else {
        path
    };
    let prefix = if windows && path.starts_with("//") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let mut key = format!(
        "{prefix}{}",
        path.split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect::<Vec<_>>()
            .join("/")
    );
    if windows && key.len() == 2 && key.ends_with(':') && path.ends_with('/') {
        key.push('/');
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_paths_still_reject_parent_traversal() {
        let t = tempfile::tempdir().unwrap();
        assert!(canonical(&t.path().join("../escape")).is_err());
        #[cfg(windows)]
        assert!(canonical(Path::new(r"\\?\C:\Sample Drive\..\escape")).is_err());
    }
    #[test]
    fn windows_display_hides_extended_prefixes_including_unc() {
        assert_eq!(
            display_windows(r"\\?\C:\Sample Drive\a.mxf"),
            r"C:\Sample Drive\a.mxf"
        );
        assert_eq!(
            display_windows(r"\\?\UNC\server\share\a.mxf"),
            r"\\server\share\a.mxf"
        );
        assert_eq!(
            display_windows("C:/Sample Drive/a.mxf"),
            r"C:\Sample Drive\a.mxf"
        );
    }
    #[cfg(windows)]
    #[test]
    fn long_paths_keep_filesystem_access_but_never_expose_prefixes_or_break_clip_ids() {
        let t = tempfile::tempdir().unwrap();
        let mut root = canonical(t.path()).unwrap();
        for _ in 0..12 {
            root.push("long-directory-name");
        }
        let file = root.join("Avid MediaFiles/MXF/1/sample.mxf");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"media").unwrap();
        let path = canonical(&file).unwrap();
        assert!(path.to_string_lossy().starts_with(r"\\?\"));
        assert!(fs::metadata(display(&path)).is_ok());
        let original = crate::MediaFile {
            path: file.clone(),
            folder: file.parent().unwrap().into(),
            size: 5,
            modified_ns: crate::ops::stamp(&fs::metadata(&file).unwrap()),
            metadata: Default::default(),
        };
        let db = t.path().join("data/index.sqlite");
        let mut index = crate::Index::open(&db).unwrap();
        index
            .record_transfer(&original, &file, &file, false)
            .unwrap();
        let indexed = index.files(None).unwrap().remove(0);
        let json = serde_json::to_value(&indexed).unwrap();
        assert!(!json["path"].as_str().unwrap().contains(r"\\?\"));
        assert_eq!(
            crate::clips::clip_id(&indexed),
            format!("file:{}", json["path"].as_str().unwrap())
        );
        let conn = rusqlite::Connection::open(db).unwrap();
        let stored_id: String = conn
            .query_row("SELECT clip_id FROM clip_members", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored_id, crate::clips::clip_id(&indexed));
        let row = crate::ops::LogRow {
            op: "copy".into(),
            source: path.clone(),
            dest: path,
            size: 5,
            checksum: "test".into(),
            status: "done".into(),
            guard: String::new(),
        };
        let mut csv = csv::Writer::from_writer(Vec::new());
        csv.serialize(&row).unwrap();
        assert!(!String::from_utf8(csv.into_inner().unwrap())
            .unwrap()
            .contains(r"\\?\"));
    }
    #[cfg(unix)]
    #[test]
    fn system_alias_above_selected_root_is_allowed_but_inner_links_are_not() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("private/var/session/drive");
        fs::create_dir_all(real.join("Avid MediaFiles/MXF/1")).unwrap();
        std::os::unix::fs::symlink(t.path().join("private/var"), t.path().join("var")).unwrap();
        let root = t.path().join("var/session/drive");
        assert_eq!(
            within(&root.join("Avid MediaFiles/MXF/1/new.mxf"), &root).unwrap(),
            canonical(&real.join("Avid MediaFiles/MXF/1/new.mxf")).unwrap()
        );
        std::os::unix::fs::symlink(t.path(), real.join("Avid MediaFiles/MXF/inside")).unwrap();
        assert!(within(&root.join("Avid MediaFiles/MXF/inside/new.mxf"), &root).is_err());
    }
}
