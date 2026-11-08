use super::{Manifest, MAX_MANIFEST_SIZE, PROTECTION};
use crate::artifacts::{files, native};
use crate::device_keys::credential::{Credential, PAYLOAD, PAYLOAD_SIZE};
use crate::device_keys::macos_password;
use crate::device_keys::proof::{Challenge, Proof};
use crate::device_keys::{refused, PublicDeviceKey};
use crate::failure::Failure;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, File, MetadataExt, OpenOptions, OpenOptionsExt};
use core_foundation::base::TCFType;
use core_foundation::data::CFData;
use security_framework::item::{
    ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit, Location, Reference,
    SearchResult,
};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::keychain_item::SecKeychainItem;
use security_framework_sys::base::SecKeychainRef;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::ptr;
use zeroize::{Zeroize, Zeroizing};

const KEYCHAIN: &str = "device.keychain";
const DATABASE: &str = "device.keychain-db";
// Apple derives these `.fl` names from the first four `SHA-1` bytes.
const KEYCHAIN_MARKER: &str = ".fl4895E04B";
const DATABASE_MARKER: &str = ".flCB3D0700";
const MANIFEST: &str = "device-key.json";
const PENDING: &str = ".device-key.pending";
const SERVICE: &str = "org.vivac.device-key.v1";
const MAX_DATABASE_SIZE: u64 = 1024 * 1024;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    #[link_name = "SecKeychainLock"]
    fn keychain_lock(keychain: SecKeychainRef) -> i32;
    #[link_name = "SecKeychainGetStatus"]
    fn keychain_get_status(keychain: SecKeychainRef, status: *mut u32) -> i32;
}

fn native_failure() -> Failure {
    refused("the macOS protected key operation failed.")
}

fn native_status(action: &'static str, status: i32) -> Failure {
    refused(&format!(
        "the macOS keychain {action} failed (OSStatus {status})."
    ))
}

