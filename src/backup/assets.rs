use crate::failure::Failure;
use crate::model::Tree;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AssetKind {
    Artifact,
    Agent,
}

#[derive(Clone, Debug)]
pub(super) struct AssetFile {
    pub lane: String,
    pub path: String,
    pub digest: String,
    pub kind: AssetKind,
    pub harness: Option<String>,
}

pub(super) fn required_lanes(tree: &Tree) -> Result<BTreeSet<String>, Failure> {
    let mut lanes = crate::artifacts::backup_lanes(tree)?;
    lanes.extend(crate::agents::backup_lanes(tree)?);
    Ok(lanes)
}

#[cfg(test)]
pub(super) fn collect(
    tree: &Tree,
    roots: &BTreeMap<String, PathBuf>,
) -> Result<Vec<AssetFile>, Failure> {
    let mut directories = BTreeMap::new();
    for lane in required_lanes(tree)? {
        let root = roots.get(&lane).ok_or_else(|| {
            Failure::usage("A required custody lane needs an explicit --lane-root.")
        })?;
        directories.insert(lane, crate::artifacts::files::open_root(root)?);
    }
    collect_with_directories(tree, roots, &directories)
}

pub(super) fn collect_with_directories(
    tree: &Tree,
    roots: &BTreeMap<String, PathBuf>,
    directories: &BTreeMap<String, cap_std::fs::Dir>,
) -> Result<Vec<AssetFile>, Failure> {
    let mut files = BTreeMap::<(String, String), AssetFile>::new();
    for lane in required_lanes(tree)? {
        let root = roots.get(&lane).ok_or_else(|| {
            Failure::usage("A required custody lane needs an explicit --lane-root.")
        })?;
        let directory = directories.get(&lane).ok_or_else(|| {
            Failure::Model("A required custody lane has no pinned directory.".into())
        })?;
        let artifacts =
            crate::artifacts::backup_files_in(tree, &lane, directory).map_err(|_| {
                Failure::Model("Required artifact custody could not be verified.".into())
            })?;
        for (path, digest) in artifacts {
            insert(
                &mut files,
                AssetFile {
                    lane: lane.clone(),
                    path,
                    digest,
                    kind: AssetKind::Artifact,
                    harness: None,
                },
            )?;
        }
        let agents =
            crate::agents::backup_files_in(tree, &lane, root, directory).map_err(|_| {
                Failure::Model("Required native agent custody could not be verified.".into())
            })?;
        for (path, digest, harness) in agents {
            insert(
                &mut files,
                AssetFile {
                    lane: lane.clone(),
                    path,
                    digest,
                    kind: AssetKind::Agent,
                    harness: Some(harness),
                },
            )?;
        }
    }
    Ok(files.into_values().collect())
}

fn insert(
    files: &mut BTreeMap<(String, String), AssetFile>,
    file: AssetFile,
) -> Result<(), Failure> {
    if file.path.is_empty()
        || file.path.len() > 1024
        || file.path.contains(['\\', ':'])
        || file
            .path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || Path::new(&file.path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Failure::usage("Invalid backup custody member path."));
    }
    let key = (file.lane.clone(), file.path.to_lowercase());
    if let Some(previous) = files.get(&key) {
        if previous.path != file.path
            || previous.digest != file.digest
            || previous.kind != file.kind
            || previous.harness != file.harness
        {
            return Err(Failure::Model(
                "Conflicting backup custody member paths.".into(),
            ));
        }
        return Ok(());
    }
    for ((lane, path), previous) in files.iter() {
        if lane != &key.0 {
            continue;
        }
        if path.starts_with(&format!("{}/", key.1)) || key.1.starts_with(&format!("{path}/")) {
            return Err(Failure::Model(
                "Conflicting backup custody member paths.".into(),
            ));
        }
        for (before, after) in previous.path.split('/').zip(file.path.split('/')) {
            if before == after {
                continue;
            }
            if before.to_lowercase() == after.to_lowercase() {
                return Err(Failure::Model(
                    "Conflicting backup custody member paths.".into(),
                ));
            }
            break;
        }
    }
    if files.len() >= 4096 {
        return Err(Failure::usage("Backup custody exceeds the file limit."));
    }
    files.insert(key, file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_collection_does_not_open_roots_without_custody_references() {
        let roots = BTreeMap::from([("main".to_owned(), PathBuf::from("absent-unused-lane-root"))]);
        assert!(collect(&Tree::default(), &roots).unwrap().is_empty());
    }

    #[test]
    fn custody_paths_reject_case_aliases_and_file_directory_conflicts() {
        let asset = |path: &str| AssetFile {
            lane: "main".into(),
            path: path.into(),
            digest: "a".repeat(64),
            kind: AssetKind::Artifact,
            harness: None,
        };
        let mut files = BTreeMap::new();
        insert(&mut files, asset(".vivac-artifacts/objects/example.blob")).unwrap();
        insert(&mut files, asset(".vivac-artifacts/objects/example.blob")).unwrap();
        assert_eq!(files.len(), 1);
        for path in [
            ".vivac-artifacts/objects/EXAMPLE.blob",
            ".vivac-artifacts/objects",
            ".vivac-artifacts/objects/example.blob/member",
            ".vivac-artifacts/../outside",
            ".vivac-artifacts//member",
        ] {
            assert!(insert(&mut files, asset(path)).is_err(), "{path}");
        }
        assert_eq!(files.len(), 1);
        insert(&mut files, asset(".vivac-artifacts/objects/Bundle/first")).unwrap();
        assert!(insert(&mut files, asset(".vivac-artifacts/objects/bundle/second")).is_err());
    }
}
