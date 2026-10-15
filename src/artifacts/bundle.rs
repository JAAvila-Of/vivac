use super::{files, guard, native::Preparation};
use crate::failure::Failure;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, Metadata, OpenOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::path::Path;

const MANIFEST: &str = ".vivac-delivery.json";
pub(crate) const MAX_MEMBERS: usize = 64;
pub(crate) const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub(crate) const SUPPORTED: bool = super::native::SUPPORTED;
pub(crate) const SQLITE_PROFILE: &str = "sqlite-standalone-v1";
const SQLITE_TRANSFORMATION: &str = "logical-export-v1";
const SQLITE_MEDIA_TYPE: &str = "application/vnd.sqlite3";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundleSelection {
    pub(crate) root: String,
    pub(crate) entrypoint: String,
    pub(crate) members: Vec<String>,
    #[serde(default)]
    pub(crate) requirements: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) database: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) schema_version: u32,
    pub(crate) delivery_type: String,
    pub(crate) entrypoint: String,
    pub(crate) requirements: Vec<String>,
    pub(crate) members: Vec<Member>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_database"
    )]
    pub(crate) database: Option<DatabaseMetadata>,
    #[serde(skip)]
    pub(crate) manifest_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseMetadata {
    pub(crate) path: String,
    pub(crate) profile: String,
    pub(crate) transformation: String,
    pub(crate) source_bytes: u64,
    pub(crate) source_sha256: String,
}
fn deserialize_database<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DatabaseMetadata>, D::Error> {
    DatabaseMetadata::deserialize(deserializer).map(Some)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Member {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    pub(crate) media_type: String,
}
pub(crate) struct RetainedBundle {
    pub(crate) retained: files::Retained,
    pub(crate) entrypoint: String,
    pub(crate) members: usize,
    pub(crate) manifest_bytes: u64,
}
pub(crate) struct PreviewResources {
    pub(crate) entrypoint: String,
    pub(crate) members: BTreeMap<String, PreviewMember>,
}
pub(crate) struct PreviewMember {
    pub(crate) media_type: String,
    pub(crate) reader: files::Download,
}
fn media_type(path: &str) -> Result<&'static str, Failure> {
    let extension = path.rsplit('.').next().unwrap_or_default();
    if extension.eq_ignore_ascii_case("html") {
        Ok("text/html")
    } else if extension.eq_ignore_ascii_case("css") {
        Ok("text/css")
    } else if extension.eq_ignore_ascii_case("js") {
        Ok("text/javascript")
    } else if ["sqlite", "sqlite3", "db"]
        .iter()
        .any(|value| extension.eq_ignore_ascii_case(value))
    {
        Ok(SQLITE_MEDIA_TYPE)
    } else {
        Err(Failure::usage(
            "Bundle members must be HTML, CSS, or JavaScript files.",
        ))
    }
}
pub(crate) fn validate_path(path: &str) -> Result<(), Failure> {
    member_path(path)
}
pub(crate) fn validate_database_path(path: &str) -> Result<(), Failure> {
    member_path(path)?;
    if media_type(path)? != SQLITE_MEDIA_TYPE {
        return Err(Failure::usage(
            "The database must be a selected SQLite member.",
        ));
    }
    Ok(())
}
fn member_path(path: &str) -> Result<(), Failure> {
    if path
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(Failure::usage(
            "Bundle member paths must not contain empty or dot components.",
        ));
    }
    files::source_path(path)?;
    if path.len() > 240
        || path.eq_ignore_ascii_case(MANIFEST)
        || path.split('/').any(|part| {
            part.len() > 64
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return Err(Failure::usage(
            "Bundle members require unambiguous portable ASCII paths.",
        ));
    }
    media_type(path)?;
    Ok(())
}
fn source_root(root: &Dir, path: &str) -> Result<Dir, Failure> {
    if path == "." {
        return Ok(root.try_clone()?);
    }
    files::source_path(path)?;
    let mut directory = root.try_clone()?;
    for part in path.split('/') {
        files::reject_link(&directory.symlink_metadata(part)?)?;
        directory = directory.open_dir_nofollow(part)?;
    }
    Ok(directory)
}
pub(crate) fn validate_selection(selection: &BundleSelection) -> Result<(), Failure> {
    if selection.root != "." {
        files::source_path(&selection.root)?;
    }
    if selection.members.is_empty() || selection.members.len() > MAX_MEMBERS {
        return Err(Failure::usage(
            "Bundles must select between 1 and 64 members.",
        ));
    }
    member_path(&selection.entrypoint)?;
    if media_type(&selection.entrypoint)? != "text/html"
        || !selection.members.contains(&selection.entrypoint)
    {
        return Err(Failure::usage(
            "The bundle entrypoint must exactly name a selected HTML member.",
        ));
    }
    let mut folded = BTreeSet::new();
    if let Some(database) = &selection.database {
        validate_database_path(database)?;
        if !selection.members.contains(database) {
            return Err(Failure::usage(
                "The database must exactly name a selected SQLite member.",
            ));
        }
    }
    for path in &selection.members {
        member_path(path)?;
        if media_type(path)? == SQLITE_MEDIA_TYPE && selection.database.as_ref() != Some(path) {
            return Err(Failure::usage(
                "SQLite members require one explicit database selection.",
            ));
        }
        if !folded.insert(path.to_ascii_lowercase()) {
            return Err(Failure::usage(
                "Bundle member paths must not collide without case distinctions.",
            ));
        }
    }
    validate_directory_spelling(&selection.members)?;
    for path in &folded {
        let mut prefix = String::new();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            if components.peek().is_some() && folded.contains(&prefix) {
                return Err(Failure::usage(
                    "A bundle member must not be another member's parent directory.",
                ));
            }
        }
    }
    for requirement in &selection.requirements {
        if let Some(finding) = crate::redact::check_field("bundle requirement", requirement) {
            return Err(Failure::Redaction(Box::new(finding)));
        }
    }
    bounded_json(selection)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct FoldedComponent<'a>(&'a str);