pub(in crate::device_keys) fn create(path: &Path) -> Result<PublicDeviceKey, Failure> {
    let location = StoreLocation::open(path)?;
    location.require_absent()?;
    let password = macos_password::read_confirmed()?;
    location.recheck_parent()?;
    location.require_absent()?;
    disable_native_ui()?;
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    location.parent.create_dir_with(&location.name, &builder)?;
    let dir = location.parent.open_dir_nofollow(&location.name)?;
    let mut guard = Creation {
        location,
        dir,
        committed: false,
        marker: None,
    };
    guard.verify()?;
    let mut payload = Zeroizing::new([0u8; PAYLOAD_SIZE]);
    payload[..PAYLOAD.len()].copy_from_slice(PAYLOAD);
    getrandom::getrandom(&mut payload[PAYLOAD.len()..])
        .map_err(|_| refused("operating-system randomness is unavailable."))?;
    let receipt = Credential::parse(&payload[..])?.receipt(PROTECTION);
    let native_path = keychain_path(&guard.location.path)?;
    let mut keychain = create_keychain(&native_path, password.as_str())?;
    guard.verify()?;
    let database_name = database_member(&guard.dir, &[])?;
    guard.marker = Some(prepare_marker(&guard.dir, database_name)?);
    guard.verify()?;
    // Only a new, unpublished store may tighten the native file's initial mode.
    let database_file = files::regular(&guard.dir, database_name)?;
    files::require_single_link(&database_file)?;
    let metadata = database_file.metadata()?;
    // SAFETY: geteuid has no arguments or borrowed storage.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.len() == 0
        || metadata.len() > MAX_DATABASE_SIZE
    {
        return Err(native_failure());
    }
    database_file.set_permissions(cap_std::fs::Permissions::from_std(
        std::fs::Permissions::from_mode(0o600),
    ))?;
    let mut database = HeldFile::from_file(
        &guard.dir,
        database_name,
        database_file,
        MAX_DATABASE_SIZE,
        false,
    )?;
    database.verify(&guard.dir)?;
    keychain
        .unlock(Some(password.as_str()))
        .map_err(|error| native_status("initial unlock", error.code()))?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    require_unlocked(&keychain)?;
    let mut add = ItemAddOptions::new(ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(&payload[..]),
    });
    add.set_service(SERVICE)
        .set_account_name(&receipt.credential_id)
        .set_location(Location::FileKeychain(keychain.clone()));
    add.add()
        .map_err(|error| native_status("item creation", error.code()))?;
    drop(add);
    database = refresh_created_database(&guard, &database)?;
    database.file.sync_all()?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    database_member(&guard.dir, &[])?;
    verify_receipt(&keychain, &receipt)?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    lock_keychain(&keychain)?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    match keychain.unlock(Some("")) {
        Err(error) if error.code() == -25293 => {}
        Err(error) => return Err(native_status("empty passphrase check", error.code())),
        Ok(()) => return Err(refused("the new keychain accepted an empty passphrase.")),
    }
    require_locked(&keychain)?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    keychain
        .unlock(Some(password.as_str()))
        .map_err(|error| native_status("unlock", error.code()))?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    verify_receipt(&keychain, &receipt)?;
    drop(keychain);
    database.file.sync_all()?;
    guard.verify()?;
    database.verify(&guard.dir)?;
    database_member(&guard.dir, &[])?;
    let database_digest = database.digest(&guard.dir)?;
    let manifest = Manifest::from_receipt(
        &receipt,
        database_digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    let bytes = serde_json::to_vec(&manifest).map_err(|_| native_failure())?;
    Manifest::parse(&bytes)?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    let mut temporary = guard.dir.open_with(PENDING, &options)?;
    native::verify_private_file(&temporary)?;
    files::require_single_link(&temporary)?;
    temporary.write_all(&bytes)?;
    temporary.sync_all()?;
    let mut manifest_file = HeldFile::from_file(
        &guard.dir,
        PENDING,
        temporary,
        MAX_MANIFEST_SIZE as u64,
        true,
    )?;
    if manifest_file.read()? != bytes {
        return Err(native_failure());
    }
    guard.verify()?;
    database.verify(&guard.dir)?;
    manifest_file.verify(&guard.dir)?;
    manifest.matches_database_digest(&database.digest(&guard.dir)?)?;
    database_member(&guard.dir, &[PENDING])?;
    // SAFETY: both fixed C strings and the owned directory FD remain live.
    let renamed = unsafe {
        libc::renameatx_np(
            guard.dir.as_raw_fd(),
            c".device-key.pending".as_ptr(),
            guard.dir.as_raw_fd(),
            c"device-key.json".as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if renamed != 0 {
        return Err(native_failure());
    }
    // The manifest is the commit point, including when later checks fail.
    guard.committed = true;
    manifest_file.name = MANIFEST;
    files::sync_directory(&guard.dir)?;
    files::sync_directory(&guard.location.parent)?;
    guard.verify()?;
    manifest_file.verify(&guard.dir)?;
    database.verify(&guard.dir)?;
    manifest.matches_database_digest(&database.digest(&guard.dir)?)?;
    if manifest_file.read()? != bytes {
        return Err(native_failure());
    }
    database_member(&guard.dir, &[MANIFEST])?;
    Ok(receipt)
}

pub(in crate::device_keys) fn show(path: &Path) -> Result<PublicDeviceKey, Failure> {
    let package = Package::open(path)?;
    disable_native_ui()?;
    let keychain = package.keychain()?;
    let receipt = with_credential(&keychain, &package.manifest.credential_id, |credential| {
        let receipt = credential.receipt(PROTECTION);
        package.manifest.matches_credential(&receipt)?;
        Ok(receipt)
    })?;
    package.verify()?;
    Ok(receipt)
}

pub(in crate::device_keys) fn prove(
    path: &Path,
    challenge: &Challenge,
    now: u64,
) -> Result<Proof, Failure> {
    challenge.validate(now)?;
    let package = Package::open(path)?;
    disable_native_ui()?;
    let keychain = package.keychain()?;
    let proof = with_credential(&keychain, &package.manifest.credential_id, |credential| {
        package
            .manifest
            .matches_credential(&credential.receipt(PROTECTION))?;
        credential.prove(challenge, now)
    })?;
    package.verify()?;
    Ok(proof)
}

pub(in crate::device_keys) fn lock(path: &Path) -> Result<(), Failure> {
    let package = Package::open(path)?;
    disable_native_ui()?;
    let keychain = package.keychain()?;
    lock_keychain(&keychain)?;
    package.verify()
}

pub(in crate::device_keys) fn unlock(path: &Path) -> Result<(), Failure> {
    let package = Package::open(path)?;
    disable_native_ui()?;
    let mut keychain = package.keychain()?;
    let result = (|| {
        if !is_unlocked(&keychain)? {
            let password = macos_password::read_unlock()?;
            package.verify()?;
            keychain
                .unlock(Some(password.as_str()))
                .map_err(|error| native_status("unlock", error.code()))?;
        }
        with_credential(&keychain, &package.manifest.credential_id, |credential| {
            package
                .manifest
                .matches_credential(&credential.receipt(PROTECTION))
        })?;
        package.verify()
    })();
    if result.is_err() {
        let _ = lock_keychain(&keychain);
    }
    result
}

struct StoreLocation {
    path: PathBuf,
    parent: Dir,
    name: String,
}

impl StoreLocation {
    fn open(path: &Path) -> Result<Self, Failure> {
        let text = path
            .to_str()
            .filter(|text| !text.as_bytes().contains(&0))
            .ok_or_else(|| refused("key storage requires an absolute UTF-8 path without NUL."))?;
        if !path.is_absolute() {
            return Err(refused("an explicit absolute directory is required."));
        }
        let mut names = Vec::new();
        for component in Path::new(text).components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let name = name.to_str().ok_or_else(native_failure)?;
                    if name.eq_ignore_ascii_case(".git") || name.eq_ignore_ascii_case(".vivac") {
                        return Err(refused("key storage must stay outside .git and .vivac."));
                    }
                    if name.to_ascii_lowercase().starts_with("login.keychain") {
                        return Err(refused(
                            "private key storage cannot use login.keychain path components.",
                        ));
                    }
                    names.push(name);
                }
                _ => return Err(refused("key storage cannot use parent traversal.")),
            }
        }
        let name = names.pop().ok_or_else(native_failure)?.to_owned();
        let mut parent = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
        reject_git(&parent)?;
        for component in names {
            parent = parent.open_dir_nofollow(component)?;
            reject_git(&parent)?;
        }
        native::verify_private_directory(&parent)?;
        Ok(Self {
            path: path.to_path_buf(),
            parent,
            name,
        })
    }

    fn require_absent(&self) -> Result<(), Failure> {
        match self.parent.symlink_metadata(&self.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(refused("device key creation requires a new directory.")),
        }
    }

    fn recheck_parent(&self) -> Result<(), Failure> {
        let current = Self::open(&self.path)?;
        native::same_directory(&self.parent, &current.parent)?;
        native::verify_private_directory(&self.parent)
    }

    fn verify(&self, directory: &Dir) -> Result<(), Failure> {
        self.recheck_parent()?;
        native::verify_private_directory(directory)?;
        reject_git(directory)?;
        native::same_directory(directory, &self.parent.open_dir_nofollow(&self.name)?)
    }
}

