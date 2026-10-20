use crate::failure::Failure;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, Metadata, OpenOptions};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

pub struct Retained {
    pub digest: String,
    pub size: u64,
    pub object: String,
    pub site: String,
}
const STORE: &str = ".vivac-artifacts";

pub(crate) struct Download {
    reader: File,
    size: u64,
}

impl Download {
    pub(super) fn from_file(reader: File, size: u64) -> Self {
        Self { reader, size }
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            reader: self.reader.try_clone()?,
            size: self.size,
        })
    }
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn rewind(&mut self) -> io::Result<()> {
        self.reader.seek(SeekFrom::Start(0)).map(|_| ())
    }

    pub(crate) fn seek(&mut self, offset: u64) -> io::Result<()> {
        self.reader.seek(SeekFrom::Start(offset)).map(|_| ())
    }
}

impl Read for Download {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

pub(crate) fn download(
    root: &Path,
    object: &str,
    digest: &str,
    size: u64,
) -> Result<Download, Failure> {
    object_path(object)?;
    let root_dir = open_root(root)?;
    let store = store(&root_dir, false)?;
    let objects = directory(&store, "objects", false)?;
    let mut saved = regular(&store, object)?;
    let before = saved.metadata()?;
    #[cfg(windows)]
    let before_identity = file_identity(&saved)?;
    let temporary = format!(".tmp-{}", crate::id::ulid());
    check_git(root, object, &temporary)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut staged = objects.open_with(&temporary, &options)?;
    let result = (|| {
        check_git(root, object, &temporary)?;
        let receipt = super::guard::copy_checked(&mut saved, &mut staged)?;
        matching(&receipt.digest, receipt.size, digest, size)?;
        unchanged(&before, &saved.metadata()?)?;
        let current = regular(&store, object)?;
        unchanged(&before, &current.metadata()?)?;
        #[cfg(windows)]
        {
            same_source(&saved, &current)?;
            if before_identity != file_identity(&saved)? {
                return Err(Failure::usage(
                    "The retained artifact identity changed while preparing the download.",
                ));
            }
        }
        check_git(root, object, &temporary)?;
        staged.seek(SeekFrom::Start(0))?;
        objects.remove_file(&temporary)?;
        Ok(Download {
            reader: staged,
            size: receipt.size,
        })
    })();
    if result.is_err() {
        let _ = objects.remove_file(&temporary);
    }
    result
}

pub fn capture(root: &Path, source: &str, revision: &str) -> Result<Retained, Failure> {
    source_path(source)?;
    opaque_id(revision)?;
    let root_dir = open_root(root)?;
    let source_file = regular(&root_dir, source)?;
    capture_opened(root, root_dir, source, revision, source_file)
}

fn capture_opened(
    root: &Path,
    root_dir: Dir,
    source: &str,
    revision: &str,
    mut source_file: File,
) -> Result<Retained, Failure> {
    let before = source_file.metadata()?;
    #[cfg(windows)]
    let before_identity = file_identity(&source_file)?;
    check_tracking(root)?;
    let store = store(&root_dir, true)?;
    let objects = directory(&store, "objects", true)?;
    let object = format!("objects/{revision}.blob");
    check_git(root, &object, &format!(".tmp-{revision}"))?;
    let (temporary, mut staged) = temporary(&objects)?;
    let result = (|| {
        check_git(root, &object, &temporary)?;
        let receipt = super::guard::copy_checked(&mut source_file, &mut staged)?;
        unchanged(&before, &source_file.metadata()?)?;
        // A replacement of the pathname is observable even when the open
        // source handle still refers to the original file.
        let current_source = regular(&root_dir, source)?;
        unchanged(&before, &current_source.metadata()?)?;
        #[cfg(windows)]
        {
            same_source(&source_file, &current_source)?;
            if before_identity != file_identity(&source_file)? {
                return Err(Failure::usage(
                    "The artifact source identity changed while its bytes were being read.",
                ));
            }
        }
        staged.sync_all()?;
        drop(staged);
        let site = site(&store)?;
        sync_directory(&objects)?;
        check_git(root, &object, &temporary)?;
        objects.hard_link(&temporary, &objects, format!("{revision}.blob"))?;
        sync_directory(&objects).map_err(|_| published_failure(&object))?;
        objects
            .remove_file(&temporary)
            .map_err(|_| published_failure(&object))?;
        sync_directory(&objects).map_err(|_| published_failure(&object))?;
        Ok(Retained {
            digest: receipt.digest,
            size: receipt.size,
            object: object.clone(),
            site,
        })
    })();
    // Only the temporary file belongs to this invocation. Published revisions
    // remain available for diagnosis if a subsequent durability step fails.
    if result.is_err() {
        let _ = objects.remove_file(&temporary);
    }
    result
}

pub fn inspect(root: &Path, path: &str) -> Result<Option<(String, u64)>, Failure> {
    source_path(path)?;
    let dir = open_root(root)?;
    let mut file = match regular(&dir, path) {
        Ok(file) => file,
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let before = file.metadata()?;
    #[cfg(windows)]
    let before_identity = file_identity(&file)?;
    let receipt = hash(&mut file)?;
    unchanged(&before, &file.metadata()?)?;
    let current_source = regular(&dir, path)?;
    unchanged(&before, &current_source.metadata()?)?;
    #[cfg(windows)]
    {
        same_source(&file, &current_source)?;
        if before_identity != file_identity(&file)? {
            return Err(Failure::usage(
                "The artifact source identity changed while its bytes were being read.",
            ));
        }
    }
    Ok(Some(receipt))
}

#[cfg(test)]
pub fn verify(root: &Path, object: &str, digest: &str, size: u64) -> Result<(), Failure> {
    object_path(object)?;
    let root = open_root(root)?;
    let store = store(&root, false)?;
    let mut file = regular(&store, object)?;
    let before = file.metadata()?;
    let actual = hash(&mut file)?;
    unchanged(&before, &file.metadata()?)?;
    matching(&actual.0, actual.1, digest, size)
}

pub fn site_id(root: &Path) -> Result<Option<String>, Failure> {
    let root = open_root(root)?;
    let result = (|| {
        let store = store(&root, false)?;
        let mut site = String::new();
        regular(&store, ".site")?
            .take(27)
            .read_to_string(&mut site)?;
        opaque_id(&site)?;
        Ok(site)
    })();
    match result {
        Ok(site) => Ok(Some(site)),
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn inspect_retained(root: &Path, object: &str) -> Result<Option<(String, u64)>, Failure> {
    object_path(object)?;
    let root = open_root(root)?;
    let result = (|| {
        let store = store(&root, false)?;
        let mut file = regular(&store, object)?;
        let before = file.metadata()?;
        let actual = hash(&mut file)?;
        unchanged(&before, &file.metadata()?)?;
        Ok(actual)
    })();
    match result {
        Ok(actual) => Ok(Some(actual)),
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn restore(
    root: &Path,
    object: &str,
    digest: &str,
    size: u64,
    destination: &str,
) -> Result<(), Failure> {
    object_path(object)?;
    source_path(destination)?;
    let root_dir = open_root(root)?;
    let store = store(&root_dir, false)?;
    let mut saved = regular(&store, object)?;
    let before = saved.metadata()?;
    let (parent, leaf) = parent(&root_dir, destination)?;
    match parent.symlink_metadata(&leaf) {
        Ok(_) => {
            return Err(Failure::usage(
                "The artifact destination is already occupied.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let (temporary, mut staged) = temporary(&parent)?;
    let result = (|| {
        let receipt = super::guard::copy_checked(&mut saved, &mut staged)?;
        matching(&receipt.digest, receipt.size, digest, size)?;
        unchanged(&before, &saved.metadata()?)?;
        staged.sync_all()?;
        drop(staged);
        sync_directory(&parent)?;
        parent.hard_link(&temporary, &parent, &leaf)?;
        sync_directory(&parent).map_err(|_| published_failure(destination))?;
        parent
            .remove_file(&temporary)
            .map_err(|_| published_failure(destination))?;
        sync_directory(&parent).map_err(|_| published_failure(destination))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = parent.remove_file(&temporary);
    }
    result
}

pub fn for_each_object(
    root: &Path,
    mut visit: impl FnMut(String) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let root = open_root(root)?;
    let store = match store(&root, false) {
        Ok(dir) => dir,
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let objects = match directory(&store, "objects", false) {
        Ok(dir) => dir,
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in objects.entries()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| Failure::usage("Invalid artifact object name."))?;
        if let Some(id) = name.strip_prefix(".tmp-bundle-") {
            opaque_id(id)?;
            reject_link(&objects.symlink_metadata(&name)?)?;
            objects.open_dir_nofollow(&name)?;
            visit(format!("objects/{name}"))?;
            continue;
        }
        if let Some(id) = name.strip_suffix(".bundle") {
            opaque_id(id)?;
            reject_link(&objects.symlink_metadata(&name)?)?;
            objects.open_dir_nofollow(&name)?;
            visit(format!("objects/{name}"))?;
            continue;
        }
        if let Some(id) = name.strip_prefix(".tmp-") {
            opaque_id(id)?;
            regular(&objects, &name)?;
            visit(format!("objects/{name}"))?;
            continue;
        }
        let path = format!("objects/{name}");
        object_path(&path)?;
        regular(&objects, &name)?;
        visit(path)?;
    }
    Ok(())
}

pub(super) fn source_path(path: &str) -> Result<(), Failure> {
    if path.is_empty()
        || path.contains(['\\', ':'])
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Failure::usage(
            "Artifact paths must be normalized project-relative paths.",
        ));
    }
    for part in path.split('/') {
        let name = part.to_ascii_lowercase();
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ');
        let device = matches!(
            stem,
            "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" | "clock$"
        ) || ["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        let private = matches!(
            name.as_str(),
            ".vivac"
                | STORE
                | ".git"
                | ".ssh"
                | ".aws"
                | ".gnupg"
                | "credentials"
                | "secrets"
                | "private_key"
                | "privatekey"
                | "privatekeys"
                | "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
        ) || name == ".env"
            || name.starts_with(".env.")
            || name.starts_with("credentials.")
            || name.starts_with("secrets.")
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|suffix| name.ends_with(suffix));
        if part.is_empty() || part.ends_with(['.', ' ']) || device || private {
            return Err(Failure::usage(
                "Artifact paths must not name private, reserved, or credential locations.",
            ));
        }
    }
    if let Some(finding) = crate::redact::check_field("artifact path", path) {
        return Err(Failure::Redaction(Box::new(finding)));
    }
    Ok(())
}

pub(super) fn opaque_id(id: &str) -> Result<(), Failure> {
    if id.len() != 26
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(Failure::usage(
            "Invalid artifact revision or site identifier.",
        ));
    }
    Ok(())
}

fn object_path(path: &str) -> Result<(), Failure> {
    let id = path
        .strip_prefix("objects/")
        .and_then(|name| name.strip_suffix(".blob"))
        .ok_or_else(|| Failure::usage("Invalid artifact object path."))?;
    opaque_id(id)
}

pub(super) fn open_root(root: &Path) -> Result<Dir, Failure> {
    let absolute = std::path::absolute(root)?;
    let parent_path = absolute
        .parent()
        .ok_or_else(|| Failure::usage("Artifact roots must name a project directory."))?;
    let leaf = absolute
        .file_name()
        .ok_or_else(|| Failure::usage("Artifact roots must name a project directory."))?;
    let parent = Dir::open_ambient_dir(parent_path, cap_std::ambient_authority())?;
    Ok(parent.open_dir_nofollow(leaf)?)
}

pub(super) fn parent(root: &Dir, path: &str) -> Result<(Dir, String), Failure> {
    let mut parts = path.split('/').peekable();
    let mut dir = root.try_clone()?;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok((dir, part.to_string()));
        }
        reject_link(&dir.symlink_metadata(part)?)?;
        dir = dir.open_dir_nofollow(part)?;
    }
    Err(Failure::usage("Empty artifact path."))
}

pub(super) fn regular(root: &Dir, path: &str) -> Result<File, Failure> {
    let (parent, leaf) = parent(root, path)?;
    reject_link(&parent.symlink_metadata(&leaf)?)?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = parent.open_with(leaf, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Failure::usage("Artifacts must be regular files."));
    }
    reject_link(&metadata)?;
    Ok(file)
}

pub(super) fn require_single_link(file: &File) -> Result<(), Failure> {
    #[cfg(unix)]
    let links = {
        use cap_std::fs::MetadataExt;
        file.metadata()?.nlink()
    };
    #[cfg(windows)]
    let links = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
        };
        let mut information = FILE_STANDARD_INFO::default();
        // SAFETY: the live file handle and correctly sized output buffer remain
        // valid throughout the call. The buffer is read only on success.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStandardInfo,
                (&mut information as *mut FILE_STANDARD_INFO).cast(),
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        u64::from(information.NumberOfLinks)
    };
    #[cfg(not(any(unix, windows)))]
    let links = 0;
    if links != 1 {
        return Err(Failure::usage(
            "SQLite source files must have exactly one hard link.",
        ));
    }
    Ok(())
}

pub(super) fn reject_link(metadata: &Metadata) -> Result<(), Failure> {
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use cap_std::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked {
        return Err(Failure::usage(
            "Artifacts must not cross symbolic links or reparse points.",
        ));
    }
    Ok(())
}

pub(super) fn directory(parent: &Dir, name: &str, create: bool) -> Result<Dir, Failure> {
    if create {
        match parent.create_dir(name) {
            Ok(()) => sync_directory(parent)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(parent.open_dir_nofollow(name)?)
}

pub(super) fn store(root: &Dir, create: bool) -> Result<Dir, Failure> {
    let store = directory(root, STORE, create)?;
    if create {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        match store.open_with(".gitignore", &options) {
            Ok(mut file) => {
                file.write_all(b"*\n")?;
                file.sync_all()?;
                sync_directory(&store)?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                regular(&store, ".gitignore")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(store)
}

pub(super) fn site(store: &Dir) -> Result<String, Failure> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    match store.open_with(".site", &options) {
        Ok(mut file) => {
            let site = crate::id::ulid();
            file.write_all(site.as_bytes())?;
            file.sync_all()?;
            sync_directory(store)?;
            Ok(site)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let mut site = String::new();
            regular(store, ".site")?
                .take(27)
                .read_to_string(&mut site)?;
            opaque_id(&site)?;
            Ok(site)
        }
        Err(error) => Err(error.into()),
    }
}

fn temporary(dir: &Dir) -> Result<(String, File), Failure> {
    let name = format!(".tmp-{}", crate::id::ulid());
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    Ok((name.clone(), dir.open_with(name, &options)?))
}

fn hash(file: &mut File) -> Result<(String, u64), Failure> {
    if file.metadata()?.len() > super::MAX_FILE_BYTES {
        return Err(Failure::usage("Artifact files must not exceed 10 MiB."));
    }
    let mut digest = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
        if size > super::MAX_FILE_BYTES {
            return Err(Failure::usage("Artifact files must not exceed 10 MiB."));
        }
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

pub(super) fn matching(
    actual: &str,
    actual_size: u64,
    digest: &str,
    size: u64,
) -> Result<(), Failure> {
    if actual != digest || actual_size != size {
        return Err(Failure::Model(
            "The saved artifact bytes do not match the revision digest and size.".into(),
        ));
    }
    Ok(())
}

pub(super) fn unchanged(before: &Metadata, after: &Metadata) -> Result<(), Failure> {
    let mut changed = before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.created().ok() != after.created().ok();
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        changed |= before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec();
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        changed |= before.file_attributes() != after.file_attributes();
    }
    if changed {
        return Err(Failure::usage(
            "The artifact source changed while its bytes were being read.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn same_source(first: &File, second: &File) -> Result<(), Failure> {
    if file_identity(first)? != file_identity(second)? {
        return Err(Failure::usage(
            "The artifact source was replaced while its bytes were being read.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn file_identity(file: &File) -> Result<(u64, [u8; 16]), Failure> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut information = FILE_ID_INFO::default();
    // SAFETY: the capability file owns the live handle for the whole call.
    // The maintained binding defines the buffer's layout, and its byte size
    // matches FILE_ID_INFO. The buffer is read only after a successful call.
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut information as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok((
        information.VolumeSerialNumber,
        information.FileId.Identifier,
    ))
}

fn bounded_output(reader: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0; 8192];
    loop {
        let remaining = limit - output.len();
        let requested = buffer.len().min(remaining.saturating_add(1));
        let count = match reader.read(&mut buffer[..requested]) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if count == 0 {
            return Ok(output);
        }
        if count > remaining {
            return Err(io::Error::other("Git check output exceeded its limit."));
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn git_output(
    command: &mut Command,
    input: Option<&[u8]>,
    limit: usize,
) -> Result<Vec<u8>, Failure> {
    const ERROR: &str = "The artifact store's Git check failed.";
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| Failure::usage(ERROR))?;
    let result = (|| {
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other(ERROR))?
                .write_all(input)?;
        }
        let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other(ERROR))?;
        bounded_output(&mut stdout, limit)
    })();
    let output = match result {
        Ok(output) => output,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::usage(ERROR));
        }
    };
    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::usage(ERROR));
        }
    };
    if !status.success() {
        return Err(Failure::usage(ERROR));
    }
    Ok(output)
}

pub(super) fn check_tracking(root: &Path) -> Result<bool, Failure> {
    // A plain project is supported, but a Git marker makes every subsequent
    // Git check mandatory. Errors never turn a repository into a plain project.
    let absolute = std::path::absolute(root)?;
    let mut repository = false;
    for ancestor in absolute.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                repository = true;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !repository {
        return Ok(false);
    }
    git_output(
        git_command(root).args(["ls-files", "-z", "--", STORE]),
        None,
        0,
    )
    .map_err(|_| {
        Failure::usage("The artifact store must be untracked by Git; the tracking check failed.")
    })?;
    Ok(true)
}

pub(super) fn check_git(root: &Path, object: &str, temporary: &str) -> Result<(), Failure> {
    if !check_tracking(root)? {
        return Ok(());
    }
    let paths = [
        format!("{STORE}/{object}"),
        format!("{STORE}/objects/{temporary}"),
        format!("{STORE}/.site"),
    ];
    let mut input = Vec::new();
    for path in &paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut command = git_command(root);
    command.args(["check-ignore", "--no-index", "--verbose", "-z", "--stdin"]);
    let (prefix, ignored) = std::thread::scope(|scope| {
        let prefix = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn_scoped(scope, || {
                git_output(
                    git_command(root).args(["rev-parse", "--show-prefix"]),
                    None,
                    4096,
                )
            })
            .map_err(|_| Failure::usage("The artifact store's Git check failed."))?;
        let ignored = git_output(&mut command, Some(&input), 64 * 1024);
        let prefix = prefix
            .join()
            .map_err(|_| Failure::usage("The artifact store's Git check failed."));
        Ok::<_, Failure>((prefix, ignored))
    })?;
    let prefix = String::from_utf8(prefix??)
        .map_err(|_| Failure::usage("The artifact store's Git location is not UTF-8."))?;
    let prefix = prefix.strip_suffix('\n').unwrap_or(&prefix);
    let prefix = prefix.strip_suffix('\r').unwrap_or(prefix);
    if prefix.contains(['\0', '\r', '\n']) {
        return Err(Failure::usage(
            "The artifact store's Git location has invalid delimiters.",
        ));
    }
    let source = format!("{prefix}{STORE}/.gitignore");
    let ignored = ignored?;
    let mut fields = ignored.split(|byte| *byte == 0);
    let records_valid = paths.iter().all(|path| {
        let origin = fields.next();
        let line = fields.next();
        let pattern = fields.next();
        let checked = fields.next();
        origin == Some(source.as_bytes())
            && line.is_some()
            && pattern == Some(&b"*"[..])
            && checked == Some(path.as_bytes())
    });
    let valid = records_valid
        && match fields.next() {
            None => true,
            Some(field) => field.is_empty() && fields.next().is_none(),
        };
    if !valid {
        return Err(Failure::usage("Artifact objects, temporary files, and site identity must be excluded by the store's own '*' Git rule."));
    }
    Ok(())
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root);
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ] {
        command.env_remove(variable);
    }
    command
}

pub(super) fn sync_directory(dir: &Dir) -> Result<(), Failure> {
    #[cfg(windows)]
    {
        use cap_fs_ext::OpenOptionsMaybeDirExt;
        use cap_std::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .maybe_dir(true)
            .custom_flags(0x0200_0000)
            .follow(FollowSymlinks::No);
        dir.open_with(".", &options)?.sync_all()?;
    }
    #[cfg(unix)]
    {
        use cap_fs_ext::OpenOptionsMaybeDirExt;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .maybe_dir(true)
            .follow(FollowSymlinks::No);
        dir.open_with(".", &options)?.sync_all()?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err(Failure::usage(
            "This platform cannot synchronize artifact directories.",
        ));
    }
    Ok(())
}

fn published_failure(path: &str) -> Failure {
    Failure::Io(io::Error::other(format!("Artifact bytes were published at {path}, but durability could not be confirmed; retained bytes require review.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    const REVISION: &str = "01arz3ndektsv4rrffq69g5fav";
    #[test]
    fn git_output_is_bounded_before_a_large_stream_is_collected() {
        struct Huge {
            read: usize,
        }
        impl Read for Huge {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.read += buffer.len();
                buffer.fill(b'x');
                Ok(buffer.len())
            }
        }
        for limit in [0, 4096, 64 * 1024] {
            let mut stream = Huge { read: 0 };
            let (result, peak) = crate::test_alloc::measure(|| bounded_output(&mut stream, limit));
            assert!(result.is_err());
            assert!(
                peak <= 2 * 1024 * 1024,
                "Git output allocated {peak} auxiliary bytes"
            );
            assert_eq!(stream.read, limit + 1);
        }
        assert_eq!(bounded_output(&mut &b""[..], 0).unwrap(), b"");
        assert_eq!(bounded_output(&mut &b"abc"[..], 3).unwrap(), b"abc");
    }
    fn objects(root: &Path) -> Result<Vec<String>, Failure> {
        let mut paths = Vec::new();
        for_each_object(root, |path| {
            paths.push(path);
            Ok(())
        })?;
        paths.sort();
        Ok(paths)
    }
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("vivac-artifact-files-{}", crate::id::ulid()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success());
    }
    #[test]
    fn retained_revision_and_restore_never_replace_occupied_files() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        let second = capture(&dir.0, "source.txt", "01arz3ndektsv4rrffq69g5faw").unwrap();
        assert_eq!(saved.site, second.site);
        fs::write(dir.0.join("source.txt"), b"different artifact").unwrap();
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        verify(&dir.0, &saved.object, &saved.digest, saved.size).unwrap();
        fs::write(dir.0.join("occupied.txt"), b"keep me").unwrap();
        assert!(restore(
            &dir.0,
            &saved.object,
            &saved.digest,
            saved.size,
            "occupied.txt"
        )
        .is_err());
        assert_eq!(fs::read(dir.0.join("occupied.txt")).unwrap(), b"keep me");
        restore(
            &dir.0,
            &saved.object,
            &saved.digest,
            saved.size,
            "restored.txt",
        )
        .unwrap();
        assert_eq!(
            fs::read(dir.0.join("restored.txt")).unwrap(),
            b"safe artifact"
        );
    }

    #[test]
    fn prepared_download_is_unnamed_and_independent_of_later_retained_changes() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"selected saved delivery").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        let mut reader = download(&dir.0, &saved.object, &saved.digest, saved.size).unwrap();
        assert_eq!(reader.size(), saved.size);
        assert_eq!(objects(&dir.0).unwrap().len(), 1);
        fs::write(
            dir.0.join(STORE).join(&saved.object),
            b"later retained modification",
        )
        .unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"selected saved delivery");
        assert!(download(&dir.0, &saved.object, &saved.digest, saved.size).is_err());
        assert_eq!(objects(&dir.0).unwrap().len(), 1);
    }

