use super::{files, guard, native::Preparation, runtime};
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
pub(crate) const WAL_PROFILE: &str = "readonly-wal-v1";
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_selection_identity"
    )]
    pub(crate) database: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_database_source"
    )]
    pub(crate) database_source: Option<DatabaseSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_selection_identity"
    )]
    pub(crate) runtime: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_media_review"
    )]
    pub(crate) media_review: Option<MediaReview>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MediaReview {
    // Caller attestation of prior human review, not authentication or proof of safety.
    pub(crate) profile: String,
    pub(crate) members: Vec<ReviewedMember>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseSource {
    pub(crate) profile: String,
    pub(crate) wal: String,
    pub(crate) shm: String,
}
fn deserialize_database_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DatabaseSource>, D::Error> {
    DatabaseSource::deserialize(deserializer).map(Some)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewedMember {
    pub(crate) path: String,
    pub(crate) sha256: String,
}
fn deserialize_media_review<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MediaReview>, D::Error> {
    MediaReview::deserialize(deserializer).map(Some)
}
fn deserialize_selection_identity<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_runtime"
    )]
    pub(super) runtime: Option<runtime::Descriptor>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_media_review"
    )]
    pub(crate) media_review: Option<MediaReview>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_retained_source"
    )]
    pub(crate) source: Option<RetainedDatabaseSource>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedDatabaseSource {
    pub(crate) profile: String,
    pub(crate) wal: RetainedWal,
    pub(crate) shm: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedWal {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}
fn deserialize_retained_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RetainedDatabaseSource>, D::Error> {
    RetainedDatabaseSource::deserialize(deserializer).map(Some)
}
fn deserialize_database<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DatabaseMetadata>, D::Error> {
    DatabaseMetadata::deserialize(deserializer).map(Some)
}
fn deserialize_runtime<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<runtime::Descriptor>, D::Error> {
    runtime::Descriptor::deserialize(deserializer).map(Some)
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
    pub(super) runtime: Option<runtime::Descriptor>,
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
    } else if extension.eq_ignore_ascii_case("png") {
        Ok("image/png")
    } else if extension.eq_ignore_ascii_case("mp4") {
        Ok("video/mp4")
    } else if ["sqlite", "sqlite3", "db"]
        .iter()
        .any(|value| extension.eq_ignore_ascii_case(value))
    {
        Ok(SQLITE_MEDIA_TYPE)
    } else {
        Err(Failure::usage(
            "Bundle members must be HTML, CSS, JavaScript, PNG, or MP4 files.",
        ))
    }
}
fn is_media(media_type: &str) -> bool {
    matches!(media_type, "image/png" | "video/mp4")
}
fn validate_media_review(selection: &BundleSelection) -> Result<(), Failure> {
    let media: BTreeSet<_> = selection
        .members
        .iter()
        .filter(|path| media_type(path).is_ok_and(is_media))
        .map(String::as_str)
        .collect();
    let Some(review) = &selection.media_review else {
        return if media.is_empty() {
            Ok(())
        } else {
            Err(Failure::usage(
                "Media members require prior human review of their exact bytes.",
            ))
        };
    };
    if media.is_empty()
        || selection.database.is_some()
        || selection.runtime.is_some()
        || review.profile != "reviewed-media-v1"
        || review.members.len() != media.len()
    {
        return Err(Failure::usage(
            "Invalid media review selection or mixed application bundle.",
        ));
    }
    let mut reviewed = BTreeSet::new();
    for member in &review.members {
        member_path(&member.path)?;
        if !valid_digest(&member.sha256)
            || !media.contains(member.path.as_str())
            || !reviewed.insert(&member.path)
        {
            return Err(Failure::usage(
                "Media review must exactly name each media member once with its lowercase SHA-256.",
            ));
        }
    }
    Ok(())
}
fn match_media_review(
    review: Option<&MediaReview>,
    path: &str,
    digest: &str,
) -> Result<(), Failure> {
    if !review.is_some_and(|review| {
        review
            .members
            .iter()
            .any(|member| member.path == path && member.sha256 == digest)
    }) {
        return Err(Failure::usage(
            "Media bytes do not match the declared prior human review.",
        ));
    }
    Ok(())
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
    validate_selection_shape(selection)?;
    if let Some(id) = &selection.runtime {
        runtime::descriptor(id)?;
    }
    Ok(())
}
fn validate_selection_shape(selection: &BundleSelection) -> Result<(), Failure> {
    if selection.root != "." {
        files::source_path(&selection.root)?;
    }
    if !matches!(
        (&selection.database, &selection.runtime),
        (None, None) | (Some(_), Some(_))
    ) || selection.database.as_ref().is_some_and(String::is_empty)
        || selection.runtime.as_ref().is_some_and(String::is_empty)
    {
        return Err(Failure::usage(
            "SQLite applications require paired database and product runtime identities.",
        ));
    }
    if selection.members.is_empty()
        || selection.members.len() + usize::from(selection.runtime.is_some()) > MAX_MEMBERS
    {
        return Err(Failure::usage(
            "Bundles must select between 1 and 64 members.",
        ));
    }
    if let Some(source) = &selection.database_source {
        let database = selection.database.as_deref().ok_or_else(|| {
            Failure::usage("A database source requires a selected database and product runtime.")
        })?;
        validate_database_source(database, source)?;
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
        if selection.runtime.is_some()
            && path
                .split('/')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case(runtime::PATH))
        {
            return Err(Failure::usage(
                "The product runtime member path is reserved.",
            ));
        }
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
    validate_media_review(selection)?;
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
fn validate_database_source(database: &str, source: &DatabaseSource) -> Result<(), Failure> {
    validate_database_path(database)?;
    if source.profile != WAL_PROFILE
        || source.wal != format!("{database}-wal")
        || source.shm != format!("{database}-shm")
    {
        return Err(Failure::usage(
            "The WAL source must exactly select the database's WAL and SHM paths.",
        ));
    }
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
    let selection = read_selection(root, selector)?;
    validate_selection(&selection)?;
    Ok(selection)
}
fn read_selection(root: &Path, selector: &str) -> Result<BundleSelection, Failure> {
    files::source_path(selector)?;
    let root = files::open_root(root)?;
    let mut source = Held::open(&root, selector)?;
    let bytes = bounded_read(&mut source.file, MAX_MANIFEST_BYTES)?;
    source.recheck(&root)?;
    guard::copy_checked(std::io::Cursor::new(&bytes), io::sink())?;
    let mut selection: BundleSelection = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::usage("Invalid bundle selector JSON."))?;
    validate_selection_shape(&selection)?;
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
    #[cfg(windows)]
    prepared_runtime: bool,
}
impl Held {
    fn from_prepared_runtime(root: &Dir, file: File) -> Result<Self, Failure> {
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(Failure::usage("Artifacts must be regular files."));
        }
        files::reject_link(&before)?;
        #[cfg(windows)]
        let identity = files::file_identity(&file)?;
        Ok(Self {
            path: runtime::PATH.into(),
            parent: root.try_clone()?,
            name: runtime::PATH.into(),
            file,
            before,
            #[cfg(windows)]
            identity,
            #[cfg(windows)]
            prepared_runtime: true,
        })
    }
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
            #[cfg(windows)]
            prepared_runtime: false,
        })
    }
    fn check_wal_identity(&self) -> Result<(), Failure> {
        let metadata = self.file.metadata()?;
        files::reject_link(&metadata)?;
        files::require_single_link(&self.file)?;
        files::unchanged(&self.before, &metadata)?;
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            let named = stat_name(&self.parent, &self.name)?.ok_or_else(|| {
                Failure::usage("A SQLite source name disappeared during capture.")
            })?;
            if named.st_mode & libc::S_IFMT != libc::S_IFREG
                || named.st_nlink != 1
                || named.st_dev as i128 != i128::from(metadata.dev())
                || named.st_ino as u128 != u128::from(metadata.ino())
            {
                return Err(Failure::usage(
                    "A SQLite source identity changed during capture.",
                ));
            }
        }
        #[cfg(windows)]
        {
            use cap_std::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, SYNCHRONIZE};
            let mut options = OpenOptions::new();
            options
                .read(true)
                .follow(FollowSymlinks::No)
                .access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE);
            let named = self.parent.open_with(&self.name, &options)?;
            files::reject_link(&named.metadata()?)?;
            files::require_single_link(&named)?;
            files::same_source(&self.file, &named)?;
        }
        Ok(())
    }
    fn recheck(&self, root: &Dir) -> Result<(), Failure> {
        files::unchanged(&self.before, &self.file.metadata()?)?;
        #[cfg(windows)]
        let current = if self.prepared_runtime {
            // The prepared runtime's original handle supplies every byte check.
            // Reopening its name needs attributes and identity only; this handle
            // does not pin the name.
            open_prepared_runtime_attributes(root)?
        } else {
            files::regular(root, &self.path)?
        };
        #[cfg(not(windows))]
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
#[cfg(windows)]
fn open_prepared_runtime_attributes(root: &Dir) -> Result<File, Failure> {
    use cap_std::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, SYNCHRONIZE};

    let (parent, name) = member_parent(root, runtime::PATH)?;
    files::reject_link(&parent.symlink_metadata(&name)?)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE);
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Failure::usage("Artifacts must be regular files."));
    }
    files::reject_link(&metadata)?;
    Ok(file)
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