fn reject_git(directory: &Dir) -> Result<(), Failure> {
    match directory.symlink_metadata(".git") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(refused("key storage must stay outside Git working trees.")),
    }
}

struct Creation {
    location: StoreLocation,
    dir: Dir,
    committed: bool,
    marker: Option<HeldFile>,
}

impl Creation {
    fn verify(&self) -> Result<(), Failure> {
        self.location.verify(&self.dir)?;
        if let Some(marker) = &self.marker {
            marker.verify(&self.dir)?;
        }
        Ok(())
    }
}

impl Drop for Creation {
    fn drop(&mut self) {
        if !self.committed
            && self.location.recheck_parent().is_ok()
            && self
                .location
                .parent
                .open_dir_nofollow(&self.location.name)
                .and_then(|current| {
                    native::same_directory(&self.dir, &current)
                        .map_err(|_| std::io::Error::other("directory identity changed"))
                })
                .is_ok()
        {
            // Capability cleanup never follows links and only owns this identity.
            let _ = self.location.parent.remove_dir_all(&self.location.name);
        }
    }
}

struct HeldFile {
    name: &'static str,
    file: File,
    identity: (u64, u64),
    size: u64,
    maximum: u64,
    fixed_size: bool,
}

impl HeldFile {
    fn open(
        directory: &Dir,
        name: &'static str,
        maximum: u64,
        fixed_size: bool,
    ) -> Result<Self, Failure> {
        let file = files::regular(directory, name)?;
        Self::from_file(directory, name, file, maximum, fixed_size)
    }

    fn from_file(
        directory: &Dir,
        name: &'static str,
        file: File,
        maximum: u64,
        fixed_size: bool,
    ) -> Result<Self, Failure> {
        let metadata = file.metadata()?;
        let held = Self {
            name,
            identity: (metadata.dev(), metadata.ino()),
            size: metadata.len(),
            file,
            maximum,
            fixed_size,
        };
        held.verify(directory)?;
        Ok(held)
    }

    fn verify(&self, directory: &Dir) -> Result<(), Failure> {
        for file in [&self.file, &files::regular(directory, self.name)?] {
            native::verify_private_file(file)?;
            files::require_single_link(file)?;
            let metadata = file.metadata()?;
            if (metadata.dev(), metadata.ino()) != self.identity
                || (metadata.len() == 0 && self.maximum != 0)
                || metadata.len() > self.maximum
                || (self.fixed_size && metadata.len() != self.size)
            {
                return Err(refused(
                    "the protected key package changed or has an invalid size.",
                ));
            }
        }
        Ok(())
    }