impl PartialEq for FoldedComponent<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}
impl Eq for FoldedComponent<'_> {}
impl PartialOrd for FoldedComponent<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FoldedComponent<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(other.0.bytes().map(|byte| byte.to_ascii_lowercase()))
    }
}
fn validate_directory_spelling(paths: &[String]) -> Result<(), Failure> {
    // Borrow component spelling from the bounded member paths. Parent indices
    // share prefixes without allocating every ancestor's full path again.
    let mut directories: BTreeMap<(usize, FoldedComponent<'_>), (usize, &str)> = BTreeMap::new();
    let mut next_index = 1;
    for path in paths {
        let mut parent_index = 0;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                break;
            }
            let key = (parent_index, FoldedComponent(component));
            if let Some((index, spelling)) = directories.get(&key) {
                if *spelling != component {
                    return Err(Failure::usage("Bundle directory prefixes must have one exact spelling without case aliases."));
                }
                parent_index = *index;
            } else {
                let index = next_index;
                next_index += 1;
                directories.insert(key, (index, component));
                parent_index = index;
            }
        }
    }
    Ok(())
}
pub(crate) fn load_selection(root: &Path, selector: &str) -> Result<BundleSelection, Failure> {
    files::source_path(selector)?;
    let root = files::open_root(root)?;
    let mut source = Held::open(&root, selector)?;
    let bytes = bounded_read(&mut source.file, MAX_MANIFEST_BYTES)?;
    source.recheck(&root)?;
    guard::copy_checked(std::io::Cursor::new(&bytes), io::sink())?;
    let mut selection: BundleSelection = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::usage("Invalid bundle selector JSON."))?;
    validate_selection(&selection)?;
    selection.requirements.shrink_to_fit();
    Ok(selection)
}
struct Held {
    path: String,
    parent: Dir,
    name: String,
    file: File,
    before: Metadata,
    #[cfg(windows)]
    identity: (u64, [u8; 16]),
}
impl Held {
    fn open(root: &Dir, path: &str) -> Result<Self, Failure> {
        let (parent, name) = member_parent(root, path)?;
        let file = files::regular(&parent, &name)?;
        let before = file.metadata()?;
        #[cfg(windows)]
        let identity = files::file_identity(&file)?;
        Ok(Self {
            path: path.into(),
            parent,
            name,
            file,
            before,
            #[cfg(windows)]
            identity,
        })
    }
    fn recheck(&self, root: &Dir) -> Result<(), Failure> {
        files::unchanged(&self.before, &self.file.metadata()?)?;
        let current = files::regular(root, &self.path)?;
        files::unchanged(&self.before, &current.metadata()?)?;
        #[cfg(windows)]
        {
            files::same_source(&self.file, &current)?;
            if self.identity != files::file_identity(&self.file)? {
                return Err(Failure::usage(
                    "The bundle source identity changed while preparing the revision.",
                ));
            }
        }
        Ok(())
    }
}
fn recheck_all(root: &Dir, held: &[Held]) -> Result<(), Failure> {
    for source in held {
        source.recheck(root)?;
    }
    Ok(())
}

fn member_parent(root: &Dir, path: &str) -> Result<(Dir, String), Failure> {
    files::source_path(path)?;
    let mut parent = root.try_clone()?;
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            return Ok((parent, component.into()));
        }
        files::reject_link(&parent.symlink_metadata(component)?)?;
        parent = parent.open_dir_nofollow(component)?;
    }
    Err(Failure::usage("Invalid SQLite member path."))
}

