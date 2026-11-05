use super::{capture, digest, refused, CapturedPlan, Result};
use crate::artifacts::{files, native};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn endpoint(path: &Path) -> Result<(PathBuf, PathBuf)> {
    if !path.is_absolute() {
        return Err(refused());
    }
    let named_parent = path.parent().ok_or_else(refused)?.to_path_buf();
    let parent = named_parent.canonicalize()?;
    let name = path.file_name().ok_or_else(refused)?;
    Ok((parent.join(name), named_parent))
}

fn same_parent(named_parent: &Path, parent: &Dir) -> Result<()> {
    native::same_directory(
        parent,
        &Dir::open_ambient_dir(named_parent, cap_std::ambient_authority())?,
    )
}

fn dedicated_parent(plan: &CapturedPlan, path: &Path, parent: &Dir) -> Result<()> {
    let identity = native::DirectoryIdentity::of(parent)?;
    if identity == native::DirectoryIdentity::of(&plan.pins.destination_parent)? {
        return Err(refused());
    }
    let roots = plan
        .pins
        .roots
        .values()
        .map(native::DirectoryIdentity::of)
        .collect::<Result<Vec<_>>>()?;
    for ancestor in path.parent().ok_or_else(refused)?.ancestors() {
        let directory = Dir::open_ambient_dir(ancestor, cap_std::ambient_authority())?;
        if roots.contains(&native::DirectoryIdentity::of(&directory)?) {
            return Err(refused());
        }
    }
    same_parent(path.parent().ok_or_else(refused)?, parent)
}

pub(super) fn save(plan: &CapturedPlan, path: &Path) -> Result<()> {
    let (path, named_parent) = endpoint(path)?;
    let (parent, name) = crate::backup::destination(&path)?;
    dedicated_parent(plan, &path, &parent)?;
    same_parent(&named_parent, &parent)?;
    let bytes = plan.encode_private()?;
    capture::with_verified_source(plan, || {
        dedicated_parent(plan, &path, &parent)?;
        same_parent(&named_parent, &parent)?;
        let mut preparation = native::Preparation::new(&parent)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = preparation.dir.open_with("plan.json", &options)?;
        native::verify_private_directory(&preparation.dir)?;
        native::verify_private_file(&file)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        native::verify_private_file(&file)?;
        drop(file);
        native::sync_preparation(&preparation.dir)?;
        native::verify_private_directory(&preparation.dir)?;
        same_parent(&named_parent, &parent)?;
        preparation.publish(&parent, &name)?;
        same_parent(&named_parent, &parent)?;
        Ok(())
    })
}

pub(super) fn load(path: &Path, expected_digest: &str) -> Result<CapturedPlan> {
    let (path, named_parent) = endpoint(path)?;
    for ancestor in path.parent().ok_or_else(refused)?.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(refused()),
        }
    }
    let parent = files::open_root(path.parent().ok_or_else(refused)?)?;
    let directory = parent.open_dir_nofollow(path.file_name().ok_or_else(refused)?)?;
    native::verify_private_directory(&directory)?;
    let mut entries = directory.entries()?;
    let entry = entries.next().ok_or_else(refused)??;
    if entry.file_name() != "plan.json" || entries.next().is_some() {
        return Err(refused());
    }
    let mut file = files::regular(&directory, "plan.json")?;
    files::require_single_link(&file)?;
    native::verify_private_file(&file)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(crate::backup::MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > crate::backup::MAX_MANIFEST || digest(&bytes) != expected_digest {
        return Err(refused());
    }
    let plan = CapturedPlan::decode_private(&bytes)?;
    if plan.encode_private()? != bytes {
        return Err(refused());
    }
    dedicated_parent(&plan, &path, &parent)?;
    native::verify_private_directory(&directory)?;
    native::verify_private_file(&file)?;
    files::require_single_link(&file)?;
    same_parent(&named_parent, &parent)?;
    native::same_directory(
        &directory,
        &parent.open_dir_nofollow(path.file_name().ok_or_else(refused)?)?,
    )?;
    Ok(plan)
}