#[cfg(unix)]
fn stat_name(parent: &Dir, name: &str) -> Result<Option<libc::stat>, Failure> {
    use std::os::fd::AsRawFd;
    let name =
        std::ffi::CString::new(name).map_err(|_| Failure::usage("Invalid SQLite source name."))?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the capability directory remains live and the name is terminated.
    // fstatat fills the output on success without opening another file descriptor.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        // SAFETY: a successful fstatat initialized the complete stat value.
        return Ok(Some(unsafe { metadata.assume_init() }));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(error.into())
    }
}
fn name_exists(parent: &Dir, name: &str) -> Result<bool, Failure> {
    #[cfg(unix)]
    return stat_name(parent, name).map(|metadata| metadata.is_some());
    #[cfg(not(unix))]
    match parent.symlink_metadata(name) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn check_wal_sources(database: &Held, wal: &Held, shm: &Held) -> Result<(), Failure> {
    super::native::same_directory(&database.parent, &wal.parent)?;
    super::native::same_directory(&database.parent, &shm.parent)?;
    for source in [database, wal, shm] {
        source.check_wal_identity()?;
    }
    if name_exists(&database.parent, &format!("{}-journal", database.name))? {
        return Err(Failure::usage(
            "SQLite rollback journals are not supported.",
        ));
    }
    Ok(())
}
fn check_auxiliary(parent: &Dir, name: &str) -> Result<(), Failure> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if name_exists(parent, &format!("{name}{suffix}"))? {
            return Err(Failure::usage("SQLite auxiliary files are not supported."));
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

struct PositionalReader<'a> {
    file: &'a File,
    offset: u64,
}
impl<'a> PositionalReader<'a> {
    fn new(file: &'a File) -> Self {
        Self { file, offset: 0 }
    }
}
impl Read for PositionalReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let count = cap_std::fs::FileExt::read_at(self.file, buffer, self.offset)?;
        #[cfg(windows)]
        let count = cap_std::fs::FileExt::seek_read(self.file, buffer, self.offset)?;
        #[cfg(not(any(unix, windows)))]
        let count = {
            use std::io::{Seek, SeekFrom};
            let mut file = self.file;
            file.seek(SeekFrom::Start(self.offset))?;
            file.read(buffer)?
        };
        self.offset = self
            .offset
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("Bundle size overflow."))?;
        Ok(count)
    }
}
fn copy_binary_limit(
    source: &File,
    mut writer: impl Write,
    limit: u64,
) -> Result<guard::Receipt, Failure> {
    let mut reader = PositionalReader::new(source);
    let mut buffer = [0; 64 * 1024];
    let mut digest = Sha256::new();
    let mut size = 0u64;
    loop {
        let read = reader.read(&mut buffer);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<&'a runtime::Descriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_review: Option<&'a MediaReview>,
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
    let runtime = selection
        .runtime
        .as_deref()
        .map(runtime::descriptor)
        .transpose()?;
    let mut expected_bytes = runtime.as_ref().map_or(0, |runtime| runtime.bytes);
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
    let mut auxiliary = Vec::new();
    if let Some(selected) = &selection.database_source {
        auxiliary.push(Held::open(&source_dir, &selected.wal)?);
        auxiliary.push(Held::open(&source_dir, &selected.shm)?);
        expected_bytes = expected_bytes
            .checked_add(auxiliary[0].before.len())
            .ok_or_else(|| Failure::usage("Bundle size overflow."))?;
        if expected_bytes > MAX_BYTES {
            return Err(Failure::usage("The complete source bundle exceeds 10 MiB."));
        }
        let database = sources
            .iter()
            .find(|source| selection.database.as_ref() == Some(&source.path))
            .ok_or_else(|| Failure::usage("Missing selected SQLite source."))?;
        check_wal_sources(database, &auxiliary[0], &auxiliary[1])?;
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
    let mut media_budget = super::media::Budget::default();
    let mut manifest = Manifest {
        schema_version: if selection.database_source.is_some() {
            5
        } else if selection.media_review.is_some() {
            4
        } else if selection.database.is_some() {
            3
        } else {
            1
        },
        delivery_type: if selection.database.is_some() {
            "sqlite_application"
        } else {
            "bundle"
        }
        .into(),
        entrypoint: selection.entrypoint.clone(),
        requirements: vec![],
        database: None,
        runtime,
        media_review: selection.media_review.clone(),
        members: Vec::with_capacity(sources.len()),
        manifest_bytes: 0,
    };
    for source in &mut sources {
        let mut output = new_member(&preparation.dir, &source.path)?;
        let receipt = if selection.database.as_ref() == Some(&source.path) {
            let (snapshot, retained_source) = if let Some(selected) = &selection.database_source {
                let captured = super::sqlite::normalize_wal(
                    &source.file,
                    &auxiliary[0].file,
                    &auxiliary[1].file,
                    &output,
                    || check_wal_sources(source, &auxiliary[0], &auxiliary[1]),
                )?;
                (
                    captured.snapshot,
                    Some(RetainedDatabaseSource {
                        profile: WAL_PROFILE.into(),
                        wal: RetainedWal {
                            path: selected.wal.clone(),
                            bytes: captured.wal_bytes,
                            sha256: captured.wal_sha256,
                        },
                        shm: selected.shm.clone(),
                    }),
                )
            } else {
                (
                    super::sqlite::normalize(&source.file, &output, || {
                        check_auxiliary(&source.parent, &source.name)
                    })?,
                    None,
                )
            };
            if snapshot.bytes > MAX_BYTES - total {
                return Err(Failure::usage("The complete bundle exceeds 10 MiB."));
            }
            manifest.database = Some(DatabaseMetadata {
                path: source.path.clone(),
                profile: SQLITE_PROFILE.into(),
                transformation: SQLITE_TRANSFORMATION.into(),
                source_bytes: snapshot.source_bytes,
                source_sha256: snapshot.source_sha256,
                source: retained_source,
            });
            guard::Receipt {
                digest: snapshot.sha256,
                size: snapshot.bytes,
            }
        } else if is_media(media_type(&source.path)?) {
            let receipt = super::media::copy_checked_with_budget(
                &mut source.file,
                &mut output,
                media_type(&source.path)?,
                MAX_BYTES - total,
                &mut media_budget,
            )?;
            match_media_review(
                selection.media_review.as_ref(),
                &source.path,
                &receipt.digest,
            )?;
            receipt
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
    let mut prepared_runtime = None;
    if let Some(runtime) = &manifest.runtime {
        if runtime.bytes > MAX_BYTES - total {
            return Err(Failure::usage("The complete bundle exceeds 10 MiB."));
        }
        let mut output = new_member(&preparation.dir, &runtime.path)?;
        let receipt = runtime::materialize(&runtime.id, &mut output)?;
        files::matching(
            &receipt.digest,
            receipt.size,
            &runtime.sha256,
            runtime.bytes,
        )?;
        output.sync_all()?;
        prepared_runtime = Some(output);
        total += receipt.size;
        manifest.members.push(Member {
            path: runtime.path.clone(),
            bytes: receipt.size,
            sha256: receipt.digest,
            media_type: "text/javascript".into(),
        });
        manifest
            .members
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
    recheck_all(&source_dir, &sources)?;
    recheck_all(&source_dir, &auxiliary)?;
    let bytes = manifest_bytes(&BorrowedManifest {
        schema_version: manifest.schema_version,
        delivery_type: &manifest.delivery_type,
        entrypoint: &manifest.entrypoint,
        requirements: &selection.requirements,
        members: &manifest.members,
        database: manifest.database.as_ref(),
        runtime: manifest.runtime.as_ref(),
        media_review: manifest.media_review.as_ref(),
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
    verify_directory_inner(
        &preparation.dir,
        Some((&digest, total)),
        None,
        prepared_runtime,
    )?;
    recheck_all(&source_dir, &sources)?;
    recheck_selected_root(&root_dir, &source_dir, &selection.root, &sources)?;
    recheck_all(&source_dir, &auxiliary)?;
    recheck_selected_root(&root_dir, &source_dir, &selection.root, &auxiliary)?;
    if !auxiliary.is_empty() {
        let database = sources
            .iter()
            .find(|source| selection.database.as_ref() == Some(&source.path))
            .ok_or_else(|| Failure::usage("Missing selected SQLite source."))?;
        check_wal_sources(database, &auxiliary[0], &auxiliary[1])?;
    }
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
        runtime: manifest.runtime,
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), Failure> {
    if manifest.schema_version == 2 && manifest.delivery_type == "sqlite_application" {
        return Err(Failure::usage(
            "Experimental SQLite application manifest schema 2 is not supported.",
        ));
    }
    if !matches!(
        (
            manifest.schema_version,
            manifest.delivery_type.as_str(),
            manifest.database.as_ref(),
            manifest.runtime.as_ref(),
            manifest.media_review.as_ref()
        ),
        (1, "bundle", None, None, None)
            | (3, "sqlite_application", Some(_), Some(_), None)
            | (5, "sqlite_application", Some(_), Some(_), None)
            | (4, "bundle", None, None, Some(_))
    ) {
        return Err(Failure::usage("Unsupported bundle manifest schema."));
    }
    if let Some(runtime) = &manifest.runtime {
        runtime::validate(runtime)?;
        let mut generated = manifest
            .members
            .iter()
            .filter(|member| member.path == runtime.path);
        let member = generated
            .next()
            .ok_or_else(|| Failure::usage("The product runtime member is missing."))?;
        if generated.next().is_some()
            || member.bytes != runtime.bytes
            || member.sha256 != runtime.sha256
            || member.media_type != "text/javascript"
        {
            return Err(Failure::usage(
                "Product runtime member metadata does not match the engine catalog.",
            ));
        }
    }
    let selection = BundleSelection {
        root: ".".into(),
        entrypoint: manifest.entrypoint.clone(),
        members: manifest
            .members
            .iter()
            .filter(|member| {
                manifest
                    .runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.path != member.path)
            })
            .map(|member| member.path.clone())
            .collect(),
        requirements: vec![],
        database: manifest
            .database
            .as_ref()
            .map(|database| database.path.clone()),
        database_source: manifest
            .database
            .as_ref()
            .and_then(|database| database.source.as_ref())
            .map(|source| DatabaseSource {
                profile: source.profile.clone(),
                wal: source.wal.path.clone(),
                shm: source.shm.clone(),
            }),
        runtime: manifest.runtime.as_ref().map(|runtime| runtime.id.clone()),
        media_review: manifest.media_review.clone(),
    };
    validate_selection_shape(&selection)?;
    if let Some(database) = &manifest.database {
        if database.profile != SQLITE_PROFILE
            || database.transformation != SQLITE_TRANSFORMATION
            || database.source_bytes > MAX_BYTES
            || database.source_bytes < 512
            || !valid_digest(&database.source_sha256)
        {
            return Err(Failure::usage("Invalid SQLite application metadata."));
        }
        if (manifest.schema_version == 5) != database.source.is_some() {
            return Err(Failure::usage(
                "SQLite source metadata does not match the manifest schema.",
            ));
        }
        if let Some(source) = &database.source {
            let source_total = database
                .source_bytes
                .checked_add(source.wal.bytes)
                .and_then(|bytes| {
                    manifest
                        .members
                        .iter()
                        .filter(|member| member.path != database.path)
                        .try_fold(bytes, |total, member| total.checked_add(member.bytes))
                });
            // An empty WAL is legitimate after TRUNCATE. A partial header is not.
            if (1..32).contains(&source.wal.bytes)
                || source_total.is_none_or(|bytes| bytes > MAX_BYTES)
                || !valid_digest(&source.wal.sha256)
            {
                return Err(Failure::usage("Invalid SQLite WAL source metadata."));
            }
        }
    }
    for requirement in &manifest.requirements {
        if let Some(finding) = crate::redact::check_field("bundle requirement", requirement) {
            return Err(Failure::Redaction(Box::new(finding)));
        }
    }
    let mut previous = None;
    for member in &manifest.members {
        if is_media(media_type(&member.path)?) {
            match_media_review(manifest.media_review.as_ref(), &member.path, &member.sha256)?;
        }
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
    verify_directory_inner(root, expected, destination, None)
}
fn verify_directory_inner(
    root: &Dir,
    expected: Option<(&str, u64)>,
    destination: Option<&Dir>,
    mut prepared_runtime: Option<File>,
) -> Result<Manifest, Failure> {
    let (manifest, bytes, manifest_source) = load_manifest(root, expected)?;
    if prepared_runtime.is_some() && manifest.runtime.is_none() {
        return Err(Failure::usage(
            "A prepared product runtime requires matching catalog metadata.",
        ));
    }
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
        let source = if manifest
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.path == member.path)
        {
            if let Some(file) = prepared_runtime.take() {
                Held::from_prepared_runtime(root, file)?
            } else {
                Held::open(root, &member.path)?
            }
        } else {
            Held::open(root, &member.path)?
        };
        sources.push(source);
    }
    if prepared_runtime.is_some() {
        return Err(Failure::usage(
            "The prepared product runtime was not verified.",
        ));
    }
    let mut media_budget = super::media::Budget::default();
    for (source, member) in sources.iter_mut().zip(&manifest.members) {
        let product_runtime = manifest
            .runtime
            .as_ref()
            .filter(|runtime| runtime.path == member.path);
        let database = manifest
            .database
            .as_ref()
            .is_some_and(|database| database.path == member.path);
        if database {
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
        if let Some(runtime) = product_runtime {
            let inspected = runtime::inspect(runtime, PositionalReader::new(&source.file))?;
            files::matching(
                &inspected.digest,
                inspected.size,
                &member.sha256,
                member.bytes,
            )?;
        }
        let binary = database || product_runtime.is_some();
        let receipt = if let Some(destination) = destination {
            let mut output = new_member(destination, &member.path)?;
            let receipt = if binary {
                copy_binary_limit(&source.file, &mut output, member.bytes)?
            } else if is_media(&member.media_type) {
                super::media::copy_checked_with_budget(
                    &mut source.file,
                    &mut output,
                    &member.media_type,
                    member.bytes,
                    &mut media_budget,
                )?
            } else {
                guard::copy_checked_limit(&mut source.file, &mut output, member.bytes)?
            };
            output.sync_all()?;
            receipt
        } else {
            if binary {
                copy_binary_limit(&source.file, io::sink(), member.bytes)?
            } else if is_media(&member.media_type) {
                super::media::copy_checked_with_budget(
                    &mut source.file,
                    io::sink(),
                    &member.media_type,
                    member.bytes,
                    &mut media_budget,
                )?
            } else {
                guard::copy_checked_limit(&mut source.file, io::sink(), member.bytes)?
            }
        };
        files::matching(&receipt.digest, receipt.size, &member.sha256, member.bytes)?;
        if is_media(&member.media_type) {
            match_media_review(
                manifest.media_review.as_ref(),
                &member.path,
                &receipt.digest,
            )?;
        }
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
pub(crate) fn source_check(
    root: &Path,
    source_root_path: &str,
    selector: Option<&str>,
    manifest: &Manifest,
) -> Value {
    let result = (|| {
        let runtime_matches = if let Some(runtime) = &manifest.runtime {
            let selector =
                selector.ok_or_else(|| Failure::usage("Missing bundle selection reference."))?;
            // Compare the selected identity without consulting capture availability
            // or opening a project file at the generated member's path.
            let selected = read_selection(root, selector)?;
            let expected_source = manifest
                .database
                .as_ref()
                .and_then(|database| database.source.as_ref())
                .map(|source| DatabaseSource {
                    profile: source.profile.clone(),
                    wal: source.wal.path.clone(),
                    shm: source.shm.clone(),
                });
            selected.runtime.as_deref() == Some(runtime.id.as_str())
                && selected.database_source == expected_source
        } else {
            true
        };
        let root = files::open_root(root)?;
        let source = source_root(&root, source_root_path)?;
        let mut sources = Vec::new();
        let members: Vec<_> = manifest
            .members
            .iter()
            .filter(|member| {
                manifest
                    .runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.path != member.path)
            })
            .collect();
        for member in &members {
            sources.push(Held::open(&source, &member.path)?);
        }
        let mut matches = runtime_matches;
        let mut auxiliary = Vec::new();
        let mut media_budget = super::media::Budget::default();
        for (held, member) in sources.iter_mut().zip(members) {
            if let Some(database) = manifest
                .database
                .as_ref()
                .filter(|database| database.path == member.path)
            {
                let receipt = copy_binary_limit(&held.file, io::sink(), MAX_BYTES)?;
                matches &= receipt.digest == database.source_sha256
                    && receipt.size == database.source_bytes;
                if let Some(selected) = &database.source {
                    let wal = Held::open(&source, &selected.wal.path)?;
                    let shm = Held::open(&source, &selected.shm)?;
                    check_wal_sources(held, &wal, &shm)?;
                    let receipt = copy_binary_limit(
                        &wal.file,
                        io::sink(),
                        MAX_BYTES - database.source_bytes,
                    )?;
                    matches &=
                        receipt.digest == selected.wal.sha256 && receipt.size == selected.wal.bytes;
                    auxiliary.extend([wal, shm]);
                }
            } else if is_media(&member.media_type) {
                let receipt = super::media::copy_checked_with_budget(
                    &mut held.file,
                    io::sink(),
                    &member.media_type,
                    MAX_BYTES,
                    &mut media_budget,
                )?;
                matches &= receipt.digest == member.sha256 && receipt.size == member.bytes;
            } else {
                let receipt = guard::copy_checked_limit(&mut held.file, io::sink(), MAX_BYTES)?;
                matches &= receipt.digest == member.sha256 && receipt.size == member.bytes;
            }
        }
        recheck_all(&source, &sources)?;
        recheck_selected_root(&root, &source, source_root_path, &sources)?;
        recheck_all(&source, &auxiliary)?;
        recheck_selected_root(&root, &source, source_root_path, &auxiliary)?;
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

    #[cfg(unix)]
    #[test]
    fn wal_checks_keep_the_original_directory_and_reject_replaced_names() {
        let scratch = Scratch::new();
        fs::create_dir(scratch.0.join("app")).unwrap();
        for name in ["data.sqlite", "data.sqlite-wal", "data.sqlite-shm"] {
            fs::write(scratch.0.join("app").join(name), []).unwrap();
        }
        let root = files::open_root(&scratch.0).unwrap();
        let source = source_root(&root, "app").unwrap();
        let database = Held::open(&source, "data.sqlite").unwrap();
        let wal = Held::open(&source, "data.sqlite-wal").unwrap();
        let shm = Held::open(&source, "data.sqlite-shm").unwrap();
        check_wal_sources(&database, &wal, &shm).unwrap();
        fs::rename(scratch.0.join("app"), scratch.0.join("old")).unwrap();
        fs::create_dir(scratch.0.join("app")).unwrap();
        for name in ["data.sqlite", "data.sqlite-wal", "data.sqlite-shm"] {
            fs::write(scratch.0.join("app").join(name), []).unwrap();
        }
        fs::write(scratch.0.join("old/data.sqlite-journal"), []).unwrap();
        assert!(check_wal_sources(&database, &wal, &shm).is_err());
        fs::remove_file(scratch.0.join("old/data.sqlite-journal")).unwrap();
        fs::rename(
            scratch.0.join("old/data.sqlite-wal"),
            scratch.0.join("old/previous-wal"),
        )
        .unwrap();
        fs::write(scratch.0.join("old/data.sqlite-wal"), []).unwrap();
        assert!(check_wal_sources(&database, &wal, &shm).is_err());
        assert!(recheck_selected_root(&root, &source, "app", &[database, wal, shm]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn wal_name_checks_preserve_the_outer_sqlite_leases() {
        use libsqlite3_sys as ffi;
        use std::cell::RefCell;
        let writer = RefCell::new(super::super::sqlite::vfs::wal_tests::Writer::start());
        let path = writer.borrow().path("");
        let root = files::open_root(path.parent().unwrap()).unwrap();
        let database = Held::open(&root, "source.db").unwrap();
        let wal = Held::open(&root, "source.db-wal").unwrap();
        let shm = Held::open(&root, "source.db-shm").unwrap();
        let output = new_member(&root, "snapshot.db").unwrap();
        let calls = std::cell::Cell::new(0);
        super::super::sqlite::normalize_wal(&database.file, &wal.file, &shm.file, &output, || {
            check_wal_sources(&database, &wal, &shm)?;
            calls.set(calls.get() + 1);
            assert_eq!(writer.borrow_mut().command("write"), ffi::SQLITE_BUSY);
            assert_eq!(writer.borrow_mut().command("checkpoint"), ffi::SQLITE_BUSY);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(writer.borrow_mut().command("write"), ffi::SQLITE_OK);
        assert_eq!(writer.borrow_mut().command("checkpoint"), ffi::SQLITE_OK);
    }

    #[test]
    fn wal_source_manifest_requires_exact_versioned_metadata() {
        let value = json!({
            "schema_version":5,"delivery_type":"sqlite_application","entrypoint":"index.html","requirements":[],
            "runtime":runtime::descriptor(runtime::ID).unwrap(),
            "database":{"path":"data.sqlite","profile":SQLITE_PROFILE,"transformation":SQLITE_TRANSFORMATION,"source_bytes":512,"source_sha256":"a".repeat(64),
            "source":{"profile":WAL_PROFILE,"wal":{"path":"data.sqlite-wal","bytes":32,"sha256":"b".repeat(64)},"shm":"data.sqlite-shm"}},
            "members":[{"path":"data.sqlite","bytes":512,"sha256":"c".repeat(64),"media_type":SQLITE_MEDIA_TYPE},
            {"path":"index.html","bytes":1,"sha256":"d".repeat(64),"media_type":"text/html"},
            {"path":runtime::PATH,"bytes":runtime::BYTES,"sha256":runtime::SHA256,"media_type":"text/javascript"}]
        });
        let valid = |value: &Value| {
            serde_json::from_value::<Manifest>(value.clone())
                .is_ok_and(|manifest| validate_manifest(&manifest).is_ok())
        };
        assert!(valid(&value));
        let mut empty = value.clone();
        empty["database"]["source"]["wal"]["bytes"] = json!(0);
        assert!(valid(&empty));
        let mut cases = Vec::new();
        let mut excessive_input = value.clone();
        excessive_input["database"]["source_bytes"] = json!(MAX_BYTES - runtime::BYTES);
        cases.push(excessive_input);
        for bytes in [1, 31, MAX_BYTES] {
            let mut case = value.clone();
            case["database"]["source"]["wal"]["bytes"] = json!(bytes);
            cases.push(case);
        }
        for version in [1, 3, 4] {
            let mut case = value.clone();
            case["schema_version"] = json!(version);
            cases.push(case);
        }
        for source in [
            Value::Null,
            json!({}),
            json!({"profile":WAL_PROFILE,"wal":{"path":"data.sqlite-wal","bytes":32,"sha256":"b".repeat(64)},"shm":"data.sqlite-shm","extra":true}),
        ] {
            let mut case = value.clone();
            case["database"]["source"] = source;
            cases.push(case);
        }
        let mut missing = value.clone();
        missing["database"]
            .as_object_mut()
            .unwrap()
            .remove("source");
        cases.push(missing);
        for (field, entry) in [("profile", json!("other")), ("shm", json!("other-shm"))] {
            let mut case = value.clone();
            case["database"]["source"][field] = entry;
            cases.push(case);
        }
        for (field, entry) in [
            ("path", json!("../data.sqlite-wal")),
            ("sha256", json!("A".repeat(64))),
            ("extra", json!(true)),
        ] {
            let mut case = value.clone();
            case["database"]["source"]["wal"][field] = entry;
            cases.push(case);
        }
        for case in cases {
            assert!(!valid(&case), "accepted invalid source metadata: {case}");
        }
    }

    #[test]
    fn reviewed_media_selection_requires_exact_complete_attestation() {
        let valid = json!({
            "root": ".", "entrypoint": "index.html",
            "members": ["index.html", "assets/frame.png", "clip.mp4"],
            "media_review": {"profile": "reviewed-media-v1", "members": [
                {"path": "assets/frame.png", "sha256": "a".repeat(64)},
                {"path": "clip.mp4", "sha256": "b".repeat(64)}
            ]}
        });
        let parse = |value: &Value| serde_json::from_value::<BundleSelection>(value.clone());
        assert!(validate_selection(&parse(&valid).expect("reviewed media selector")).is_ok());
        let mut invalid = Vec::new();
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove("media_review");
        invalid.push(value);
        let mut value = valid.clone();
        value["media_review"] = Value::Null;
        invalid.push(value);
        for (field, replacement) in [
            ("profile", json!("automatic-media-v1")),
            ("members", json!([])),
        ] {
            let mut value = valid.clone();
            value["media_review"][field] = replacement;
            invalid.push(value);
        }
        for (field, replacement) in [
            ("path", json!("assets/FRAME.png")),
            ("sha256", json!("A".repeat(64))),
            ("path", json!("index.html")),
        ] {
            let mut value = valid.clone();
            value["media_review"]["members"][0][field] = replacement;
            invalid.push(value);
        }
        let mut value = valid.clone();
        value["media_review"]["members"]
            .as_array_mut()
            .unwrap()
            .pop();
        invalid.push(value);
        let mut value = valid.clone();
        let duplicate = value["media_review"]["members"][0].clone();
        value["media_review"]["members"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        invalid.push(value);
        let mut value = valid.clone();
        value["members"] = json!(["index.html"]);
        invalid.push(value);
        let mut value = valid.clone();
        value["members"]
            .as_array_mut()
            .unwrap()
            .push(json!("data.sqlite"));
        value["database"] = json!("data.sqlite");
        value["runtime"] = json!(runtime::ID);
        invalid.push(value);
        for value in invalid {
            assert!(
                parse(&value).map_or(true, |selection| validate_selection(&selection).is_err()),
                "accepted invalid media review: {value}"
            );
        }
    }

    #[test]
    fn reviewed_media_manifest_requires_schema_four_and_matching_member_hashes() {
        let valid = json!({
            "schema_version": 4, "delivery_type": "bundle", "entrypoint": "index.html",
            "requirements": [], "members": [
                {"path": "frame.png", "bytes": 100, "sha256": "a".repeat(64), "media_type": "image/png"},
                {"path": "index.html", "bytes": 10, "sha256": "b".repeat(64), "media_type": "text/html"}
            ],
            "media_review": {"profile": "reviewed-media-v1", "members": [
                {"path": "frame.png", "sha256": "a".repeat(64)}
            ]}
        });
        let parse = |value: &Value| serde_json::from_value::<Manifest>(value.clone());
        assert!(validate_manifest(&parse(&valid).expect("media manifest")).is_ok());
        for schema in [1, 2, 3] {
            let mut value = valid.clone();
            value["schema_version"] = json!(schema);
            assert!(validate_manifest(&parse(&value).unwrap()).is_err());
        }
        let mut value = valid.clone();
        value["media_review"]["members"][0]["sha256"] = json!("c".repeat(64));
        assert!(validate_manifest(&parse(&value).unwrap()).is_err());
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove("media_review");
        assert!(validate_manifest(&parse(&value).unwrap()).is_err());
        let mut value = valid.clone();
        value["members"].as_array_mut().unwrap().remove(0);
        value.as_object_mut().unwrap().remove("media_review");
        assert!(validate_manifest(&parse(&value).unwrap()).is_err());
    }

    #[test]
    fn reviewed_media_custody_preserves_exact_bytes_and_rechecks_structure() {
        let (root, mut selection) = fixture();
        let png = include_bytes!("../../tests/data/artifact-media/sample.png");
        let mp4 = include_bytes!("../../tests/data/artifact-media/sample.mp4");
        let members = [
            ("assets/frame.png", png.as_slice()),
            ("clip.mp4", mp4.as_slice()),
        ];
        let reviewed = members
            .iter()
            .map(|(path, bytes)| {
                fs::write(root.0.join("app").join(path), bytes).unwrap();
                selection.members.push((*path).into());
                ReviewedMember {
                    path: (*path).into(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                }
            })
            .collect();
        selection.media_review = Some(MediaReview {
            profile: "reviewed-media-v1".into(),
            members: reviewed,
        });
        let saved = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let mut manifest = check(&root.0, &saved.object, &saved.digest, saved.size).unwrap();
        assert_eq!(manifest.schema_version, 4);
        let retained_manifest = fs::read(
            root.0
                .join(".vivac-artifacts")
                .join(&saved.object)
                .join(MANIFEST),
        )
        .unwrap();
        assert_eq!(
            saved.digest,
            format!("{:x}", Sha256::digest(&retained_manifest))
        );
        for (path, bytes) in members {
            assert!(manifest
                .media_review
                .as_ref()
                .unwrap()
                .members
                .iter()
                .any(|member| member.path == path
                    && member.sha256 == format!("{:x}", Sha256::digest(bytes))));
        }
        assert_eq!(
            source_check(&root.0, "app", None, &manifest)["status"],
            "matches"
        );
        restore(
            &root.0,
            &saved.object,
            &saved.digest,
            saved.size,
            "restored-media",
        )
        .unwrap();
        for (path, bytes) in members {
            assert_eq!(
                fs::read(root.0.join("restored-media").join(path)).unwrap(),
                bytes
            );
        }
        selection.media_review.as_mut().unwrap().members[0].sha256 = "0".repeat(64);
        assert!(capture(&root.0, &selection, &crate::id::ulid()).is_err());

        // A self-consistent forged manifest must still pass the media structure guard.
        let damaged = b"Invalid media structure";
        let digest = format!("{:x}", Sha256::digest(damaged));
        let member = manifest
            .members
            .iter_mut()
            .find(|member| member.path == "assets/frame.png")
            .unwrap();
        member.bytes = damaged.len() as u64;
        member.sha256 = digest.clone();
        manifest
            .media_review
            .as_mut()
            .unwrap()
            .members
            .iter_mut()
            .find(|member| member.path == "assets/frame.png")
            .unwrap()
            .sha256 = digest;
        let object = root.0.join(".vivac-artifacts").join(&saved.object);
        fs::write(object.join("assets/frame.png"), damaged).unwrap();
        let bytes = manifest_bytes(&manifest).unwrap();
        fs::write(object.join(MANIFEST), &bytes).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let total = bytes.len() as u64
            + manifest
                .members
                .iter()
                .map(|member| member.bytes)
                .sum::<u64>();
        assert!(check(&root.0, &saved.object, &digest, total).is_err());
        assert!(restore(&root.0, &saved.object, &digest, total, "refused-media").is_err());
        assert!(!root.0.join("refused-media").exists());
    }

    fn solid_png_review(
        root: &Path,
        path: &str,
        width: u32,
        height: u32,
        rgba16: bool,
    ) -> ReviewedMember {
        let image_path = root.join(path);
        let mut encoder = png::Encoder::new(fs::File::create(&image_path).unwrap(), width, height);
        encoder.set_color(if rgba16 {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        });
        encoder.set_depth(if rgba16 {
            png::BitDepth::Sixteen
        } else {
            png::BitDepth::Eight
        });
        let mut writer = encoder.write_header().unwrap();
        {
            let mut stream = writer.stream_writer_with_size(64 * 1024).unwrap();
            let row = vec![0u8; width as usize * if rgba16 { 8 } else { 3 }];
            for _ in 0..height {
                stream.write_all(&row).unwrap();
            }
            stream.finish().unwrap();
        }
        writer.finish().unwrap();
        let mut source = fs::File::open(&image_path).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = source.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        ReviewedMember {
            path: path.into(),
            sha256: format!("{:x}", digest.finalize()),
        }
    }

    #[test]
    fn reviewed_media_expansion_budget_is_shared_by_all_png_members() {
        let (root, mut selection) = fixture();
        let mut review = MediaReview {
            profile: "reviewed-media-v1".into(),
            members: Vec::new(),
        };
        for index in 0..3 {
            let path = format!("frame-{index}.png");
            review.members.push(solid_png_review(
                &root.0.join("app"),
                &path,
                2048,
                2048,
                false,
            ));
            selection.members.push(path);
        }
        let compressed_bytes: u64 = review
            .members
            .iter()
            .map(|member| {
                fs::metadata(root.0.join("app").join(&member.path))
                    .unwrap()
                    .len()
            })
            .sum();
        assert!(compressed_bytes < MAX_BYTES - MAX_MANIFEST_BYTES);
        selection.media_review = Some(review);
        assert!(
            capture(&root.0, &selection, &crate::id::ulid()).is_err(),
            "Three individually valid PNGs exceed the revision expansion budget."
        );
        selection.members.pop();
        selection.media_review.as_mut().unwrap().members.pop();
        let saved = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let mut manifest = check(&root.0, &saved.object, &saved.digest, saved.size).unwrap();
        // An internally consistent retained revision must enforce the same aggregate limit.
        let path = "frame-2.png";
        let source = root.0.join("app").join(path);
        let bytes = fs::metadata(&source).unwrap().len();
        let digest = format!("{:x}", Sha256::digest(fs::read(&source).unwrap()));
        let object = root.0.join(".vivac-artifacts").join(&saved.object);
        fs::copy(source, object.join(path)).unwrap();
        manifest.members.push(Member {
            path: path.into(),
            bytes,
            sha256: digest.clone(),
            media_type: "image/png".into(),
        });
        manifest
            .members
            .sort_by(|left, right| left.path.cmp(&right.path));
        manifest
            .media_review
            .as_mut()
            .unwrap()
            .members
            .push(ReviewedMember {
                path: path.into(),
                sha256: digest,
            });
        let raw = manifest_bytes(&manifest).unwrap();
        fs::write(object.join(MANIFEST), &raw).unwrap();
        let digest = format!("{:x}", Sha256::digest(&raw));
        let total = raw.len() as u64
            + manifest
                .members
                .iter()
                .map(|member| member.bytes)
                .sum::<u64>();
        assert!(check(&root.0, &saved.object, &digest, total).is_err());
        assert!(restore(&root.0, &saved.object, &digest, total, "over-budget").is_err());
        assert!(!root.0.join("over-budget").exists());
        assert_eq!(
            source_check(&root.0, "app", None, &manifest)["status"],
            "read_failed"
        );
    }

    #[test]
    fn reviewed_media_rejects_single_png_above_revision_expansion_budget() {
        let (root, mut selection) = fixture();
        let review = solid_png_review(&root.0.join("app"), "large.png", 4096, 4096, true);
        assert!(
            fs::metadata(root.0.join("app/large.png")).unwrap().len()
                < MAX_BYTES - MAX_MANIFEST_BYTES
        );
        selection.members.push("large.png".into());
        selection.media_review = Some(MediaReview {
            profile: "reviewed-media-v1".into(),
            members: vec![review],
        });
        assert!(capture(&root.0, &selection, &crate::id::ulid()).is_err());
    }

    fn prepared_runtime_fixture() -> (Scratch, Dir, File, files::Retained) {
        let (root, selection, _, _, _) = sqlite_benchmark_fixture(None);
        let retained = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let (_, _, bundle) = open_object(&root.0, &retained.object).unwrap();
        bundle.remove_file(runtime::PATH).unwrap();
        let mut output = new_member(&bundle, runtime::PATH).unwrap();
        let receipt = runtime::materialize(runtime::ID, &mut output).unwrap();
        files::matching(
            &receipt.digest,
            receipt.size,
            runtime::SHA256,
            runtime::BYTES,
        )
        .unwrap();
        output.sync_all().unwrap();
        (root, bundle, output, retained)
    }

    #[test]
    fn prepared_runtime_verification_reads_the_complete_file_from_zero() {
        use std::io::{Seek, SeekFrom};
        let (_root, bundle, mut output, retained) = prepared_runtime_fixture();
        assert_eq!(output.seek(SeekFrom::End(0)).unwrap(), runtime::BYTES);
        let manifest = verify_directory_inner(
            &bundle,
            Some((&retained.digest, retained.size)),
            None,
            Some(output),
        )
        .unwrap();
        assert_eq!(
            manifest.runtime.unwrap(),
            runtime::known_descriptor(runtime::ID).unwrap()
        );
        assert_eq!(manifest.members.len(), 4);
    }

    #[cfg(windows)]
    #[test]
    fn prepared_runtime_attributes_preserve_identity_without_allowing_byte_reads() {
        let (_root, bundle, output, _retained) = prepared_runtime_fixture();
        let mut attributes = open_prepared_runtime_attributes(&bundle).unwrap();
        assert!(attributes.metadata().unwrap().is_file());
        files::unchanged(&output.metadata().unwrap(), &attributes.metadata().unwrap()).unwrap();
        assert_eq!(
            files::file_identity(&attributes).unwrap(),
            files::file_identity(&output).unwrap()
        );
        files::same_source(&output, &attributes).unwrap();
        let error = attributes.read(&mut [0u8; 1]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let inspected = runtime::inspect(
            &runtime::known_descriptor(runtime::ID).unwrap(),
            PositionalReader::new(&output),
        )
        .unwrap();
        assert_eq!(inspected.size, runtime::BYTES);
        assert_eq!(inspected.digest, runtime::SHA256);
    }

    #[test]
    fn prepared_runtime_verification_refuses_bytes_changed_through_the_same_file() {
        use std::io::{Seek, SeekFrom};
        let (_root, bundle, mut output, retained) = prepared_runtime_fixture();
        output.seek(SeekFrom::Start(0)).unwrap();
        output.write_all(b"\0").unwrap();
        output.sync_all().unwrap();
        let error = verify_directory_inner(
            &bundle,
            Some((&retained.digest, retained.size)),
            None,
            Some(output),
        )
        .unwrap_err();
        assert!(
            matches!(error, Failure::Usage(message) if message.trim() == "Retained product runtime bytes do not match the engine catalog.")
        );
    }

    #[test]
    fn prepared_runtime_verification_refuses_a_replaced_name_with_identical_bytes() {
        let (root, bundle, output, retained) = prepared_runtime_fixture();
        fs::rename(
            root.0
                .join(".vivac-artifacts")
                .join(&retained.object)
                .join(runtime::PATH),
            root.0.join("moved-runtime.js"),
        )
        .unwrap();
        let mut replacement = new_member(&bundle, runtime::PATH).unwrap();
        let receipt = runtime::materialize(runtime::ID, &mut replacement).unwrap();
        files::matching(
            &receipt.digest,
            receipt.size,
            runtime::SHA256,
            runtime::BYTES,
        )
        .unwrap();
        replacement.sync_all().unwrap();
        drop(replacement);
        let manifest = load_manifest(&bundle, Some((&retained.digest, retained.size)))
            .unwrap()
            .0;
        let (expected_files, expected_dirs) = expected_paths(&manifest);
        let mut seen = BTreeSet::new();
        exact_set(&bundle, "", &expected_files, &expected_dirs, &mut seen).unwrap();
        assert_eq!(seen, expected_files);
        let inspected = runtime::inspect(
            manifest.runtime.as_ref().unwrap(),
            PositionalReader::new(&output),
        )
        .unwrap();
        assert_eq!(inspected.digest, runtime::SHA256);
        assert_eq!(inspected.size, runtime::BYTES);
        let error = verify_directory_inner(
            &bundle,
            Some((&retained.digest, retained.size)),
            None,
            Some(output),
        )
        .unwrap_err();
        assert!(
            matches!(&error, Failure::Usage(message) if message.trim() == "The artifact source changed while its bytes were being read." || message.trim() == "The artifact source was replaced while its bytes were being read." || message.trim() == "The bundle source identity changed while preparing the revision."),
            "{error:?}"
        );
    }

    #[test]
    fn prepared_runtime_verification_requires_a_matching_runtime_descriptor() {
        let (root, selection) = fixture();
        let retained = capture(&root.0, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let (project, _, bundle) = open_object(&root.0, &retained.object).unwrap();
        let mut output = new_member(&project, "prepared-runtime.js").unwrap();
        runtime::materialize(runtime::ID, &mut output).unwrap();
        output.sync_all().unwrap();
        let error = verify_directory_inner(
            &bundle,
            Some((&retained.digest, retained.size)),
            None,
            Some(output),
        )
        .unwrap_err();
        assert!(
            matches!(error, Failure::Usage(message) if message.trim() == "A prepared product runtime requires matching catalog metadata.")
        );
    }

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
            database_source: None,
            runtime: None,
            media_review: None,
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
            runtime: None,
            media_review: None,
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
            database_source: None,
            runtime: None,
            media_review: None,
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
            database_source: None,
            runtime: None,
            media_review: None,
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
                database_source: None,
                runtime: None,
                media_review: None,
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
                    runtime: None,
                    media_review: None,
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
    #[test]
    #[ignore = "Run explicitly in release mode with --test-threads=1 to measure reviewed media retention."]
    fn retained_reviewed_media_release_benchmark() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        let mut failures = Vec::new();
        for (label, target, budget, high_expansion) in [
            ("one_mib", 1024 * 1024, 500.0, false),
            ("ten_mib", 10 * 1024 * 1024, 2000.0, false),
            ("high_expansion_rgba16", 1024 * 1024, 500.0, true),
        ] {
            let root = Scratch::new();
            fs::write(
                root.0.join("index.html"),
                b"<!doctype html><img src=frame.png alt='Synthetic noise'>",
            )
            .unwrap();
            let width = if high_expansion { 2048u32 } else { 1024u32 };
            let row_bytes = width as usize * if high_expansion { 8 } else { 3 };
            let mut height = if high_expansion {
                2047
            } else {
                ((target - 4096) / row_bytes) as u32
            };
            let image_path = root.0.join("frame.png");
            let image_bytes = loop {
                assert!(
                    width <= 4096
                        && height <= 4096
                        && u64::from(width) * u64::from(height) <= 16_777_216
                );
                let mut encoder =
                    png::Encoder::new(fs::File::create(&image_path).unwrap(), width, height);
                encoder.set_color(if high_expansion {
                    png::ColorType::Rgba
                } else {
                    png::ColorType::Rgb
                });
                encoder.set_depth(if high_expansion {
                    png::BitDepth::Sixteen
                } else {
                    png::BitDepth::Eight
                });
                let mut writer = encoder.write_header().unwrap();
                {
                    let mut stream = writer.stream_writer_with_size(64 * 1024).unwrap();
                    let mut row = vec![0u8; row_bytes];
                    let mut state = 0x6d2b_79f5u32;
                    for _ in 0..height {
                        if !high_expansion {
                            for byte in &mut row {
                                state ^= state << 13;
                                state ^= state >> 17;
                                state ^= state << 5;
                                *byte = state as u8;
                            }
                        }
                        stream.write_all(&row).unwrap();
                    }
                    stream.finish().unwrap();
                }
                writer.finish().unwrap();
                let length = fs::metadata(&image_path).unwrap().len() as usize;
                if high_expansion {
                    assert!(
                        length <= target - 4096,
                        "High expansion fixture must stay below one MiB."
                    );
                    break length;
                }
                if length <= target - 4096 {
                    break length;
                }
                let excess = length - (target - 4096);
                height -= excess.div_ceil(row_bytes) as u32;
            };
            let mut digest = Sha256::new();
            let mut image = fs::File::open(&image_path).unwrap();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = image.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            let selection = BundleSelection {
                root: ".".into(),
                entrypoint: "index.html".into(),
                members: vec!["frame.png".into(), "index.html".into()],
                requirements: vec![],
                database: None,
                database_source: None,
                runtime: None,
                media_review: Some(MediaReview {
                    profile: "reviewed-media-v1".into(),
                    members: vec![ReviewedMember {
                        path: "frame.png".into(),
                        sha256: format!("{:x}", digest.finalize()),
                    }],
                }),
            };
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
                if high_expansion {
                    assert!(measured_total <= target as u64);
                } else {
                    assert!((target as u64 - 8192..=target as u64).contains(&measured_total));
                }
                let project = files::open_root(&root.0).unwrap();
                files::store(&project, false)
                    .unwrap()
                    .remove_dir_all(&retained.object)
                    .unwrap();
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}",
                json!({"benchmark":"reviewed_media_retention","case":label,"samples":100,"profile":"release","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"filesystem_root":root.0,"total_bytes":measured_total,"image_bytes":image_bytes,"decoded_raster_bytes":row_bytes as u64 * u64::from(height),"width":width,"height":height,"members":2,"review_profile":"reviewed-media-v1","guards":"complete_png_structure_decode_metadata_review_hash_source_identity_git_native_exclusive","p50_ms":samples[49],"p99_ms":samples[98],"max_ms":samples[99],"peak_auxiliary_heap_bytes":maximum_heap,"stack_allowance_bytes":128*1024})
            );
            if samples[98] >= budget {
                failures.push(format!(
                    "{label} p99 {} ms exceeds {budget} ms",
                    samples[98]
                ));
            }
            if maximum_heap + 128 * 1024 > 2 * 1024 * 1024 {
                failures.push(format!(
                    "{label} auxiliary heap {maximum_heap} bytes plus stack exceeds 2 MiB"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    fn video_benchmark_bytes(text_bytes: usize) -> Vec<u8> {
        const SAMPLE: &[u8] = include_bytes!("../../tests/data/artifact-media/sample.mp4");
        const SAMPLES: u32 = 4096;
        fn atom(kind: &[u8], body: &[u8]) -> Vec<u8> {
            let mut result = Vec::with_capacity(8 + body.len());
            result.extend_from_slice(&u32::try_from(8 + body.len()).unwrap().to_be_bytes());
            result.extend_from_slice(kind);
            result.extend_from_slice(body);
            result
        }
        fn words(values: &[u32]) -> Vec<u8> {
            values
                .iter()
                .flat_map(|value| value.to_be_bytes())
                .collect()
        }
        fn rewrite(bytes: &[u8], sample_size: u32, media_start: u32) -> Vec<u8> {
            let mut result = Vec::new();
            let mut position = 0;
            while position < bytes.len() {
                let length =
                    u32::from_be_bytes(bytes[position..position + 4].try_into().unwrap()) as usize;
                let kind = &bytes[position + 4..position + 8];
                let body = &bytes[position + 8..position + length];
                let replacement = match kind {
                    b"trak" | b"mdia" | b"minf" | b"stbl" => {
                        rewrite(body, sample_size, media_start)
                    }
                    b"stsz" => words(&[0, sample_size, SAMPLES]),
                    b"stts" => words(&[0, 1, SAMPLES, 1]),
                    b"stsc" => words(&[0, 1, 1, SAMPLES, 1]),
                    b"stco" => words(&[0, 1, media_start]),
                    b"ctts" | b"stss" | b"edts" => {
                        position += length;
                        continue;
                    }
                    _ => body.to_vec(),
                };
                result.extend(atom(kind, &replacement));
                position += length;
            }
            result
        }
        let mut sei = vec![6, 5];
        let mut payload_size = 16 + text_bytes + 1;
        while payload_size >= 255 {
            sei.push(255);
            payload_size -= 255;
        }
        sei.push(payload_size as u8);
        sei.extend_from_slice(b"synthetic-video!");
        for index in 0..text_bytes {
            sei.push(b"Synthetic video benchmark. "[index % 27]);
        }
        sei.extend_from_slice(&[0, 128]);
        let mut sample = vec![0, 0, 0, 2, 0x65, 0x80];
        sample.extend_from_slice(&u32::try_from(sei.len()).unwrap().to_be_bytes());
        sample.extend_from_slice(&sei);
        let mut ftyp = None;
        let mut moov = None;
        let mut position = 0;
        while position < SAMPLE.len() {
            let length =
                u32::from_be_bytes(SAMPLE[position..position + 4].try_into().unwrap()) as usize;
            match &SAMPLE[position + 4..position + 8] {
                b"ftyp" => ftyp = Some(&SAMPLE[position..position + length]),
                b"moov" => moov = Some(&SAMPLE[position + 8..position + length]),
                _ => {}
            }
            position += length;
        }
        let mut result = ftyp.unwrap().to_vec();
        let media_start = u32::try_from(result.len() + 8).unwrap();
        result.extend_from_slice(
            &u32::try_from(8 + sample.len() * SAMPLES as usize)
                .unwrap()
                .to_be_bytes(),
        );
        result.extend_from_slice(b"mdat");
        for _ in 0..SAMPLES {
            result.extend_from_slice(&sample);
        }
        result.extend(atom(
            b"moov",
            &rewrite(moov.unwrap(), sample.len() as u32, media_start),
        ));
        result
    }

    #[test]
    #[ignore = "Run explicitly in release mode with --test-threads=1 to measure reviewed video retention."]
    fn retained_reviewed_video_release_benchmark() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        let mut failures = Vec::new();
        for (label, target, budget) in [
            ("one_mib", 1024 * 1024, 500.0),
            ("ten_mib", 10 * 1024 * 1024, 2000.0),
        ] {
            let root = Scratch::new();
            let mut text_bytes = (target - 4096) / 8192;
            let video = loop {
                let video = video_benchmark_bytes(text_bytes);
                if video.len() * 2 <= target - 4096 {
                    break video;
                }
                text_bytes -= 1;
            };
            let video_bytes = video.len() * 2;
            let digest = format!("{:x}", Sha256::digest(&video));
            for name in ["first.mp4", "second.mp4"] {
                fs::write(root.0.join(name), &video).unwrap();
            }
            drop(video);
            let mut html = b"<!doctype html><video src=first.mp4 controls></video><video src=second.mp4 controls></video>".to_vec();
            html.resize(target - video_bytes - 2048, b' ');
            fs::write(root.0.join("index.html"), &html).unwrap();
            drop(html);
            let selection = BundleSelection {
                root: ".".into(),
                entrypoint: "index.html".into(),
                members: vec!["first.mp4".into(), "index.html".into(), "second.mp4".into()],
                requirements: vec![],
                database: None,
                database_source: None,
                runtime: None,
                media_review: Some(MediaReview {
                    profile: "reviewed-media-v1".into(),
                    members: ["first.mp4", "second.mp4"]
                        .into_iter()
                        .map(|path| ReviewedMember {
                            path: path.into(),
                            sha256: digest.clone(),
                        })
                        .collect(),
                }),
            };
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
                assert!((target as u64 - 8192..=target as u64).contains(&measured_total));
                let project = files::open_root(&root.0).unwrap();
                files::store(&project, false)
                    .unwrap()
                    .remove_dir_all(&retained.object)
                    .unwrap();
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}",
                json!({"benchmark":"reviewed_video_retention","case":label,"samples":100,"profile":"release","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"filesystem_root":root.0,"total_bytes":measured_total,"video_bytes":video_bytes,"members":3,"video_members":2,"video_samples":8192,"nal_units":16384,"sei_text_bytes_per_sample":text_bytes,"review_profile":"reviewed-media-v1","guards":"complete_mp4_structure_sample_coverage_sei_metadata_review_hash_shared_work_budgets_source_identity_git_native_exclusive","p50_ms":samples[49],"p99_ms":samples[98],"max_ms":samples[99],"peak_auxiliary_heap_bytes":maximum_heap,"stack_allowance_bytes":128*1024})
            );
            if samples[98] >= budget {
                failures.push(format!(
                    "{label} p99 {} ms exceeds {budget} ms",
                    samples[98]
                ));
            }
            if maximum_heap + 128 * 1024 > 2 * 1024 * 1024 {
                failures.push(format!(
                    "{label} auxiliary heap {maximum_heap} bytes plus stack exceeds 2 MiB"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    fn sqlite_benchmark_fixture(
        target: Option<usize>,
    ) -> (Scratch, BundleSelection, u64, u64, u64) {
        use libsqlite3_sys as ffi;
        use std::ffi::CString;
        let root = Scratch::new();
        let selection = BundleSelection {
            root: ".".into(),
            entrypoint: "index.html".into(),
            members: vec!["data.sqlite".into(), "index.html".into(), "main.js".into()],
            requirements: vec![],
            database: Some("data.sqlite".into()),
            database_source: None,
            runtime: Some(runtime::ID.into()),
            media_review: None,
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
            let rows = target.map_or(1, |target| {
                (target - runtime::BYTES as usize) * 9 / 10 / 280
            });
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
        let target = target.unwrap_or(warmup.size as usize);
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
        let source_bytes = fs::metadata(root.0.join("data.sqlite")).unwrap().len();
        (root, selection, ready.size, source_bytes, database_bytes)
    }

    #[test]
    #[ignore = "Run explicitly in release mode with --test-threads=1 to measure SQLite application retention."]
    fn retained_sqlite_application_release_benchmark() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        let mut timing_failures = Vec::new();
        for (label, target, budget) in [
            ("minimum_complete", None, 500.0),
            (
                "runtime_plus_one_mib",
                Some(runtime::BYTES as usize + 1024 * 1024),
                500.0,
            ),
            ("ten_mib_complete", Some(10 * 1024 * 1024), 2000.0),
        ] {
            let (root, selection, target, source_bytes, database_bytes) =
                sqlite_benchmark_fixture(target);
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
                assert_eq!(retained.size, target);
                let project = files::open_root(&root.0).unwrap();
                files::store(&project, false)
                    .unwrap()
                    .remove_dir_all(&retained.object)
                    .unwrap();
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{}",
                json!({"benchmark":"sqlite_application_retention","case":label,"samples":100,"profile":"release","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"filesystem_root":root.0,"total_bytes":target,"database_source_bytes":source_bytes,"database_snapshot_bytes":database_bytes,"runtime_bytes":runtime::BYTES,"other_bytes":target-database_bytes-runtime::BYTES,"transformation":SQLITE_TRANSFORMATION,"members":4,"guards":"complete_product_runtime_catalog_logical_sqlite_utf8_sensitive_metadata_source_lock_identity_git_native_exclusive","p50_ms":samples[49],"p99_ms":samples[98],"max_ms":samples[99],"peak_rust_auxiliary_heap_bytes":maximum_rust,"peak_sqlite_heap_bytes":maximum_sqlite,"combined_conservative_heap_bytes":maximum_rust+maximum_sqlite,"stack_allowance_bytes":128*1024,"compiled_runtime_bytes":runtime::PACKED_LEN,"total_conservative_auxiliary_bytes":maximum_rust+maximum_sqlite+128*1024+runtime::PACKED_LEN})
            );
            let within_time_budget = samples[98] < budget;
            if !within_time_budget {
                timing_failures.push(format!(
                    "{label} p99 {} ms exceeds {budget} ms",
                    samples[98]
                ));
            }
            assert!(
                maximum_rust + maximum_sqlite + 128 * 1024 + runtime::PACKED_LEN <= 2 * 1024 * 1024,
                "{label} combined auxiliary memory exceeds 2 MiB"
            );
        }
        assert!(timing_failures.is_empty(), "{}", timing_failures.join("; "));
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
            database_source: None,
            runtime: None,
            media_review: None,
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
            database_source: None,
            runtime: None,
            media_review: None,
        };
        assert!(validate_selection(&selection).is_err());
    }
}

#[cfg(test)]
mod wal_bench;