    fn read(&self) -> Result<Vec<u8>, Failure> {
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(self.size as usize);
        file.take(self.maximum + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.size {
            return Err(native_failure());
        }
        Ok(bytes)
    }

    fn digest(&self, directory: &Dir) -> Result<[u8; 32], Failure> {
        self.verify(directory)?;
        let before = self.file.metadata()?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut input = file.take(self.maximum + 1);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        let mut size = 0u64;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > self.maximum {
                return Err(native_failure());
            }
            hasher.update(&buffer[..count]);
        }
        self.verify(directory)?;
        let after = self.file.metadata()?;
        if size != before.len()
            || before.len() != after.len()
            || before.modified()? != after.modified()?
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(refused(
                "the protected key database changed while being read.",
            ));
        }
        Ok(hasher.finalize().into())
    }
}

fn refresh_created_database(guard: &Creation, previous: &HeldFile) -> Result<HeldFile, Failure> {
    guard.verify()?;
    if guard.committed
        || guard.marker.is_none()
        || database_member(&guard.dir, &[])? != previous.name
    {
        return Err(native_failure());
    }
    let current = HeldFile::open(&guard.dir, previous.name, MAX_DATABASE_SIZE, false)?;
    if current.identity == previous.identity {
        previous.verify(&guard.dir)?;
    } else {
        // Native item creation commits by replacing the database. Only this
        // creation seam may advance the held identity after the old file lost
        // its last link; ordinary readers retain their original identity.
        native::verify_private_file(&previous.file)?;
        let metadata = previous.file.metadata()?;
        if metadata.nlink() != 0
            || (metadata.dev(), metadata.ino()) != previous.identity
            || metadata.len() == 0
            || metadata.len() > MAX_DATABASE_SIZE
        {
            return Err(native_failure());
        }
    }
    guard.verify()?;
    if database_member(&guard.dir, &[])? != previous.name {
        return Err(native_failure());
    }
    current.verify(&guard.dir)?;
    Ok(current)
}

struct Package {
    location: StoreLocation,
    dir: Dir,
    manifest: Manifest,
    manifest_file: HeldFile,
    manifest_bytes: Vec<u8>,
    database: HeldFile,
    marker: HeldFile,
}

impl Package {
    fn open(path: &Path) -> Result<Self, Failure> {
        let location = StoreLocation::open(path)?;
        let dir = location.parent.open_dir_nofollow(&location.name)?;
        location.verify(&dir)?;
        // Missing commit refuses before any native call or password prompt.
        let manifest_file = HeldFile::open(&dir, MANIFEST, MAX_MANIFEST_SIZE as u64, true)?;
        let manifest_bytes = manifest_file.read()?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        let database_name = database_member(&dir, &[MANIFEST])?;
        let database = HeldFile::open(&dir, database_name, MAX_DATABASE_SIZE, false)?;
        let marker = HeldFile::open(&dir, marker_name(database_name)?, 0, true)?;
        let package = Self {
            location,
            dir,
            manifest,
            manifest_file,
            manifest_bytes,
            database,
            marker,
        };
        package.verify()?;
        Ok(package)
    }

    fn verify(&self) -> Result<(), Failure> {
        self.location.verify(&self.dir)?;
        self.manifest_file.verify(&self.dir)?;
        self.database.verify(&self.dir)?;
        self.marker.verify(&self.dir)?;
        if database_member(&self.dir, &[MANIFEST])? != self.database.name
            || self.manifest_file.read()? != self.manifest_bytes
        {
            return Err(native_failure());
        }
        self.manifest
            .matches_database_digest(&self.database.digest(&self.dir)?)?;
        Ok(())
    }

    fn keychain(&self) -> Result<SecKeychain, Failure> {
        self.verify()?;
        let keychain = SecKeychain::open(self.location.path.join(self.database.name))
            .map_err(|error| native_status("open", error.code()))?;
        self.verify()?;
        Ok(keychain)
    }
}

fn database_member(directory: &Dir, others: &[&str]) -> Result<&'static str, Failure> {
    let mut database = None;
    let mut marker = None;
    let mut seen = 0;
    for entry in directory.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(native_failure)?;
        match name {
            KEYCHAIN | DATABASE if database.is_none() => {
                database = Some(if name == KEYCHAIN { KEYCHAIN } else { DATABASE });
            }
            KEYCHAIN_MARKER | DATABASE_MARKER if marker.is_none() => {
                marker = Some(if name == KEYCHAIN_MARKER {
                    KEYCHAIN_MARKER
                } else {
                    DATABASE_MARKER
                });
            }
            _ if others.contains(&name) => seen += 1,
            _ => return Err(refused("the protected key package has unexpected members.")),
        }
    }
    if seen != others.len() {
        return Err(native_failure());
    }
    let database = database.ok_or_else(native_failure)?;
    if marker != Some(marker_name(database)?) {
        return Err(refused(
            "the protected key package is missing its matching lock marker.",
        ));
    }
    Ok(database)
}

