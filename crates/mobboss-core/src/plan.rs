//! Pure destination planning. Disk snapshots and checksums are collected by ops.
use crate::MediaFile;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub const MEDIA_BY_PROJECT: &str = "Media By Project";
pub const FOLDER_LIMIT: usize = 5_000;
pub const SPACE_MARGIN: u64 = 16 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Move,
    Copy,
    Collect,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub file: MediaFile,
    pub root: PathBuf,
    pub volume: String,
    pub checksum: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Existing {
    pub size: u64,
    pub checksum: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub op: Operation,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub file: MediaFile,
    pub checksum: String,
    pub duplicate: bool,
    pub step: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveSummary {
    pub root: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub required: u64,
    pub available: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub project: String,
    pub actions: Vec<Action>,
    pub drives: Vec<DriveSummary>,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
    pub log_dirs: Vec<PathBuf>,
    /// Internal collision requests; prepare resolves these before returning a plan.
    #[serde(skip)]
    pub checksum_requests: Vec<(PathBuf, PathBuf)>,
}
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c))
        .collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0');
    let s: String = s.chars().take(100).collect();
    let s = s.trim_end_matches(['.', ' ']);
    if s.is_empty() {
        "Unnamed project".into()
    } else if reserved {
        format!("_{s}")
    } else {
        s.to_string()
    }
}
pub fn project_dir(root: &Path, project: &str) -> PathBuf {
    root.join(MEDIA_BY_PROJECT).join(sanitize(project))
}
pub fn build(
    op: Operation,
    project: &str,
    inputs: &[Input],
    destination: Option<&Path>,
    existing: &BTreeMap<PathBuf, Existing>,
    space: &BTreeMap<PathBuf, u64>,
) -> Result<Plan> {
    ensure!(!inputs.is_empty(), "no indexed files selected");
    ensure!(
        op != Operation::Copy || destination.is_some(),
        "Copy needs a destination root"
    );
    let mut plan = Plan {
        project: project.into(),
        actions: vec![],
        drives: vec![],
        conflicts: vec![],
        log_dirs: vec![],
        checksum_requests: vec![],
        warnings: vec![
            "Quit Media Composer before moving media".into(),
            "Avid will rebuild its media databases on next launch".into(),
        ],
    };
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let mut occupied: BTreeMap<String, (PathBuf, Existing)> = existing
        .iter()
        .map(|(p, e)| (key(p), (p.clone(), e.clone())))
        .collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for p in existing.keys() {
        if let Some(parent) = p.parent() {
            *counts.entry(key(parent)).or_default() += 1;
        }
    }
    let mut folders: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
    for i in inputs {
        folders
            .entry(
                sanitize(
                    i.file
                        .folder
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref(),
                )
                .to_lowercase(),
            )
            .or_default()
            .insert(i.root.clone());
    }
    let mut summaries: BTreeMap<PathBuf, (usize, u64)> = BTreeMap::new();
    for input in inputs {
        ensure!(
            input
                .file
                .path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mxf")),
            "only indexed MXF media can be operated on"
        );
        let orig = sanitize(
            &input
                .file
                .folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        );
        let folder = if folders[&orig.to_lowercase()].len() > 1 {
            // Include a stable short root id even when labels are identical.
            let id = blake3::hash(input.root.to_string_lossy().as_bytes()).to_hex();
            format!("{orig} ({}-{})", sanitize(&input.volume), &id[..6])
        } else {
            orig
        };
        let mut source = input.file.path.clone();
        let mut stages = vec![];
        match op {
            Operation::Copy => stages.push((Operation::Copy, destination.unwrap().to_path_buf())),
            Operation::Move => stages.push((
                Operation::Move,
                destination.unwrap_or(&input.root).to_path_buf(),
            )),
            Operation::Collect => stages.push((Operation::Move, input.root.clone())),
        }
        if op == Operation::Collect {
            if let Some(dest) = destination {
                stages.push((Operation::Copy, dest.into()));
            }
        }
        for (step, (stage, root)) in stages.into_iter().enumerate() {
            let dir = project_dir(&root, project);
            if !plan.log_dirs.contains(&dir) {
                plan.log_dirs.push(dir.clone());
            }
            let base = dir.join("Avid MediaFiles/MXF");
            let mut n = 1;
            let dest_folder = loop {
                let p = base.join(if n == 1 {
                    folder.clone()
                } else {
                    format!("{folder}.{n}")
                });
                let candidate = p.join(input.file.path.file_name().unwrap_or_default());
                if candidate != source {
                    if let Some((actual, e)) = occupied.get(&key(&candidate)) {
                        if existing.contains_key(actual)
                            && e.size == input.file.size
                            && (e.checksum.is_empty() || input.checksum.is_empty())
                        {
                            plan.checksum_requests
                                .push((input.file.path.clone(), actual.clone()));
                        }
                    }
                }
                let same = occupied.get(&key(&candidate)).is_some_and(|(_, e)| {
                    !input.checksum.is_empty()
                        && e.size == input.file.size
                        && e.checksum == input.checksum
                });
                if counts.get(&key(&p)).copied().unwrap_or(0) < FOLDER_LIMIT
                    || same
                    || candidate == source
                {
                    break p;
                }
                n += 1;
            };
            let filename = input.file.path.file_name().unwrap_or_default();
            let mut dest = dest_folder.join(filename);
            if source == dest {
                plan.warnings
                    .push(format!("Already collected: {}", source.display()));
                continue;
            }
            let mut duplicate = false;
            let mut suffix = 0;
            loop {
                // Case insensitive reservation also prevents clashes on Mac/Windows.
                let found = occupied.get(&key(&dest));
                if let Some((actual_path, e)) = found {
                    if existing.contains_key(actual_path)
                        && e.size == input.file.size
                        && (e.checksum.is_empty() || input.checksum.is_empty())
                    {
                        plan.checksum_requests
                            .push((input.file.path.clone(), actual_path.clone()));
                    }
                    if !input.checksum.is_empty()
                        && e.size == input.file.size
                        && e.checksum == input.checksum
                    {
                        dest = actual_path.clone();
                        duplicate = true;
                        break;
                    }
                    suffix += 1;
                    let stem = Path::new(filename)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy();
                    dest = dest_folder.join(format!("{stem}_dup{suffix}.mxf"));
                } else {
                    break;
                }
            }
            if suffix > 0 {
                plan.conflicts
                    .push(format!("Filename conflict; renamed to {}", dest.display()));
            }
            if duplicate {
                plan.warnings.push(format!(
                    "Duplicate skipped; source retained: {}",
                    source.display()
                ));
            }
            if !duplicate {
                occupied.insert(
                    key(&dest),
                    (
                        dest.clone(),
                        Existing {
                            size: input.file.size,
                            checksum: input.checksum.clone(),
                        },
                    ),
                );
                *counts.entry(key(&dest_folder)).or_default() += 1;
            }
            let summary = summaries.entry(root).or_default();
            summary.0 += 1;
            if !duplicate {
                summary.1 += input.file.size;
            }
            plan.actions.push(Action {
                op: stage,
                source: source.clone(),
                dest: dest.clone(),
                file: input.file.clone(),
                checksum: input.checksum.clone(),
                duplicate,
                step,
            });
            // Duplicate moves retain their indexed source for the optional copy.
            if !duplicate {
                source = dest;
            }
        }
    }
    plan.actions.sort_by_key(|a| a.step);
    for (root, (files, bytes)) in summaries {
        let available = *space.get(&root).unwrap_or(&0);
        let required = bytes
            .saturating_add(bytes / 20)
            .saturating_add(SPACE_MARGIN);
        if available < required {
            plan.warnings.push(format!(
                "Insufficient free space at {}: need {required}, available {available}",
                root.display()
            ));
        }
        plan.drives.push(DriveSummary {
            root,
            files,
            bytes,
            required,
            available,
        });
    }
    Ok(plan)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaMetadata;
    fn input(root: &str, name: &str) -> Input {
        let root = PathBuf::from(root);
        let folder = root.join("Avid MediaFiles/MXF/1");
        Input {
            file: MediaFile {
                path: folder.join(name),
                folder,
                size: 5,
                modified_ns: "0".into(),
                metadata: MediaMetadata::default(),
            },
            root,
            volume: "Drive".into(),
            checksum: "abc".into(),
        }
    }
    fn plan(inputs: &[Input], existing: &BTreeMap<PathBuf, Existing>) -> Plan {
        build(
            Operation::Copy,
            "Project",
            inputs,
            Some(Path::new("/shuttle")),
            existing,
            &BTreeMap::from([(PathBuf::from("/shuttle"), u64::MAX)]),
        )
        .unwrap()
    }
    #[test]
    fn portable_names() {
        for (a, b) in [
            ("A<>:\"/\\|?*B. ", "AB"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("LPT1", "_LPT1"),
            ("  ", "Unnamed project"),
            ("Film 2026", "Film 2026"),
        ] {
            assert_eq!(sanitize(a), b);
        }
    }
    #[test]
    fn drive_collisions_and_collect_stages() {
        let inputs = [input("/drive-a", "a.mxf"), input("/drive-b", "a.mxf")];
        let p = build(
            Operation::Collect,
            "Project",
            &inputs,
            Some(Path::new("/shuttle")),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(p.actions.len(), 4);
        assert_ne!(p.actions[2].dest, p.actions[3].dest);
        assert_eq!(p.actions[2].source, p.actions[0].dest);
        assert_eq!(p.actions[0].op, Operation::Move);
        assert_eq!(p.actions[2].op, Operation::Copy);
        assert!(p.actions[0]
            .dest
            .starts_with("/drive-a/Media By Project/Project"));
    }
    #[test]
    fn duplicate_and_conflict() {
        let i = input("/drive", "a.mxf");
        let dest = plan(std::slice::from_ref(&i), &BTreeMap::new()).actions[0]
            .dest
            .clone();
        let same = BTreeMap::from([(
            dest.clone(),
            Existing {
                size: 5,
                checksum: "abc".into(),
            },
        )]);
        assert!(plan(std::slice::from_ref(&i), &same).actions[0].duplicate);
        let different = BTreeMap::from([(
            dest,
            Existing {
                size: 5,
                checksum: "different".into(),
            },
        )]);
        let p = plan(&[i], &different);
        assert!(p.actions[0].dest.ends_with("a_dup1.mxf"));
        assert_eq!(p.conflicts.len(), 1);
    }
    #[test]
    fn splits_at_five_thousand() {
        let inputs: Vec<_> = (0..5001)
            .map(|n| input("/drive", &format!("{n}.mxf")))
            .collect();
        let p = plan(&inputs, &BTreeMap::new());
        assert!(p.actions[4999].dest.parent().unwrap().ends_with("1"));
        assert!(p.actions[5000].dest.parent().unwrap().ends_with("1.2"));
    }
    #[test]
    fn case_conflicts_and_space_margin() {
        let p = plan(
            &[input("/a", "A.mxf"), input("/a", "a.mxf")],
            &BTreeMap::new(),
        );
        assert!(p.actions[1].duplicate);
        assert_eq!(p.drives[0].bytes, 5);
        assert_eq!(p.drives[0].required, SPACE_MARGIN + 5);
    }
    #[test]
    fn copy_requires_destination_and_selection() {
        assert!(build(
            Operation::Copy,
            "P",
            &[],
            None,
            &BTreeMap::new(),
            &BTreeMap::new()
        )
        .is_err());
        assert!(build(
            Operation::Copy,
            "P",
            &[input("/a", "a.mxf")],
            None,
            &BTreeMap::new(),
            &BTreeMap::new()
        )
        .is_err());
    }
    #[test]
    fn full_folder_still_skips_existing_duplicate() {
        let i = input("/drive", "a.mxf");
        let target = plan(std::slice::from_ref(&i), &BTreeMap::new()).actions[0]
            .dest
            .clone();
        let existing = (0..FOLDER_LIMIT)
            .map(|n| {
                (
                    target.with_file_name(if n == 0 {
                        "a.mxf".into()
                    } else {
                        format!("other{n}.mxf")
                    }),
                    Existing {
                        size: 5,
                        checksum: "abc".into(),
                    },
                )
            })
            .collect();
        let p = plan(&[i], &existing);
        assert!(p.actions[0].duplicate);
        assert_eq!(p.actions[0].dest, target);
    }
    #[test]
    fn move_accepts_another_drive_root() {
        let p = build(
            Operation::Move,
            "Project",
            &[input("/drive", "a.mxf")],
            Some(Path::new("/other")),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.actions[0]
            .dest
            .starts_with("/other/Media By Project/Project"));
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("Insufficient free space")));
    }
    #[test]
    fn collect_duplicate_copies_from_retained_indexed_source() {
        let i = input("/drive", "a.mxf");
        let target = project_dir(&i.root, "Project").join("Avid MediaFiles/MXF/1/a.mxf");
        let p = build(
            Operation::Collect,
            "Project",
            std::slice::from_ref(&i),
            Some(Path::new("/other")),
            &BTreeMap::from([(
                target,
                Existing {
                    size: 5,
                    checksum: "abc".into(),
                },
            )]),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(p.actions[0].duplicate);
        assert_eq!(p.actions[1].source, i.file.path);
    }
    #[test]
    fn databases_never_become_actions() {
        assert!(build(
            Operation::Move,
            "P",
            &[input("/drive", "msmFMID.pmr")],
            None,
            &BTreeMap::new(),
            &BTreeMap::new()
        )
        .is_err());
        assert!(build(
            Operation::Move,
            "P",
            &[input("/drive", "msmMMOB.mdb")],
            None,
            &BTreeMap::new(),
            &BTreeMap::new()
        )
        .is_err());
    }
    #[test]
    fn truncated_name_has_no_trailing_dot() {
        assert!(!sanitize(&format!("{}.more", "a".repeat(99))).ends_with('.'));
    }
}