    #[test]
    fn preview_download_rewind_reads_the_same_private_bytes_after_source_changes() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"selected saved delivery").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        let mut reader = download(&dir.0, &saved.object, &saved.digest, saved.size).unwrap();
        let mut first = Vec::new();
        reader.read_to_end(&mut first).unwrap();
        fs::write(dir.0.join("source.txt"), b"changed source").unwrap();
        fs::write(
            dir.0.join(STORE).join(&saved.object),
            b"changed retained bytes",
        )
        .unwrap();
        reader.rewind().unwrap();
        let mut second = Vec::new();
        reader.read_to_end(&mut second).unwrap();
        assert_eq!(first, b"selected saved delivery");
        assert_eq!(second, first);
        reader.rewind().unwrap();
        let mut prefix = [0; 8];
        reader.read_exact(&mut prefix).unwrap();
        assert_eq!(&prefix, b"selected");
        assert_eq!(objects(&dir.0).unwrap(), vec![saved.object]);
    }

    #[test]
    fn maximum_download_uses_bounded_auxiliary_memory_and_leaves_no_temporary() {
        let dir = Scratch::new();
        let mut source = fs::File::create(dir.0.join("source.txt")).unwrap();
        let block = [b' '; 64 * 1024];
        for _ in 0..160 {
            source.write_all(&block).unwrap();
        }
        drop(source);
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        let (result, peak) = crate::test_alloc::measure(|| {
            download(&dir.0, &saved.object, &saved.digest, saved.size)
        });
        let mut reader = result.unwrap();
        assert_eq!(reader.size(), super::super::MAX_FILE_BYTES);
        assert!(
            peak + 128 * 1024 <= 2 * 1024 * 1024,
            "download allocated {peak} auxiliary bytes"
        );
        assert_eq!(io::copy(&mut reader, &mut io::sink()).unwrap(), saved.size);
        assert_eq!(objects(&dir.0).unwrap().len(), 1);
    }

    #[test]
    fn download_refuses_paths_and_git_tracking_before_staging() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"safe selected delivery").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        for object in [
            "../outside",
            "objects/../../source.txt",
            "objects/not-an-id.blob",
        ] {
            assert!(download(&dir.0, object, &saved.digest, saved.size).is_err());
        }
        git(&dir.0, &["init", "--quiet"]);
        git(&dir.0, &["add", "--force", ".vivac-artifacts"]);
        assert!(download(&dir.0, &saved.object, &saved.digest, saved.size).is_err());
        assert_eq!(objects(&dir.0).unwrap().len(), 1);
    }
    #[test]
    fn source_replaced_after_open_is_rejected_without_published_or_temporary_bytes() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"old delivery").unwrap();
        fs::write(dir.0.join("replacement.txt"), b"new delivery").unwrap();
        let root = open_root(&dir.0).unwrap();
        let opened = regular(&root, "source.txt").unwrap();
        fs::rename(dir.0.join("source.txt"), dir.0.join("original.txt")).unwrap();
        fs::rename(dir.0.join("replacement.txt"), dir.0.join("source.txt")).unwrap();
        let failed = capture_opened(&dir.0, root, "source.txt", REVISION, opened)
            .err()
            .expect("a replaced source must be rejected");
        assert_eq!(failed.code(), 2);
        assert!(failed.message().contains("The artifact source"));
        assert!(objects(&dir.0).unwrap().is_empty());
        assert!(!dir
            .0
            .join(STORE)
            .join(format!("objects/{REVISION}.blob"))
            .exists());
        assert_eq!(fs::read(dir.0.join("source.txt")).unwrap(), b"new delivery");
    }
    #[test]
    fn corruption_and_guard_refusal_publish_no_destination_or_revision() {
        let dir = Scratch::new();
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        fs::write(
            dir.0.join(".vivac-artifacts").join(&saved.object),
            b"corrupted",
        )
        .unwrap();
        assert!(verify(&dir.0, &saved.object, &saved.digest, saved.size).is_err());
        assert!(restore(
            &dir.0,
            &saved.object,
            &saved.digest,
            saved.size,
            "restored.txt"
        )
        .is_err());
        assert!(!dir.0.join("restored.txt").exists());
        fs::write(
            dir.0.join("secret.txt"),
            b"-----BEGIN PRIVATE KEY-----\nsynthetic\n-----END PRIVATE KEY-----",
        )
        .unwrap();
        assert!(capture(&dir.0, "secret.txt", "01arz3ndektsv4rrffq69g5faw").is_err());
        assert_eq!(objects(&dir.0).unwrap(), vec![saved.object]);
    }
    #[test]
    fn invalid_paths_are_refused_and_reads_do_not_create_store() {
        let dir = Scratch::new();
        for path in [
            "../outside.txt",
            "/outside",
            "C:/outside",
            "file:stream",
            "NUL.txt",
            ".vivac/log.jsonl",
            ".vivac-artifacts/object",
            ".git/config",
            ".env",
            "keys/id_rsa",
        ] {
            assert!(inspect(&dir.0, path).is_err(), "{path}");
        }
        assert!(inspect(&dir.0, "absent.txt").unwrap().is_none());
        assert!(objects(&dir.0).unwrap().is_empty());
        assert!(!dir.0.join(".vivac-artifacts").exists());
    }
    #[test]
    fn retained_inspection_is_read_only_and_reports_actual_bytes() {
        let dir = Scratch::new();
        assert!(site_id(&dir.0).unwrap().is_none());
        assert!(
            inspect_retained(&dir.0, &format!("objects/{REVISION}.blob"))
                .unwrap()
                .is_none()
        );
        assert!(!dir.0.join(".vivac-artifacts").exists());
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        assert_eq!(
            site_id(&dir.0).unwrap().as_deref(),
            Some(saved.site.as_str())
        );
        assert_eq!(
            inspect_retained(&dir.0, &saved.object).unwrap(),
            Some((saved.digest, saved.size))
        );
        assert!(inspect_retained(&dir.0, "../outside").is_err());
    }
    #[test]
    fn retained_inspection_rejects_oversized_objects() {
        let dir = Scratch::new();
        fs::create_dir_all(dir.0.join(".vivac-artifacts/objects")).unwrap();
        let file = fs::File::create(
            dir.0
                .join(format!(".vivac-artifacts/objects/{REVISION}.blob")),
        )
        .unwrap();
        file.set_len(10 * 1024 * 1024 + 1).unwrap();
        assert!(inspect_retained(&dir.0, &format!("objects/{REVISION}.blob")).is_err());
    }
    #[test]
    fn object_visit_stops_and_preserves_the_first_callback_failure() {
        let dir = Scratch::new();
        let directory = dir.0.join(STORE).join("objects");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{REVISION}.blob")),
            b"first delivery",
        )
        .unwrap();
        fs::write(
            directory.join("01arz3ndektsv4rrffq69g5faw.blob"),
            b"second delivery",
        )
        .unwrap();
        let mut visits = 0;
        let failure = for_each_object(&dir.0, |path| {
            object_path(&path)?;
            visits += 1;
            Err(Failure::Model("Stop visiting artifact objects.".into()))
        })
        .unwrap_err();
        assert_eq!(visits, 1);
        assert_eq!(failure.message(), "Stop visiting artifact objects.");
    }
    #[test]
    fn object_visit_reports_revisions_and_temporaries_without_creating_missing_store() {
        let dir = Scratch::new();
        for_each_object(&dir.0, |_| panic!("an absent store has no objects")).unwrap();
        assert!(!dir.0.join(STORE).exists());
        let directory = dir.0.join(STORE).join("objects");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("{REVISION}.blob")), b"delivery").unwrap();
        fs::write(directory.join(format!(".tmp-{REVISION}")), b"unfinished").unwrap();
        let mut visited = Vec::new();
        for_each_object(&dir.0, |path| {
            visited.push(path);
            Ok(())
        })
        .unwrap();
        visited.sort();
        assert_eq!(
            visited,
            vec![
                format!("objects/.tmp-{REVISION}"),
                format!("objects/{REVISION}.blob")
            ]
        );
    }
    #[test]
    fn git_tracking_and_ignore_overrides_refuse_publication() {
        let dir = Scratch::new();
        git(&dir.0, &["init", "--quiet"]);
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        fs::create_dir(dir.0.join(".vivac-artifacts")).unwrap();
        fs::write(
            dir.0.join(".vivac-artifacts/.gitignore"),
            b"!objects/\n!objects/*.blob\n",
        )
        .unwrap();
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert!(objects(&dir.0).unwrap().is_empty());
        fs::write(dir.0.join(".vivac-artifacts/.gitignore"), b"*\n").unwrap();
        git(&dir.0, &["add", "-f", ".vivac-artifacts/.gitignore"]);
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert!(objects(&dir.0).unwrap().is_empty());
    }
    #[test]
    fn tracked_store_is_rejected_before_creating_or_editing_its_files() {
        let dir = Scratch::new();
        git(&dir.0, &["init", "--quiet"]);
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        fs::create_dir(dir.0.join(STORE)).unwrap();
        fs::write(dir.0.join(STORE).join("tracked.txt"), b"keep me").unwrap();
        git(&dir.0, &["add", ".vivac-artifacts/tracked.txt"]);
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert!(!dir.0.join(STORE).join(".gitignore").exists());
        assert!(!dir.0.join(STORE).join("objects").exists());
        assert_eq!(
            fs::read(dir.0.join(STORE).join("tracked.txt")).unwrap(),
            b"keep me"
        );
        fs::remove_file(dir.0.join(STORE).join("tracked.txt")).unwrap();
        fs::remove_dir(dir.0.join(STORE)).unwrap();
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert!(!dir.0.join(STORE).exists());
    }
    #[test]
    fn global_ignore_cannot_replace_the_stores_own_wildcard_rule() {
        let dir = Scratch::new();
        let configuration = Scratch::new();
        git(&dir.0, &["init", "--quiet"]);
        let excludes = configuration.0.join("ignore");
        fs::write(&excludes, b"*\n").unwrap();
        git(
            &dir.0,
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        );
        fs::write(dir.0.join("source.txt"), b"safe artifact").unwrap();
        fs::create_dir(dir.0.join(STORE)).unwrap();
        let local_ignore = dir.0.join(STORE).join(".gitignore");
        fs::write(&local_ignore, b"# existing local rules\n").unwrap();
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert_eq!(
            fs::read(&local_ignore).unwrap(),
            b"# existing local rules\n"
        );
        assert!(objects(&dir.0).unwrap().is_empty());
        fs::write(&local_ignore, b"*.blob\n").unwrap();
        assert!(capture(&dir.0, "source.txt", REVISION).is_err());
        assert_eq!(fs::read(&local_ignore).unwrap(), b"*.blob\n");
        assert!(objects(&dir.0).unwrap().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn replacement_with_matching_metadata_is_a_different_source() {
        use std::os::windows::fs::MetadataExt;
        let dir = Scratch::new();
        let source = dir.0.join("source.txt");
        fs::write(&source, b"old bytes").unwrap();
        fs::write(dir.0.join("replacement.txt"), b"new bytes").unwrap();
        let original_metadata = fs::metadata(&source).unwrap();
        let root = open_root(&dir.0).unwrap();
        let original = regular(&root, "source.txt").unwrap();
        fs::rename(&source, dir.0.join("old.txt")).unwrap();
        fs::rename(dir.0.join("replacement.txt"), &source).unwrap();
        let command = format!(
            "$file = Get-Item -LiteralPath '{}'; $file.CreationTimeUtc = [DateTime]::FromFileTimeUtc({}); $file.LastWriteTimeUtc = [DateTime]::FromFileTimeUtc({})",
            source.to_string_lossy().replace('\'', "''"),
            original_metadata.creation_time(), original_metadata.last_write_time()
        );
        assert!(Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &command])
            .output()
            .unwrap()
            .status
            .success());
        let replacement = regular(&root, "source.txt").unwrap();
        unchanged(
            &original.metadata().unwrap(),
            &replacement.metadata().unwrap(),
        )
        .unwrap();
        assert!(same_source(&original, &replacement).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn links_cannot_escape_at_parent_or_leaf() {
        let dir = Scratch::new();
        let outside = Scratch::new();
        fs::write(outside.0.join("source.txt"), b"safe outside").unwrap();
        std::os::unix::fs::symlink(&outside.0, dir.0.join("parent")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("source.txt"), dir.0.join("leaf.txt")).unwrap();
        assert!(inspect(&dir.0, "parent/source.txt").is_err());
        assert!(inspect(&dir.0, "leaf.txt").is_err());
        fs::write(dir.0.join("source.txt"), b"safe selected delivery").unwrap();
        let saved = capture(&dir.0, "source.txt", REVISION).unwrap();
        fs::remove_file(dir.0.join(STORE).join(&saved.object)).unwrap();
        std::os::unix::fs::symlink(
            outside.0.join("source.txt"),
            dir.0.join(STORE).join(&saved.object),
        )
        .unwrap();
        assert!(download(&dir.0, &saved.object, &saved.digest, saved.size).is_err());
        assert!(objects(&dir.0).is_err());
        let entries: Vec<_> = fs::read_dir(dir.0.join(STORE).join("objects"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), format!("{REVISION}.blob").as_str());
        assert!(fs::symlink_metadata(entries[0].path())
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!entries
            .iter()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(".tmp-")));
    }
    #[cfg(windows)]
    #[test]
    fn junction_cannot_escape_at_parent() {
        let dir = Scratch::new();
        let outside = Scratch::new();
        fs::write(outside.0.join("source.txt"), b"safe outside").unwrap();
        let link = dir.0.join("parent");
        let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
        let command = format!(
            "New-Item -ItemType Junction -Path {} -Target {} | Out-Null",
            quote(&link),
            quote(&outside.0)
        );
        assert!(Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &command])
            .output()
            .unwrap()
            .status
            .success());
        assert!(inspect(&dir.0, "parent/source.txt").is_err());
    }
}