fn marker_name(database: &str) -> Result<&'static str, Failure> {
    match database {
        KEYCHAIN => Ok(KEYCHAIN_MARKER),
        DATABASE => Ok(DATABASE_MARKER),
        _ => Err(native_failure()),
    }
}

fn prepare_marker(directory: &Dir, database: &str) -> Result<HeldFile, Failure> {
    let name = marker_name(database)?;
    let file = files::regular(directory, name)?;
    files::require_single_link(&file)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or borrowed storage.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.len() != 0 {
        return Err(native_failure());
    }
    // Only a fresh store may remove the native marker's group/other access.
    file.set_permissions(cap_std::fs::Permissions::from_std(
        std::fs::Permissions::from_mode(0o400),
    ))?;
    file.sync_all()?;
    HeldFile::from_file(directory, name, file, 0, true)
}

fn keychain_path(path: &Path) -> Result<CString, Failure> {
    CString::new(path.join(KEYCHAIN).to_str().ok_or_else(native_failure)?)
        .map_err(|_| native_failure())
}

fn disable_native_ui() -> Result<(), Failure> {
    // SAFETY: the native API receives only a false Boolean, with no buffers.
    let status =
        unsafe { security_framework_sys::keychain::SecKeychainSetUserInteractionAllowed(0) };
    if status != 0 {
        return Err(native_status("UI policy", status));
    }
    Ok(())
}

fn create_keychain(path: &CString, password: &str) -> Result<SecKeychain, Failure> {
    let mut keychain = ptr::null_mut();
    // SAFETY: path and zeroizing password storage remain borrowed throughout;
    // a successful create transfers exactly one retained, non-null CF owner.
    let status = unsafe {
        security_framework_sys::keychain::SecKeychainCreate(
            path.as_ptr(),
            password.len() as u32,
            password.as_ptr().cast(),
            0,
            ptr::null_mut(),
            &mut keychain,
        )
    };
    if status != 0 {
        return Err(native_status("creation", status));
    }
    if keychain.is_null() {
        return Err(native_failure());
    }
    // SAFETY: a successful create with non-null output transfers one owner.
    let keychain = unsafe { SecKeychain::wrap_under_create_rule(keychain) };
    Ok(keychain)
}

fn is_unlocked(keychain: &SecKeychain) -> Result<bool, Failure> {
    let mut status = 0u32;
    let reference = keychain.as_concrete_TypeRef();
    if reference.is_null() {
        return Err(native_failure());
    }
    // SAFETY: the non-null CF object remains retained; status is initialized
    // writable storage and this call cannot select a default keychain.
    let result = unsafe { keychain_get_status(reference, &mut status) };
    if result != 0 {
        return Err(native_status("status query", result));
    }
    Ok(status & 1 != 0)
}

fn require_unlocked(keychain: &SecKeychain) -> Result<(), Failure> {
    if !is_unlocked(keychain)? {
        return Err(refused(
            "the device key store is locked; unlock it explicitly.",
        ));
    }
    Ok(())
}

fn require_locked(keychain: &SecKeychain) -> Result<(), Failure> {
    if is_unlocked(keychain)? {
        return Err(native_failure());
    }
    Ok(())
}

fn lock_keychain(keychain: &SecKeychain) -> Result<(), Failure> {
    let reference = keychain.as_concrete_TypeRef();
    if reference.is_null() {
        return Err(native_failure());
    }
    // SAFETY: the non-null CF object remains retained and explicitly selects
    // only this keychain, never all keychains or the default one.
    let result = unsafe { keychain_lock(reference) };
    if result != 0 {
        return Err(native_status("lock", result));
    }
    require_locked(keychain)
}

struct SearchResults(Vec<SearchResult>);

impl Drop for SearchResults {
    fn drop(&mut self) {
        for result in &mut self.0 {
            if let SearchResult::Data(bytes) = result {
                bytes.zeroize();
            }
        }
    }
}

fn search(keychain: &SecKeychain, account: &str, data: bool) -> Result<SearchResults, Failure> {
    let mut options = ItemSearchOptions::new();
    options
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(account)
        .load_refs(!data)
        .load_data(data)
        .limit(if data { Limit::Max(1) } else { Limit::All });
    options
        .search()
        .map(SearchResults)
        .map_err(|error| native_status("lookup", error.code()))
}

fn single_reference(results: &SearchResults) -> Result<&SecKeychainItem, Failure> {
    match results.0.as_slice() {
        [SearchResult::Ref(Reference::KeychainItem(item))] => Ok(item),
        _ => Err(refused("the protected key item is missing or ambiguous.")),
    }
}

