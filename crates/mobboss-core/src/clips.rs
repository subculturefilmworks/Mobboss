//! Logical clips retain every physical member, including duplicate copies on other volumes.
use crate::{MediaFile, MediaMetadata};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub metadata: MediaMetadata,
    pub size: u64,
    pub tracks: String,
    pub files: Vec<MediaFile>,
}
pub fn clip_id(file: &MediaFile) -> String {
    file.metadata
        .material_package_uid
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|uid| format!("uid:{uid}"))
        .unwrap_or_else(|| format!("file:{}", file.path.to_string_lossy()))
}
pub fn group(files: Vec<MediaFile>) -> Vec<Clip> {
    let mut groups: BTreeMap<String, Vec<MediaFile>> = BTreeMap::new();
    for file in files {
        groups.entry(clip_id(&file)).or_default().push(file);
    }
    groups
        .into_iter()
        .map(|(id, mut files)| {
            files.sort_by(|a, b| {
                a.metadata
                    .track_kind
                    .cmp(&b.metadata.track_kind)
                    .reverse()
                    .then(a.metadata.track_id.cmp(&b.metadata.track_id))
                    .then(a.path.cmp(&b.path))
            });
            let representative = files
                .iter()
                .find(|f| f.metadata.track_kind.as_deref() == Some("video"))
                .unwrap_or(&files[0]);
            let metadata = representative.metadata.clone();
            let name = metadata
                .clip_name
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    representative
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                });
            let mut tracks = Vec::new();
            for (kind, prefix) in [("video", "V"), ("audio", "A"), ("data", "D")] {
                // File UID deduplicates copies but never removes members from actions.
                let mut unique = std::collections::BTreeSet::new();
                let n = files
                    .iter()
                    .filter(|f| f.metadata.track_kind.as_deref() == Some(kind))
                    .filter(|f| {
                        unique.insert(
                            f.metadata
                                .file_package_uid
                                .clone()
                                .unwrap_or_else(|| f.path.to_string_lossy().into()),
                        )
                    })
                    .count();
                if n > 0 {
                    tracks.push(if n == 1 {
                        format!("{prefix}1")
                    } else {
                        format!("{prefix}1–{prefix}{n}")
                    });
                }
            }
            Clip {
                id,
                name,
                project: metadata.project.clone(),
                metadata,
                size: files.iter().map(|f| f.size).sum(),
                tracks: tracks.join(" + "),
                files,
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn file(path: &str, uid: Option<&str>, kind: &str) -> MediaFile {
        MediaFile {
            path: path.into(),
            folder: "/media".into(),
            size: 10,
            modified_ns: "0".into(),
            metadata: MediaMetadata {
                material_package_uid: uid.map(str::to_owned),
                track_kind: Some(kind.into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn groups_by_uid_never_by_name_and_keeps_every_member() {
        let clips = group(vec![
            file("v", Some("one"), "video"),
            file("a1", Some("one"), "audio"),
            file("a2", Some("one"), "audio"),
            file("other", Some("two"), "video"),
            file("unknown1", None, "video"),
            file("unknown2", None, "video"),
        ]);
        assert_eq!(clips.len(), 4);
        let c = clips.iter().find(|c| c.id == "uid:one").unwrap();
        assert_eq!(c.files.len(), 3);
        assert_eq!(c.size, 30);
        assert_eq!(c.tracks, "V1 + A1–A2");
    }
    #[test]
    fn migration_populates_members_and_transfer_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE files(path TEXT PRIMARY KEY,folder TEXT NOT NULL,size INTEGER NOT NULL,modified_ns TEXT NOT NULL,project TEXT,metadata TEXT NOT NULL,present INTEGER NOT NULL DEFAULT 1);").unwrap();
        conn.execute(
            "INSERT INTO files VALUES('v','/media',10,'0',NULL,?1,1)",
            [serde_json::to_string(&file("v", Some("one"), "video").metadata).unwrap()],
        )
        .unwrap();
        drop(conn);
        let index = crate::Index::open(&path).unwrap();
        assert_eq!(index.clips().unwrap()[0].id, "uid:one");
        let conn = rusqlite::Connection::open(path).unwrap();
        assert_eq!(
            conn.query_row("SELECT clip_id FROM clip_members WHERE path='v'", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "uid:one"
        );
        conn.execute("DELETE FROM files", []).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM clip_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