fn check_auxiliary(parent: &Dir, name: &str) -> Result<(), Failure> {
    for suffix in ["-wal", "-shm", "-journal"] {
        match parent.symlink_metadata(format!("{name}{suffix}")) {
            Ok(_) => return Err(Failure::usage("SQLite auxiliary files are not supported.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn copy_binary_limit(
    source: &File,
    mut writer: impl Write,
    limit: u64,
) -> Result<guard::Receipt, Failure> {
    let reader = source.try_clone()?.into_std();
    let mut buffer = [0; 64 * 1024];
    let mut digest = Sha256::new();
    let mut size = 0u64;
    loop {
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(&reader, &mut buffer, size);
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(&reader, &mut buffer, size);
        #[cfg(not(any(unix, windows)))]
        let read = {
            use std::io::{Seek, SeekFrom};
            let mut reader = &reader;
            reader.seek(SeekFrom::Start(size))?;
            reader.read(&mut buffer)
        };
        let count = match read {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| Failure::usage("Bundle size overflow."))?;
        if size > limit {
            return Err(Failure::usage("The SQLite member exceeds its byte budget."));
        }
        digest.update(&buffer[..count]);
        writer.write_all(&buffer[..count])?;
    }
    Ok(guard::Receipt {
        digest: format!("{:x}", digest.finalize()),
        size,
    })
}

fn recheck_selected_root(
    project: &Dir,
    original: &Dir,
    path: &str,
    held: &[Held],
) -> Result<(), Failure> {
    let current = source_root(project, path)?;
    super::native::same_directory(original, &current)?;
    recheck_all(&current, held)
}
fn bounded_read(source: &mut File, limit: u64) -> Result<Vec<u8>, Failure> {
    if source.metadata()?.len() > limit {
        return Err(Failure::usage("Bundle metadata exceeds 64 KiB."));
    }
    let mut bytes = Vec::new();
    source.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Failure::usage("Bundle metadata exceeds 64 KiB."));
    }
    Ok(bytes)
}
fn object_path(object: &str) -> Result<&str, Failure> {
    let revision = object
        .strip_prefix("objects/")
        .and_then(|value| value.strip_suffix(".bundle"))
        .ok_or_else(|| Failure::usage("Invalid bundle object path."))?;
    files::opaque_id(revision)?;
    Ok(revision)
}
fn new_member(root: &Dir, path: &str) -> Result<File, Failure> {
    let mut parent = root.try_clone()?;
    let mut at_root = true;
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            return Ok(parent.open_with(component, &options)?);
        }
        parent = new_directory(&parent, component, at_root)?;
        at_root = false;
    }
    Err(Failure::usage("Invalid bundle member path."))
}
fn new_directory(parent: &Dir, name: &str, at_root: bool) -> Result<Dir, Failure> {
    match parent.create_dir(name) {
        Ok(()) => {
            if at_root {
                // The private root retains DELETE access without sharing it;
                // synchronize that exact handle, rather than reopening '.'.
                super::native::sync_preparation(parent)?;
            } else {
                files::sync_directory(parent)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    files::reject_link(&parent.symlink_metadata(name)?)?;
    Ok(parent.open_dir_nofollow(name)?)
}
fn sync_tree(root: &Dir) -> Result<(), Failure> {
    sync_children(root)?;
    super::native::sync_preparation(root)
}
fn sync_children(root: &Dir) -> Result<(), Failure> {
    for entry in root.entries()? {
        let entry = entry?;
        let metadata = root.symlink_metadata(entry.file_name())?;
        files::reject_link(&metadata)?;
        if metadata.is_dir() {
            let child = root.open_dir_nofollow(entry.file_name())?;
            sync_children(&child)?;
            files::sync_directory(&child)?;
        }
    }
    Ok(())
}
fn bounded_json(value: &impl Serialize) -> Result<Vec<u8>, Failure> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len() + bytes.len() > MAX_MANIFEST_BYTES as usize {
                return Err(io::Error::other("Bundle metadata exceeds 64 KiB."));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded(Vec::with_capacity(MAX_MANIFEST_BYTES as usize));
    serde_json::to_writer(&mut output, value)
        .map_err(|_| Failure::usage("Bundle metadata exceeds 64 KiB or cannot be serialized."))?;
    Ok(output.0)
}
fn manifest_bytes(manifest: &impl Serialize) -> Result<Vec<u8>, Failure> {
    bounded_json(manifest)
}
#[derive(Serialize)]
struct BorrowedManifest<'a> {
    schema_version: u32,
    delivery_type: &'a str,
    entrypoint: &'a str,
    requirements: &'a [String],
    members: &'a [Member],
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<&'a DatabaseMetadata>,
}
fn write_manifest(root: &Dir, bytes: &[u8]) -> Result<(), Failure> {
    let mut output = new_member(root, MANIFEST)?;
    guard::copy_checked(std::io::Cursor::new(bytes), &mut output)?;
    output.sync_all()?;
    Ok(())
}

pub(crate) fn capture(
    root: &Path,
    selection: &BundleSelection,
    revision: &str,
) -> Result<RetainedBundle, Failure> {
    require_supported()?;
    validate_selection(selection)?;
    files::opaque_id(revision)?;
    let root_dir = files::open_root(root)?;
    super::native::require_controlled_parent(&root_dir)?;
    let source_dir = source_root(&root_dir, &selection.root)?;
    let mut paths = selection.members.clone();
    paths.sort();
    let mut sources = Vec::with_capacity(paths.len());
    let mut expected_bytes = 0u64;
    for path in paths {
        let source = Held::open(&source_dir, &path)?;
        expected_bytes = expected_bytes
            .checked_add(source.before.len())
            .ok_or_else(|| Failure::usage("Bundle size overflow."))?;
        if expected_bytes > MAX_BYTES {
            return Err(Failure::usage("The complete bundle exceeds 10 MiB."));
        }
        sources.push(source);
    }
    files::check_tracking(root)?;
    let store_parent = files::directory(&root_dir, ".vivac-artifacts", true)?;
    super::native::require_controlled_parent(&store_parent)?;
    let store = files::store(&root_dir, true)?;
    let objects = files::directory(&store, "objects", true)?;
    let object = format!("objects/{revision}.bundle");
    let mut preparation = Preparation::new(&objects)?;
    files::check_git(root, &object, &preparation.name)?;
    let mut total = 0u64;
    let mut manifest = Manifest {
        schema_version: if selection.database.is_some() { 2 } else { 1 },
        delivery_type: if selection.database.is_some() {
            "sqlite_application"
        } else {
            "bundle"
        }
        .into(),
        entrypoint: selection.entrypoint.clone(),
        requirements: vec![],
        database: None,
        members: Vec::with_capacity(sources.len()),
        manifest_bytes: 0,
    };
    for source in &mut sources {
        let mut output = new_member(&preparation.dir, &source.path)?;
        let receipt = if selection.database.as_ref() == Some(&source.path) {
            let snapshot = super::sqlite::normalize(&source.file, &output, || {
                check_auxiliary(&source.parent, &source.name)
            })?;
            if snapshot.bytes > MAX_BYTES - total {
                return Err(Failure::usage("The complete bundle exceeds 10 MiB."));
            }
            manifest.database = Some(DatabaseMetadata {
                path: source.path.clone(),
                profile: SQLITE_PROFILE.into(),
                transformation: SQLITE_TRANSFORMATION.into(),
                source_bytes: snapshot.source_bytes,
                source_sha256: snapshot.source_sha256,
            });
            guard::Receipt {
                digest: snapshot.sha256,
                size: snapshot.bytes,
            }
        } else {
            guard::copy_checked_limit(&mut source.file, &mut output, MAX_BYTES - total)?
        };
        total += receipt.size;
        output.sync_all()?;
        manifest.members.push(Member {
            path: source.path.clone(),
            bytes: receipt.size,
            sha256: receipt.digest,
            media_type: media_type(&source.path)?.into(),
        });
    }
    recheck_all(&source_dir, &sources)?;
    let bytes = manifest_bytes(&BorrowedManifest {
        schema_version: manifest.schema_version,
        delivery_type: &manifest.delivery_type,
        entrypoint: &manifest.entrypoint,
        requirements: &selection.requirements,
        members: &manifest.members,
        database: manifest.database.as_ref(),
    })?;
    total = total
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| Failure::usage("Bundle size overflow."))?;
    if total > MAX_BYTES {
        return Err(Failure::usage(
            "The complete bundle, including its manifest, exceeds 10 MiB.",
        ));
    }
    write_manifest(&preparation.dir, &bytes)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    // Verify the retained preparation itself before publication, rather than
    // trusting a second serialization or reopening the selected source bytes.
    verify_directory(&preparation.dir, Some((&digest, total)), None)?;
    recheck_all(&source_dir, &sources)?;
    recheck_selected_root(&root_dir, &source_dir, &selection.root, &sources)?;
    sync_tree(&preparation.dir)?;
    let site = files::site(&store)?;
    files::check_git(root, &object, &preparation.name)?;
    preparation.publish(&objects, &format!("{revision}.bundle"))?;
    Ok(RetainedBundle {
        retained: files::Retained {
            digest,
            size: total,
            object,
            site,
        },
        entrypoint: manifest.entrypoint,
        members: manifest.members.len(),
        manifest_bytes: bytes.len() as u64,
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), Failure> {
    if !matches!(
        (
            manifest.schema_version,
            manifest.delivery_type.as_str(),
            manifest.database.as_ref()
        ),
        (1, "bundle", None) | (2, "sqlite_application", Some(_))
    ) {
        return Err(Failure::usage("Unsupported bundle manifest schema."));
    }
    let selection = BundleSelection {
        root: ".".into(),
        entrypoint: manifest.entrypoint.clone(),
        members: manifest
            .members
            .iter()
            .map(|member| member.path.clone())
            .collect(),
        requirements: vec![],
        database: manifest
            .database
            .as_ref()
            .map(|database| database.path.clone()),
    };
    validate_selection(&selection)?;
    if let Some(database) = &manifest.database {
        if database.profile != SQLITE_PROFILE
            || database.transformation != SQLITE_TRANSFORMATION
            || database.source_bytes > MAX_BYTES
            || database.source_bytes < 512
            || !valid_digest(&database.source_sha256)
        {
            return Err(Failure::usage("Invalid SQLite application metadata."));
        }
    }
    for requirement in &manifest.requirements {
        if let Some(finding) = crate::redact::check_field("bundle requirement", requirement) {
            return Err(Failure::Redaction(Box::new(finding)));
        }
    }
    let mut previous = None;
    for member in &manifest.members {
        if previous.is_some_and(|value: &str| value >= member.path.as_str()) {
            return Err(Failure::usage(
                "Bundle manifest members must be sorted and unique.",
            ));
        }
        previous = Some(member.path.as_str());
        if member.bytes > MAX_BYTES
            || member.media_type != media_type(&member.path)?
            || member.sha256.len() != 64
            || !member
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(Failure::usage("Invalid bundle member metadata."));
        }
    }
    Ok(())
}
fn load_manifest(
    root: &Dir,
    expected: Option<(&str, u64)>,
) -> Result<(Manifest, Vec<u8>, Held), Failure> {
    let mut source = Held::open(root, MANIFEST)?;
    let bytes = bounded_read(&mut source.file, MAX_MANIFEST_BYTES)?;
    guard::copy_checked(std::io::Cursor::new(&bytes), io::sink())?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if let Some((expected_digest, _)) = expected {
        if digest != expected_digest {
            return Err(Failure::Model(
                "The saved bundle manifest does not match the revision digest.".into(),
            ));
        }
    }
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::usage("Invalid retained bundle manifest JSON."))?;
    manifest.manifest_bytes = bytes.len() as u64;
    manifest.requirements.shrink_to_fit();
    validate_manifest(&manifest)?;
    let total = manifest
        .members
        .iter()
        .try_fold(bytes.len() as u64, |sum, member| {
            sum.checked_add(member.bytes)
        })
        .ok_or_else(|| Failure::usage("Bundle size overflow."))?;
    if total > MAX_BYTES || expected.is_some_and(|(_, size)| size != total) {
        return Err(Failure::Model(
            "The saved bundle does not match the revision size limit or total.".into(),
        ));
    }
    source.recheck(root)?;
    Ok((manifest, bytes, source))
}
fn expected_paths(manifest: &Manifest) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut files = BTreeSet::from([MANIFEST.into()]);
    let mut directories = BTreeSet::new();
    for member in &manifest.members {
        files.insert(member.path.clone());
        let mut path = member.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            directories.insert(parent.into());
            path = parent;
        }
    }
    (files, directories)
}
fn exact_set(
    root: &Dir,
    prefix: &str,
    expected_files: &BTreeSet<String>,
    expected_dirs: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> Result<(), Failure> {
    for entry in root.entries()? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Failure::usage("Invalid bundle object member name."))?;
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = root.symlink_metadata(&name)?;
        files::reject_link(&metadata)?;
        if metadata.is_dir() {
            if !expected_dirs.contains(&path) {
                return Err(Failure::Model(
                    "The saved bundle contains an unexpected directory.".into(),
                ));
            }
            exact_set(
                &root.open_dir_nofollow(name)?,
                &path,
                expected_files,
                expected_dirs,
                seen,
            )?;
        } else {
            if !metadata.is_file() || !expected_files.contains(&path) {
                return Err(Failure::Model(
                    "The saved bundle contains an unexpected member.".into(),
                ));
            }
            seen.insert(path);
        }
    }
    Ok(())
}
fn verify_directory(
    root: &Dir,
    expected: Option<(&str, u64)>,
    destination: Option<&Dir>,
) -> Result<Manifest, Failure> {
    let (manifest, bytes, manifest_source) = load_manifest(root, expected)?;
    let (expected_files, expected_dirs) = expected_paths(&manifest);
    let mut seen = BTreeSet::new();
    exact_set(root, "", &expected_files, &expected_dirs, &mut seen)?;
    if seen != expected_files {
        return Err(Failure::Model(
            "The saved bundle is missing a declared member.".into(),
        ));
    }
    let mut sources = Vec::with_capacity(manifest.members.len());
    for member in &manifest.members {
        sources.push(Held::open(root, &member.path)?);
    }
    for (source, member) in sources.iter_mut().zip(&manifest.members) {
        let binary = manifest
            .database
            .as_ref()
            .is_some_and(|database| database.path == member.path);
        if binary {
            let inspected = super::sqlite::inspect(&source.file, || {
                check_auxiliary(&source.parent, &source.name)
            })?;
            files::matching(
                &inspected.digest,
                inspected.size,
                &member.sha256,
                member.bytes,
            )?;
        }
        let receipt = if let Some(destination) = destination {
            let mut output = new_member(destination, &member.path)?;
            let receipt = if binary {
                copy_binary_limit(&source.file, &mut output, member.bytes)?
            } else {
                guard::copy_checked_limit(&mut source.file, &mut output, member.bytes)?
            };
            output.sync_all()?;
            receipt
        } else {
            if binary {
                copy_binary_limit(&source.file, io::sink(), member.bytes)?
            } else {
                guard::copy_checked_limit(&mut source.file, io::sink(), member.bytes)?
            }
        };
        files::matching(&receipt.digest, receipt.size, &member.sha256, member.bytes)?;
    }
    recheck_all(root, &sources)?;
    manifest_source.recheck(root)?;
    let mut final_seen = BTreeSet::new();
    exact_set(root, "", &expected_files, &expected_dirs, &mut final_seen)?;
    if final_seen != expected_files {
        return Err(Failure::Model(
            "The saved bundle member set changed during verification.".into(),
        ));
    }
    if let Some(destination) = destination {
        write_manifest(destination, &bytes)?;
    }
    Ok(manifest)
}
fn open_object(root: &Path, object: &str) -> Result<(Dir, Dir, Dir), Failure> {
    require_supported()?;
    let revision = object_path(object)?;
    let root_dir = files::open_root(root)?;
    let store = files::store(&root_dir, false)?;
    let objects = files::directory(&store, "objects", false)?;
    let name = format!("{revision}.bundle");
    files::reject_link(&objects.symlink_metadata(&name)?)?;
    let bundle = objects.open_dir_nofollow(&name)?;
    Ok((root_dir, objects, bundle))
}
fn require_supported() -> Result<(), Failure> {
    if !SUPPORTED {
        return Err(Failure::usage(
            "Exclusive native bundle publication is unavailable on this platform.",
        ));
    }
    Ok(())
}
pub(crate) fn read_manifest(
    root: &Path,
    object: &str,
    digest: &str,
    total: u64,
) -> Result<Manifest, Failure> {
    let (_, _, bundle) = open_object(root, object)?;
    Ok(load_manifest(&bundle, Some((digest, total)))?.0)
}
pub(crate) fn check(
    root: &Path,
    object: &str,
    digest: &str,
    total: u64,
) -> Result<Manifest, Failure> {
    let (_, _, bundle) = open_object(root, object)?;
    verify_directory(&bundle, Some((digest, total)), None)
}
pub(crate) fn inspect_retained(
    root: &Path,
    object: &str,
) -> Result<Option<(String, u64)>, Failure> {
    let result = (|| {
        let (_, _, bundle) = open_object(root, object)?;
        let (_, bytes, _) = load_manifest(&bundle, None)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let parsed = load_manifest(&bundle, None)?.0;
        let total = parsed.manifest_bytes
            + parsed
                .members
                .iter()
                .map(|member| member.bytes)
                .sum::<u64>();
        verify_directory(&bundle, Some((&digest, total)), None)?;
        Ok((digest, total))
    })();
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
pub(crate) fn restore(
    root: &Path,
    object: &str,
    digest: &str,
    total: u64,
    to: &str,
) -> Result<(), Failure> {
    files::source_path(to)?;
    let (root_dir, _, bundle) = open_object(root, object)?;
    let (parent, leaf) = controlled_destination(&root_dir, to)?;
    match parent.symlink_metadata(&leaf) {
        Ok(_) => {
            return Err(Failure::usage(
                "The bundle destination is already occupied.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut preparation = Preparation::new(&parent)?;
    verify_directory(&bundle, Some((digest, total)), Some(&preparation.dir))?;
    verify_directory(&preparation.dir, Some((digest, total)), None)?;
    sync_tree(&preparation.dir)?;
    recheck_visible_destination(&root_dir, to, &parent)?;
    preparation.publish(&parent, &leaf)
}

fn recheck_visible_destination(
    root: &Dir,
    path: &str,
    original_parent: &Dir,
) -> Result<(), Failure> {
    let (visible_parent, _) = controlled_destination(root, path)?;
    super::native::same_directory(original_parent, &visible_parent)
}

fn controlled_destination(root: &Dir, path: &str) -> Result<(Dir, String), Failure> {
    super::native::require_controlled_parent(root)?;
    let mut parent = root.try_clone()?;
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            return Ok((parent, component.into()));
        }
        files::reject_link(&parent.symlink_metadata(component)?)?;
        parent = parent.open_dir_nofollow(component)?;
        super::native::require_controlled_parent(&parent)?;
    }
    Err(Failure::usage("Invalid bundle destination."))
}
pub(crate) fn preview(
    root: &Path,
    object: &str,
    digest: &str,
    total: u64,
) -> Result<(files::Download, PreviewResources), Failure> {
    let (_, objects, bundle) = open_object(root, object)?;
    let preparation = Preparation::new(&objects)?;
    files::check_git(root, object, &preparation.name)?;
    let manifest = verify_directory(&bundle, Some((digest, total)), Some(&preparation.dir))?;
    verify_directory(&preparation.dir, Some((digest, total)), None)?;
    let mut members = BTreeMap::new();
    for member in &manifest.members {
        let file = files::regular(&preparation.dir, &member.path)?;
        members.insert(
            member.path.clone(),
            PreviewMember {
                media_type: member.media_type.clone(),
                reader: files::Download::from_file(file, member.bytes),
            },
        );
    }
    let entry = members
        .get(&manifest.entrypoint)
        .ok_or_else(|| Failure::usage("Missing bundle entrypoint."))?
        .reader
        .try_clone()?;
    // Every member handle is retained before private preparation cleanup.
    // Store deletion or corruption cannot expand preview authority afterwards.
    drop(preparation);
    Ok((
        entry,
        PreviewResources {
            entrypoint: manifest.entrypoint,
            members,
        },
    ))
}
pub(crate) fn source_check(root: &Path, source_root_path: &str, manifest: &Manifest) -> Value {
    let result = (|| {
        let root = files::open_root(root)?;
        let source = source_root(&root, source_root_path)?;
        let mut sources = Vec::new();
        for member in &manifest.members {
            sources.push(Held::open(&source, &member.path)?);
        }
        let mut matches = true;
        for (held, member) in sources.iter_mut().zip(&manifest.members) {
            if let Some(database) = manifest
                .database
                .as_ref()
                .filter(|database| database.path == member.path)
            {
                let receipt = copy_binary_limit(&held.file, io::sink(), MAX_BYTES)?;
                matches &= receipt.digest == database.source_sha256
                    && receipt.size == database.source_bytes;
            } else {
                let receipt = guard::copy_checked_limit(&mut held.file, io::sink(), MAX_BYTES)?;
                matches &= receipt.digest == member.sha256 && receipt.size == member.bytes;
            }
        }
        recheck_all(&source, &sources)?;
        recheck_selected_root(&root, &source, source_root_path, &sources)?;
        Ok(matches)
    })();
    match result {
        Ok(true) => json!({"status":"matches"}),
        Ok(false) => json!({"status":"changed"}),
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            json!({"status":"missing"})
        }
        Err(_) => json!({"status":"read_failed"}),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn visible_restore_parent_must_match_the_retained_parent_identity() {
        let root = Scratch::new();
        fs::create_dir(root.0.join("visible")).unwrap();
        fs::create_dir(root.0.join("previous")).unwrap();
        let project = files::open_root(&root.0).unwrap();
        let (visible, _) = controlled_destination(&project, "visible/restored").unwrap();
        let (previous, _) = controlled_destination(&project, "previous/restored").unwrap();
        assert!(recheck_visible_destination(&project, "visible/restored", &visible).is_ok());
        assert!(recheck_visible_destination(&project, "visible/restored", &previous).is_err());
    }

    #[test]
    fn binary_copy_starts_at_zero_even_when_a_cloned_handle_moved_the_cursor() {
        use std::io::{Seek, SeekFrom};
        let root = Scratch::new();
        let bytes = b"Synthetic binary\0delivery bytes";
        fs::write(root.0.join("data.sqlite"), bytes).unwrap();
        let directory = files::open_root(&root.0).unwrap();
        let source = files::regular(&directory, "data.sqlite").unwrap();
        let mut clone = source.try_clone().unwrap();
        clone.seek(SeekFrom::End(0)).unwrap();
        let mut output = Vec::new();
        let receipt = copy_binary_limit(&source, &mut output, bytes.len() as u64).unwrap();
        assert_eq!(output, bytes);
        assert_eq!(receipt.size, bytes.len() as u64);
        assert_eq!(receipt.digest, format!("{:x}", Sha256::digest(bytes)));
        assert!(copy_binary_limit(&source, io::sink(), bytes.len() as u64 - 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn auxiliary_checks_follow_the_original_held_source_parent_after_a_rename() {
        let root = Scratch::new();
        fs::create_dir(root.0.join("app")).unwrap();
        fs::write(root.0.join("app/data.sqlite"), b"Synthetic held source").unwrap();
        let project = files::open_root(&root.0).unwrap();
        let source = Held::open(&project, "app/data.sqlite").unwrap();
        fs::rename(root.0.join("app"), root.0.join("original")).unwrap();
        fs::create_dir(root.0.join("app")).unwrap();
        fs::write(root.0.join("original/data.sqlite-wal"), b"").unwrap();
        assert!(check_auxiliary(&source.parent, &source.name).is_err());
        let (visible, name) = member_parent(&project, "app/data.sqlite").unwrap();
        assert!(check_auxiliary(&visible, &name).is_ok());
        assert!(source.recheck(&project).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn moved_visible_restore_parent_rejects_publication_and_cleans_only_its_preparation() {
        let root = Scratch::new();
        fs::create_dir(root.0.join("visible")).unwrap();
        let project = files::open_root(&root.0).unwrap();
        let (original, _) = controlled_destination(&project, "visible/restored").unwrap();
        let stage = Preparation::new(&original).unwrap();
        stage
            .dir
            .write("index.html", b"Prepared complete delivery.")
            .unwrap();
        fs::rename(root.0.join("visible"), root.0.join("previous")).unwrap();
        fs::create_dir(root.0.join("visible")).unwrap();
        assert!(recheck_visible_destination(&project, "visible/restored", &original).is_err());
        let temporary = stage.name.clone();
        drop(stage);
        assert!(!root.0.join("previous").join(temporary).exists());
        assert!(!root.0.join("previous/restored").exists());
        assert!(!root.0.join("visible/restored").exists());
        assert!(fs::read_dir(root.0.join("visible"))
            .unwrap()
            .next()
            .is_none());
    }

    fn representative_bytes(length: usize) -> Vec<u8> {
        let pattern = b"/* Selected static delivery */\n";
        pattern.iter().copied().cycle().take(length).collect()
    }
    #[test]
    fn directory_prefixes_require_one_exact_spelling_without_case_aliases() {
        let mut selection = BundleSelection {
            root: ".".into(),
            entrypoint: "index.html".into(),
            members: vec![
                "index.html".into(),
                "Assets/a.css".into(),
                "assets/b.css".into(),
            ],
            requirements: vec![],
            database: None,
        };
        assert!(validate_selection(&selection).is_err());
        selection.members[2] = "Assets/b.css".into();
        assert!(validate_selection(&selection).is_ok());
        selection.entrypoint = "Assets/index.HTML".into();
        selection.members = vec!["Assets/index.HTML".into(), "assets/style.CSS".into()];
        assert!(validate_selection(&selection).is_err());
        selection.members[1] = "Assets/style.CSS".into();
        assert!(validate_selection(&selection).is_ok());
        let mut manifest = Manifest {
            schema_version: 1,
            delivery_type: "bundle".into(),
            entrypoint: "Assets/index.HTML".into(),
            requirements: vec![],
            database: None,
            manifest_bytes: 0,
            members: vec![
                Member {
                    path: "Assets/index.HTML".into(),
                    bytes: 0,
                    sha256: "0".repeat(64),
                    media_type: "text/html".into(),
                },
                Member {
                    path: "assets/style.CSS".into(),
                    bytes: 0,
                    sha256: "0".repeat(64),
                    media_type: "text/css".into(),
                },
            ],
        };
        assert!(validate_manifest(&manifest).is_err());
        manifest.members[1].path = "Assets/style.CSS".into();
        assert!(validate_manifest(&manifest).is_ok());
    }
    #[test]
    fn deep_portable_directory_validation_stays_within_the_auxiliary_budget() {
        let mut members = Vec::new();
        for index in 0..64 {
            let first = format!("root-{index}");
            let path = format!("{first}/{}/index.html", vec!["a"; 110].join("/"));
            assert!(path.len() <= 240);
            members.push(path);
        }
        let selection = BundleSelection {
            root: ".".into(),
            entrypoint: members[0].clone(),
            members,
            requirements: vec![String::new(); 10_000],
            database: None,
        };
        let (result, peak) = crate::test_alloc::measure(|| validate_selection(&selection));
        result.unwrap();
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "auxiliary heap {peak} bytes"
        );
    }
    #[test]
    fn nested_preparation_members_sync_without_reopening_the_pinned_root() {
        let root = Scratch::new();
        let parent = files::open_root(&root.0).unwrap();
        let stage = Preparation::new(&parent).unwrap();
        let mut first = new_member(&stage.dir, "assets/style.css").unwrap();
        first.write_all(b"body { color: navy; }").unwrap();
        first.sync_all().unwrap();
        drop(first);
        let mut second = new_member(&stage.dir, "assets/scripts/main.js").unwrap();
        second.write_all(b"void 0;").unwrap();
        second.sync_all().unwrap();
        drop(second);
        sync_tree(&stage.dir).unwrap();
        assert_eq!(
            files::regular(&stage.dir, "assets/style.css")
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            21
        );
        let temporary = stage.name.clone();
        drop(stage);
        assert!(!root.0.join(temporary).exists());
    }
    #[test]
    fn many_small_requirements_stay_within_the_auxiliary_budget_without_changing_manifest_bytes() {
        let (root, mut selection) = fixture();
        selection.requirements = vec![String::new(); 20_000];
        let selector = serde_json::to_vec(&selection).unwrap();
        assert!(selector.len() <= MAX_MANIFEST_BYTES as usize);
        fs::write(root.0.join("selection.json"), selector).unwrap();
        let revision = crate::id::ulid();
        let (result, peak) = crate::test_alloc::measure(|| {
            let selected = load_selection(&root.0, "selection.json")?;
            capture(&root.0, &selected, &revision)
        });
        let saved = result.unwrap();
        let manifest = check(
            &root.0,
            &saved.retained.object,
            &saved.retained.digest,
            saved.retained.size,
        )
        .unwrap();
        assert_eq!(manifest.requirements.len(), 20_000);
        let expected = serde_json::to_vec(&manifest).unwrap();
        let actual = fs::read(
            root.0
                .join(".vivac-artifacts")
                .join(&saved.retained.object)
                .join(MANIFEST),
        )
        .unwrap();
        assert_eq!(actual, expected);
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "auxiliary heap {peak} bytes"
        );
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("vivac-bundle-{}", crate::id::ulid()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Scratch, BundleSelection) {
        let root = Scratch::new();
        fs::create_dir(root.0.join("app")).unwrap();
        fs::create_dir(root.0.join("app/assets")).unwrap();
        fs::write(
            root.0.join("app/index.html"),
            b"<!doctype html><p>Original delivery</p>",
        )
        .unwrap();
        fs::write(
            root.0.join("app/assets/style.css"),
            b"body { color: navy; }",
        )
        .unwrap();
        let selection = BundleSelection {
            root: "app".into(),
            entrypoint: "index.html".into(),
            members: vec!["index.html".into(), "assets/style.css".into()],
            requirements: vec![],
            database: None,
        };
        (root, selection)
    }
    #[test]
    fn complete_revision_restores_without_source_and_rejects_extra_or_damaged_members() {
        let (root, selection) = fixture();
        let saved = capture(&root.0, &selection, &crate::id::ulid()).unwrap();
        let retained = &saved.retained;
        let manifest = check(&root.0, &retained.object, &retained.digest, retained.size).unwrap();
        assert_eq!(manifest.manifest_bytes, saved.manifest_bytes);
        fs::remove_dir_all(root.0.join("app")).unwrap();
        restore(
            &root.0,
            &retained.object,
            &retained.digest,
            retained.size,
            "restored",
        )
        .unwrap();
        assert_eq!(
            fs::read(root.0.join("restored/assets/style.css")).unwrap(),
            b"body { color: navy; }"
        );
        assert!(root.0.join("restored/.vivac-delivery.json").is_file());
        assert!(restore(
            &root.0,
            &retained.object,
            &retained.digest,
            retained.size,
            "restored"
        )
        .is_err());
        let object = root.0.join(".vivac-artifacts").join(&retained.object);
        fs::write(object.join("extra.js"), b"void 0;").unwrap();
        assert!(check(&root.0, &retained.object, &retained.digest, retained.size).is_err());
        assert!(restore(
            &root.0,
            &retained.object,
            &retained.digest,
            retained.size,
            "unexpected"
        )
        .is_err());
        assert!(!root.0.join("unexpected").exists());
        fs::remove_file(object.join("extra.js")).unwrap();
        fs::write(object.join("assets/style.css"), b"body { color: gold; }").unwrap();
        assert!(check(&root.0, &retained.object, &retained.digest, retained.size).is_err());
        fs::write(object.join(MANIFEST), b"{}").unwrap();
        assert!(read_manifest(&root.0, &retained.object, &retained.digest, retained.size).is_err());
    }
    #[test]
    fn preview_holds_verified_bytes_after_store_deletion() {
        let (root, selection) = fixture();
        let saved = capture(&root.0, &selection, &crate::id::ulid()).unwrap();
        let retained = &saved.retained;
        let (mut entry, mut resources) =
            preview(&root.0, &retained.object, &retained.digest, retained.size).unwrap();
        fs::remove_dir_all(root.0.join(".vivac-artifacts")).unwrap();
        let mut html = String::new();
        entry.read_to_string(&mut html).unwrap();
        assert!(html.contains("Original delivery"));
        let mut css = String::new();
        resources
            .members
            .get_mut("assets/style.css")
            .unwrap()
            .reader
            .read_to_string(&mut css)
            .unwrap();
        assert_eq!(css, "body { color: navy; }");
    }
    #[test]
    fn all_retained_source_handles_detect_earlier_source_changes() {
        let (root, selection) = fixture();
        let root_dir = files::open_root(&root.0).unwrap();
        let source = source_root(&root_dir, &selection.root).unwrap();
        let earlier = Held::open(&source, "index.html").unwrap();
        let mut later = Held::open(&source, "assets/style.css").unwrap();
        guard::copy_checked(&mut later.file, io::sink()).unwrap();
        fs::write(
            root.0.join("app/index.html"),
            b"An observable changed delivery",
        )
        .unwrap();
        assert!(recheck_all(&source, &[earlier, later]).is_err());
    }
    #[test]
    fn replaced_source_root_is_observable_with_original_handles_retained() {
        let (root, selection) = fixture();
        let project = files::open_root(&root.0).unwrap();
        let original = source_root(&project, "app").unwrap();
        let held = vec![Held::open(&original, "index.html").unwrap()];
        let rename = fs::rename(root.0.join("app"), root.0.join("previous"));
        #[cfg(windows)]
        {
            assert_eq!(rename.unwrap_err().raw_os_error(), Some(32));
            assert!(recheck_all(&original, &held).is_ok());
            assert!(recheck_selected_root(&project, &original, &selection.root, &held).is_ok());
            let saved = capture(&root.0, &selection, &crate::id::ulid()).unwrap();
            restore(
                &root.0,
                &saved.retained.object,
                &saved.retained.digest,
                saved.retained.size,
                "restored",
            )
            .unwrap();
            assert_eq!(
                fs::read(root.0.join("restored/index.html")).unwrap(),
                b"<!doctype html><p>Original delivery</p>"
            );
        }
        #[cfg(not(windows))]
        {
            rename.unwrap();
            fs::create_dir(root.0.join("app")).unwrap();
            fs::write(
                root.0.join("app/index.html"),
                b"<!doctype html><p>Replacement delivery</p>",
            )
            .unwrap();
            assert!(recheck_all(&original, &held).is_ok());
            assert!(recheck_selected_root(&project, &original, &selection.root, &held).is_err());
        }
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure complete bundle retention."]
    fn retained_bundle_release_benchmark() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        for (label, target, count, budget) in [
            ("one_mib", 1024 * 1024, 1, 500.0),
            ("ten_mib", 10 * 1024 * 1024, 1, 2000.0),
            ("many_small", 1024 * 1024, 64, 500.0),
        ] {
            let root = Scratch::new();
            let mut selection = BundleSelection {
                root: ".".into(),
                entrypoint: "index.html".into(),
                members: vec!["index.html".into()],
                requirements: vec![],
                database: None,
            };
            for index in 1..count {
                selection.members.push(format!("style-{index}.css"));
            }
            selection.members.sort();
            let mut payload = target - 1024;
            for _ in 0..4 {
                let members = selection
                    .members
                    .iter()
                    .enumerate()
                    .map(|(index, path)| Member {
                        path: path.clone(),
                        bytes: (payload / count + usize::from(index < payload % count)) as u64,
                        sha256: "0".repeat(64),
                        media_type: media_type(path).unwrap().into(),
                    })
                    .collect();
                let manifest = Manifest {
                    schema_version: 1,
                    delivery_type: "bundle".into(),
                    entrypoint: selection.entrypoint.clone(),
                    requirements: vec![],
                    database: None,
                    members,
                    manifest_bytes: 0,
                };
                payload = target - manifest_bytes(&manifest).unwrap().len();
            }
            for (index, path) in selection.members.iter().enumerate() {
                fs::write(
                    root.0.join(path),
                    representative_bytes(payload / count + usize::from(index < payload % count)),
                )
                .unwrap();
            }
            let mut samples = Vec::with_capacity(100);
            let mut maximum_heap = 0;
            let mut measured_total = 0;
            for _ in 0..100 {
                let revision = crate::id::ulid();
                let started = std::time::Instant::now();
                let (result, heap) =
                    crate::test_alloc::measure(|| capture(&root.0, &selection, &revision));
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
                maximum_heap = maximum_heap.max(heap);
                let retained = result.unwrap().retained;
                measured_total = retained.size;
                assert_eq!(measured_total, target as u64);
                let project = files::open_root(&root.0).unwrap();
                let store = files::store(&project, false).unwrap();
                // Only the revision created by this measured invocation is
                // removed, through a no-follow capability directory handle.
                store.remove_dir_all(&retained.object).unwrap();
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}",
                json!({"benchmark":"bundle_retention","case":label,"samples":100,"profile":"release","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"filesystem_root":root.0,"total_bytes":measured_total,"members":count,"chunk_bytes":64*1024,"guards":"complete_utf8_sensitive_metadata_source_identity_git_native_exclusive","p50_ms":samples[49],"p99_ms":samples[98],"max_ms":samples[99],"peak_auxiliary_heap_bytes":maximum_heap,"stack_allowance_bytes":128*1024})
            );
            assert!(
                samples[98] < budget,
                "{label} p99 {} ms exceeds {budget} ms",
                samples[98]
            );
            assert!(
                maximum_heap + 128 * 1024 <= 2 * 1024 * 1024,
                "{label} auxiliary heap {maximum_heap}"
            );
        }
    }
    fn sqlite_benchmark_fixture(target: usize) -> (Scratch, BundleSelection, u64) {
        use libsqlite3_sys as ffi;
        use std::ffi::CString;
        let root = Scratch::new();
        let selection = BundleSelection {
            root: ".".into(),
            entrypoint: "index.html".into(),
            members: vec!["data.sqlite".into(), "index.html".into(), "main.js".into()],
            requirements: vec![],
            database: Some("data.sqlite".into()),
        };
        fs::write(
            root.0.join("index.html"),
            b"<!doctype html><script src=main.js></script>",
        )
        .unwrap();
        fs::write(
            root.0.join("main.js"),
            b"document.title = 'Synthetic SQLite application';",
        )
        .unwrap();
        {
            let _engine = super::super::sqlite::test_engine_guard().unwrap();
            let path = CString::new(root.0.join("data.sqlite").to_str().unwrap()).unwrap();
            let value = "Synthetic SQLite application fixture data. ".repeat(6);
            let rows = target * 9 / 10 / 280;
            assert!(rows <= 100_000 && value.len() <= 65536);
            unsafe {
                let mut database = std::ptr::null_mut();
                assert_eq!(
                    ffi::sqlite3_open(path.as_ptr(), &mut database),
                    ffi::SQLITE_OK
                );
                assert_eq!(ffi::sqlite3_exec(database, c"PRAGMA cache_size=-64; PRAGMA temp_store=MEMORY; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL); BEGIN".as_ptr(), None, std::ptr::null_mut(), std::ptr::null_mut()), ffi::SQLITE_OK);
                let mut statement = std::ptr::null_mut();
                assert_eq!(
                    ffi::sqlite3_prepare_v2(
                        database,
                        c"INSERT INTO items VALUES(?1,?2)".as_ptr(),
                        -1,
                        &mut statement,
                        std::ptr::null_mut()
                    ),
                    ffi::SQLITE_OK
                );
                for index in 0..rows {
                    assert_eq!(
                        ffi::sqlite3_bind_int64(statement, 1, index as i64),
                        ffi::SQLITE_OK
                    );
                    assert_eq!(
                        ffi::sqlite3_bind_text(
                            statement,
                            2,
                            value.as_ptr().cast(),
                            value.len() as i32,
                            None
                        ),
                        ffi::SQLITE_OK
                    );
                    assert_eq!(ffi::sqlite3_step(statement), ffi::SQLITE_DONE);
                    assert_eq!(ffi::sqlite3_reset(statement), ffi::SQLITE_OK);
                }
                assert_eq!(ffi::sqlite3_finalize(statement), ffi::SQLITE_OK);
                assert_eq!(
                    ffi::sqlite3_exec(
                        database,
                        c"COMMIT".as_ptr(),
                        None,
                        std::ptr::null_mut(),
                        std::ptr::null_mut()
                    ),
                    ffi::SQLITE_OK
                );
                assert_eq!(ffi::sqlite3_close(database), ffi::SQLITE_OK);
            }
        }
        let project = files::open_root(&root.0).unwrap();
        let first = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        fs::copy(
            root.0
                .join(".vivac-artifacts")
                .join(&first.object)
                .join("data.sqlite"),
            root.0.join("data.sqlite"),
        )
        .unwrap();
        files::store(&project, false)
            .unwrap()
            .remove_dir_all(&first.object)
            .unwrap();
        let warmup = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let mut manifest =
            read_manifest(&root.0, &warmup.object, &warmup.digest, warmup.size).unwrap();
        let other = manifest
            .members
            .iter()
            .filter(|member| member.path != "index.html")
            .map(|member| member.bytes)
            .sum::<u64>() as usize;
        let mut html_bytes = 0;
        for _ in 0..4 {
            html_bytes = target
                .checked_sub(other + manifest_bytes(&manifest).unwrap().len())
                .unwrap();
            manifest
                .members
                .iter_mut()
                .find(|member| member.path == "index.html")
                .unwrap()
                .bytes = html_bytes as u64;
        }
        let mut html = b"<!doctype html><script src=main.js></script>".to_vec();
        html.extend(representative_bytes(html_bytes - html.len()));
        fs::write(root.0.join("index.html"), html).unwrap();
        files::store(&project, false)
            .unwrap()
            .remove_dir_all(&warmup.object)
            .unwrap();
        let ready = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        assert_eq!(ready.size, target as u64);
        let database_bytes = read_manifest(&root.0, &ready.object, &ready.digest, ready.size)
            .unwrap()
            .members
            .iter()
            .find(|member| member.path == "data.sqlite")
            .unwrap()
            .bytes;
        files::store(&project, false)
            .unwrap()
            .remove_dir_all(&ready.object)
            .unwrap();
        (root, selection, database_bytes)
    }

    #[test]
    #[ignore = "Run explicitly in release mode with --test-threads=1 to measure SQLite application retention."]
    fn retained_sqlite_application_release_benchmark() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        for (label, target, budget) in [
            ("one_mib", 1024 * 1024, 500.0),
            ("ten_mib", 10 * 1024 * 1024, 2000.0),
        ] {
            let (root, selection, database_bytes) = sqlite_benchmark_fixture(target);
            let mut samples = Vec::with_capacity(100);
            let mut maximum_rust = 0;
            let mut maximum_sqlite = 0;
            for _ in 0..100 {
                {
                    let _engine = super::super::sqlite::test_engine_guard().unwrap();
                    unsafe {
                        libsqlite3_sys::sqlite3_memory_highwater(1);
                    }
                }
                let revision = crate::id::ulid();
                let started = std::time::Instant::now();
                let (result, rust_heap) =
                    crate::test_alloc::measure(|| capture(&root.0, &selection, &revision));
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
                maximum_rust = maximum_rust.max(rust_heap);
                {
                    let _engine = super::super::sqlite::test_engine_guard().unwrap();
                    let sqlite_heap = unsafe { libsqlite3_sys::sqlite3_memory_highwater(0) };
                    maximum_sqlite = maximum_sqlite.max(usize::try_from(sqlite_heap).unwrap());
                }
                let retained = result.unwrap().retained;
                assert_eq!(retained.size, target as u64);
                let project = files::open_root(&root.0).unwrap();
                files::store(&project, false)
                    .unwrap()
                    .remove_dir_all(&retained.object)
                    .unwrap();
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}",
                json!({"benchmark":"sqlite_application_retention","case":label,"samples":100,"profile":"release","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"filesystem_root":root.0,"total_bytes":target,"database_bytes":database_bytes,"other_bytes":target as u64-database_bytes,"transformation":SQLITE_TRANSFORMATION,"members":3,"guards":"complete_logical_sqlite_utf8_sensitive_metadata_source_lock_identity_git_native_exclusive","p50_ms":samples[49],"p99_ms":samples[98],"max_ms":samples[99],"peak_rust_auxiliary_heap_bytes":maximum_rust,"peak_sqlite_heap_bytes":maximum_sqlite,"combined_conservative_heap_bytes":maximum_rust+maximum_sqlite,"stack_allowance_bytes":128*1024})
            );
            assert!(
                samples[98] < budget,
                "{label} p99 {} ms exceeds {budget} ms",
                samples[98]
            );
            assert!(
                maximum_rust + maximum_sqlite + 128 * 1024 <= 2 * 1024 * 1024,
                "{label} combined auxiliary memory exceeds 2 MiB"
            );
        }
    }

    #[test]
    fn selectors_are_bounded_exact_and_reject_unknown_fields() {
        let (root, selection) = fixture();
        fs::write(
            root.0.join("selection.json"),
            serde_json::to_vec(&selection).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_selection(&root.0, "selection.json")
                .unwrap()
                .members
                .len(),
            2
        );
        fs::write(
            root.0.join("selection.json"),
            br#"{"root":"app","entrypoint":"index.html","members":["index.html"],"extra":true}"#,
        )
        .unwrap();
        assert!(load_selection(&root.0, "selection.json").is_err());
        fs::write(
            root.0.join("selection.json"),
            vec![b' '; MAX_MANIFEST_BYTES as usize + 1],
        )
        .unwrap();
        assert!(load_selection(&root.0, "selection.json").is_err());
    }
    #[test]
    fn complete_bundle_limit_counts_manifest_and_rejects_sensitive_bodies() {
        let (root, mut selection) = fixture();
        selection.members = vec!["index.html".into()];
        fs::write(
            root.0.join("app/index.html"),
            vec![b' '; MAX_BYTES as usize],
        )
        .unwrap();
        assert!(capture(&root.0, &selection, &crate::id::ulid()).is_err());
        let objects = root.0.join(".vivac-artifacts/objects");
        assert!(fs::read_dir(&objects).unwrap().next().is_none());
        fs::write(root.0.join("app/index.html"), b"<p>person@example.com</p>").unwrap();
        assert!(capture(&root.0, &selection, &crate::id::ulid()).is_err());
        assert!(fs::read_dir(objects).unwrap().next().is_none());
    }
    #[test]
    fn maximum_bundle_stays_within_auxiliary_memory_budget() {
        let root = Scratch::new();
        let mut selection = BundleSelection {
            root: ".".into(),
            entrypoint: "index.html".into(),
            members: vec!["index.html".into()],
            requirements: vec![],
            database: None,
        };
        let bytes = vec![b' '; (MAX_BYTES - MAX_MANIFEST_BYTES) as usize];
        fs::write(root.0.join("index.html"), &bytes).unwrap();
        for index in 1..64 {
            let name = format!("style-{index}.css");
            fs::write(root.0.join(&name), b"body { color: navy; }").unwrap();
            selection.members.push(name);
        }
        let revision = crate::id::ulid();
        let (result, peak) = crate::test_alloc::measure(|| capture(&root.0, &selection, &revision));
        let saved = result.unwrap();
        assert_eq!(saved.members, 64);
        assert!(saved.retained.size <= MAX_BYTES);
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "auxiliary heap {peak} bytes"
        );
    }

    #[test]
    fn portable_paths_reject_ambiguity_and_reserved_metadata() {
        for path in [
            "../index.html",
            "AUX.html",
            "a\\b.html",
            "a./b.html",
            ".vivac-delivery.json",
            "a:b.html",
            "a//b.html",
            "a//b.css",
            "a/./b.css",
        ] {
            assert!(member_path(path).is_err(), "accepted {path}");
        }
        assert!(member_path("assets/style.css").is_ok());
        let selection = BundleSelection {
            root: "delivery".into(),
            entrypoint: "index.html".into(),
            members: vec!["index.html".into(), "INDEX.html".into()],
            requirements: vec![],
            database: None,
        };
        assert!(validate_selection(&selection).is_err());
    }
}