fn with_credential<R>(
    keychain: &SecKeychain,
    account: &str,
    use_key: impl FnOnce(&Credential<'_>) -> Result<R, Failure>,
) -> Result<R, Failure> {
    require_unlocked(keychain)?;
    let before = search(keychain, account, false)?;
    let reference = single_reference(&before)?;
    let data = search(keychain, account, true)?;
    let bytes = match data.0.as_slice() {
        [SearchResult::Data(bytes)] => bytes,
        _ => return Err(native_failure()),
    };
    let after = search(keychain, account, false)?;
    if reference.as_CFType() != single_reference(&after)?.as_CFType() {
        return Err(refused(
            "the protected key item changed during the operation.",
        ));
    }
    require_unlocked(keychain)?;
    let credential = Credential::parse(bytes)?;
    let result = use_key(&credential)?;
    require_unlocked(keychain)?;
    Ok(result)
}

fn verify_receipt(keychain: &SecKeychain, receipt: &PublicDeviceKey) -> Result<(), Failure> {
    with_credential(keychain, &receipt.credential_id, |credential| {
        if credential.receipt(PROTECTION) != *receipt {
            return Err(refused(
                "the protected key does not match its generated identity.",
            ));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt as _;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("vivac-macos-store-{}", crate::id::ulid()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }

        fn creation(&self, name: &str) -> Creation {
            let location = StoreLocation::open(&self.0.join(name)).unwrap();
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            location.parent.create_dir_with(name, &builder).unwrap();
            let dir = location.parent.open_dir_nofollow(name).unwrap();
            Creation {
                location,
                dir,
                committed: false,
                marker: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            let path = self.0.canonicalize().unwrap();
            assert!(path.starts_with(&temporary) && path != temporary);
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn preflight_refuses_existing_destinations_git_and_linked_ancestors() {
        let fixture = Fixture::new();
        let existing = fixture.creation("existing");
        assert!(create(&fixture.0.join("existing")).is_err());
        for forbidden in [".git", ".GiT", ".vivac", ".ViVaC"] {
            assert!(StoreLocation::open(&fixture.0.join(forbidden).join("key")).is_err());
        }
        assert!(StoreLocation::open(&fixture.0.join("../key")).is_err());
        assert!(StoreLocation::open(Path::new("relative/key")).is_err());
        std::os::unix::fs::symlink(&fixture.0, fixture.0.join("alias")).unwrap();
        assert!(StoreLocation::open(&fixture.0.join("alias/key")).is_err());
        std::fs::write(fixture.0.join(".git"), b"synthetic Git marker").unwrap();
        assert!(StoreLocation::open(&fixture.0.join("new")).is_err());
        std::fs::remove_file(fixture.0.join(".git")).unwrap();
        drop(existing);
    }

    #[test]
    fn missing_commit_refuses_every_reader_before_native_or_tty() {
        let fixture = Fixture::new();
        let stage = fixture.creation("incomplete");
        let path = fixture.0.join("incomplete");
        assert!(Package::open(&path).is_err());
        assert!(show(&path).is_err());
        assert!(lock(&path).is_err());
        assert!(unlock(&path).is_err());
        assert_eq!(stage.dir.entries().unwrap().count(), 0);
    }

    #[test]
    fn cleanup_preserves_commit_and_replaced_identity_without_following_links() {
        let fixture = Fixture::new();
        let stage = fixture.creation("temporary");
        std::os::unix::fs::symlink(&fixture.0, fixture.0.join("temporary/link")).unwrap();
        drop(stage);
        assert!(!fixture.0.join("temporary").exists());
        assert!(fixture.0.exists());

        let mut committed = fixture.creation("committed");
        committed.committed = true;
        drop(committed);
        assert!(fixture.0.join("committed").is_dir());

        let replaced = fixture.creation("replaced");
        std::fs::rename(fixture.0.join("replaced"), fixture.0.join("original")).unwrap();
        std::fs::create_dir(fixture.0.join("replaced")).unwrap();
        std::fs::write(fixture.0.join("replaced/kept"), b"replacement").unwrap();
        drop(replaced);
        assert_eq!(
            std::fs::read(fixture.0.join("replaced/kept")).unwrap(),
            b"replacement"
        );
        assert!(fixture.0.join("original").is_dir());
    }

    #[test]
    fn retained_files_reject_links_replacement_and_permission_changes() {
        let fixture = Fixture::new();
        let stage = fixture.creation("store");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = stage.dir.open_with(MANIFEST, &options).unwrap();
        file.write_all(b"synthetic public manifest").unwrap();
        drop(file);
        let held = HeldFile::open(&stage.dir, MANIFEST, 2048, true).unwrap();
        std::fs::hard_link(
            fixture.0.join("store").join(MANIFEST),
            fixture.0.join("extra"),
        )
        .unwrap();
        assert!(held.verify(&stage.dir).is_err());
        std::fs::remove_file(fixture.0.join("extra")).unwrap();
        held.verify(&stage.dir).unwrap();
        held.file
            .set_permissions(cap_std::fs::Permissions::from_std(
                std::fs::Permissions::from_mode(0o644),
            ))
            .unwrap();
        assert!(held.verify(&stage.dir).is_err());
        held.file
            .set_permissions(cap_std::fs::Permissions::from_std(
                std::fs::Permissions::from_mode(0o600),
            ))
            .unwrap();
        std::fs::rename(
            fixture.0.join("store").join(MANIFEST),
            fixture.0.join("original"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            fixture.0.join("original"),
            fixture.0.join("store").join(MANIFEST),
        )
        .unwrap();
        assert!(held.verify(&stage.dir).is_err());
    }

    #[test]
    fn database_members_require_the_exact_paired_atomic_marker() {
        assert_eq!(marker_name(KEYCHAIN).unwrap(), ".fl4895E04B");
        assert_eq!(marker_name(DATABASE).unwrap(), ".flCB3D0700");
        for database in [KEYCHAIN, DATABASE] {
            let fixture = Fixture::new();
            let stage = fixture.creation("store");
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut file = stage.dir.open_with(database, &options).unwrap();
            file.write_all(b"synthetic encrypted database").unwrap();
            assert!(database_member(&stage.dir, &[]).is_err());
            let marker = marker_name(database).unwrap();
            stage.dir.open_with(marker, &options).unwrap();
            assert_eq!(database_member(&stage.dir, &[]).unwrap(), database);
            let other_database = if database == KEYCHAIN {
                DATABASE
            } else {
                KEYCHAIN
            };
            let other_marker = marker_name(other_database).unwrap();
            stage.dir.remove_file(marker).unwrap();
            stage.dir.open_with(other_marker, &options).unwrap();
            assert!(database_member(&stage.dir, &[]).is_err());
            stage.dir.open_with(marker, &options).unwrap();
            assert!(database_member(&stage.dir, &[]).is_err());
            stage.dir.remove_file(other_marker).unwrap();
            stage.dir.open_with(".fl00000000", &options).unwrap();
            assert!(database_member(&stage.dir, &[]).is_err());
            stage.dir.remove_file(".fl00000000").unwrap();
            stage.dir.open_with(other_database, &options).unwrap();
            assert!(database_member(&stage.dir, &[]).is_err());
        }
    }

    #[test]
    fn empty_marker_profile_does_not_allow_empty_databases_or_manifests() {
        let fixture = Fixture::new();
        let stage = fixture.creation("store");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        for (name, maximum) in [
            (DATABASE, MAX_DATABASE_SIZE),
            (MANIFEST, MAX_MANIFEST_SIZE as u64),
        ] {
            stage.dir.open_with(name, &options).unwrap();
            assert!(HeldFile::open(&stage.dir, name, maximum, true).is_err());
        }
        let marker = marker_name(DATABASE).unwrap();
        let mut file = stage.dir.open_with(marker, &options).unwrap();
        let held = HeldFile::open(&stage.dir, marker, 0, true).unwrap();
        held.verify(&stage.dir).unwrap();
        file.write_all(b"x").unwrap();
        assert!(held.verify(&stage.dir).is_err());
        assert!(HeldFile::open(&stage.dir, marker, 0, true).is_err());
        file.set_len(0).unwrap();
        held.verify(&stage.dir).unwrap();
        stage.dir.remove_file(marker).unwrap();
        stage.dir.open_with(marker, &options).unwrap();
        assert!(held.verify(&stage.dir).is_err());
    }

    #[test]
    fn fresh_marker_tightening_requires_empty_single_link_and_retains_identity() {
        let fixture = Fixture::new();
        let mut stage = fixture.creation("store");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o444);
        let mut file = stage.dir.open_with(DATABASE_MARKER, &options).unwrap();
        file.set_permissions(cap_std::fs::Permissions::from_std(
            std::fs::Permissions::from_mode(0o444),
        ))
        .unwrap();
        file.write_all(b"unexpected marker content").unwrap();
        assert!(prepare_marker(&stage.dir, DATABASE).is_err());
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o444);
        file.set_len(0).unwrap();
        let extra = fixture.0.join("extra");
        std::fs::hard_link(fixture.0.join("store").join(DATABASE_MARKER), &extra).unwrap();
        assert!(prepare_marker(&stage.dir, DATABASE).is_err());
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o444);
        std::fs::remove_file(extra).unwrap();
        let marker = prepare_marker(&stage.dir, DATABASE).unwrap();
        assert_eq!(marker.file.metadata().unwrap().mode() & 0o777, 0o400);
        stage.marker = Some(marker);
        stage.verify().unwrap();
        stage.dir.remove_file(DATABASE_MARKER).unwrap();
        stage.dir.open_with(DATABASE_MARKER, &options).unwrap();
        assert!(stage.verify().is_err());
    }

    fn retained_creation_database(fixture: &Fixture) -> (Creation, HeldFile) {
        let mut stage = fixture.creation("store");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = stage.dir.open_with(DATABASE, &options).unwrap();
        file.write_all(b"original encrypted bytes").unwrap();
        let database =
            HeldFile::from_file(&stage.dir, DATABASE, file, MAX_DATABASE_SIZE, false).unwrap();
        stage.dir.open_with(DATABASE_MARKER, &options).unwrap();
        stage.marker = Some(prepare_marker(&stage.dir, DATABASE).unwrap());
        (stage, database)
    }

    fn replace_creation_database(stage: &Creation, bytes: &[u8], mode: u32) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(mode);
        let mut file = stage.dir.open_with("replacement", &options).unwrap();
        file.set_permissions(cap_std::fs::Permissions::from_std(
            std::fs::Permissions::from_mode(mode),
        ))
        .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        stage
            .dir
            .rename("replacement", &stage.dir, DATABASE)
            .unwrap();
    }

    #[test]
    fn creation_accepts_atomic_commit_only_after_original_database_is_unlinked() {
        let fixture = Fixture::new();
        let (mut stage, previous) = retained_creation_database(&fixture);
        let unchanged = refresh_created_database(&stage, &previous).unwrap();
        assert_eq!(unchanged.identity, previous.identity);
        replace_creation_database(&stage, b"updated encrypted bytes", 0o600);
        assert_eq!(previous.file.metadata().unwrap().nlink(), 0);
        assert!(previous.verify(&stage.dir).is_err());
        let current = refresh_created_database(&stage, &previous).unwrap();
        assert_ne!(current.identity, previous.identity);
        current.verify(&stage.dir).unwrap();
        stage.committed = true;
        assert!(refresh_created_database(&stage, &current).is_err());
    }

    #[test]
    fn creation_refuses_database_replacement_while_original_keeps_a_link() {
        let fixture = Fixture::new();
        let (stage, previous) = retained_creation_database(&fixture);
        std::fs::rename(
            fixture.0.join("store").join(DATABASE),
            fixture.0.join("original"),
        )
        .unwrap();
        replace_creation_database(&stage, b"updated encrypted bytes", 0o600);
        assert_eq!(previous.file.metadata().unwrap().nlink(), 1);
        assert!(refresh_created_database(&stage, &previous).is_err());
    }

    #[test]
    fn creation_refuses_replacement_with_shared_mode_empty_bytes_or_extra_links() {
        for (mode, bytes, linked) in [
            (0o644, b"updated encrypted bytes".as_slice(), false),
            (0o600, b"".as_slice(), false),
            (0o600, b"updated encrypted bytes".as_slice(), true),
        ] {
            let fixture = Fixture::new();
            let (stage, previous) = retained_creation_database(&fixture);
            replace_creation_database(&stage, bytes, mode);
            if linked {
                std::fs::hard_link(
                    fixture.0.join("store").join(DATABASE),
                    fixture.0.join("extra"),
                )
                .unwrap();
            }
            assert_eq!(previous.file.metadata().unwrap().nlink(), 0);
            assert!(refresh_created_database(&stage, &previous).is_err());
        }
    }

    #[test]
    fn opened_package_rejects_same_size_database_corruption() {
        let fixture = Fixture::new();
        let stage = fixture.creation("store");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut database = stage.dir.open_with(DATABASE, &options).unwrap();
        database.write_all(b"abc").unwrap();
        database.sync_all().unwrap();
        stage.dir.open_with(DATABASE_MARKER, &options).unwrap();
        let manifest = serde_json::json!({
            "version": 1,
            "device_id": "00000000000000000000000003",
            "credential_id": "00000000000000000000000004",
            "public_key": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "database_digest": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        });
        let mut file = stage.dir.open_with(MANIFEST, &options).unwrap();
        file.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        let package = Package::open(&fixture.0.join("store")).unwrap();
        package.verify().unwrap();
        database.seek(SeekFrom::Start(0)).unwrap();
        database.write_all(b"abd").unwrap();
        database.sync_all().unwrap();
        assert!(package.verify().is_err());
        assert!(Package::open(&fixture.0.join("store")).is_err());
        assert_eq!(stage.dir.entries().unwrap().count(), 3);
    }
}
