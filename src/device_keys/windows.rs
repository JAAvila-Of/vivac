use super::credential::{Credential, PAYLOAD, PAYLOAD_SIZE};
use super::{refused, PublicDeviceKey};
use crate::artifacts::{files, native};
use crate::failure::Failure;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions, OpenOptionsExt};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, Prefix};
use std::ptr;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::{Zeroize, Zeroizing};

const WRAPPER: &[u8] = b"vivac.dpapi-key.v1\0";
const MAX_FILE_SIZE: usize = 16 * 1024;
const FILE_NAME: &str = "device-key.bin";

pub(super) fn create(path: &Path) -> Result<PublicDeviceKey, Failure> {
    create_with_entropy(path, |bytes| {
        getrandom::getrandom(bytes)
            .map_err(|_| refused("operating-system randomness is unavailable."))
    })
}

fn create_with_entropy(
    path: &Path,
    entropy: impl FnOnce(&mut [u8]) -> Result<(), Failure>,
) -> Result<PublicDeviceKey, Failure> {
    validate_location(path)?;
    let parent_path = path
        .parent()
        .ok_or_else(|| refused("a parent directory is required."))?;
    let (parent, name) = crate::backup::destination(path)?;
    let mut payload = Zeroizing::new([0u8; PAYLOAD_SIZE]);
    payload[..PAYLOAD.len()].copy_from_slice(PAYLOAD);
    entropy(&mut payload[PAYLOAD.len()..])?;
    let receipt = public_identity(&payload[..])?;
    let protected = protect(&payload[..], WRAPPER)?;
    let wrapped = wrap(&protected)?;
    recheck_parent(parent_path, &parent)?;
    let mut preparation = native::Preparation::new(&parent)?;
    native::verify_private_directory(&preparation.dir)?;
    let mut file = native::create_private_file(&preparation.dir, FILE_NAME)?;
    let identity = file_identity(&file)?;
    native::verify_private_file(&file)?;
    file.write_all(&wrapped)?;
    file.sync_all()?;
    verify_file(&file, identity, wrapped.len() as u64)?;
    drop(file);
    let mut file = open_key(&preparation.dir)?;
    verify_file(&file, identity, wrapped.len() as u64)?;
    verify_contents(&mut file, &wrapped)?;
    native::sync_preparation(&preparation.dir)?;
    recheck_parent(parent_path, &parent)?;
    native::verify_private_directory(&preparation.dir)?;
    verify_file(&file, identity, wrapped.len() as u64)?;
    verify_file(&open_key(&preparation.dir)?, identity, wrapped.len() as u64)?;
    verify_members(&preparation.dir)?;
    drop(file);
    preparation.publish(&parent, &name)?;
    let mut file = open_key(&preparation.dir)?;
    verify_file(&file, identity, wrapped.len() as u64)?;
    verify_contents(&mut file, &wrapped)?;
    verify_file(&open_key(&preparation.dir)?, identity, wrapped.len() as u64)?;
    recheck_parent(parent_path, &parent)?;
    verify_file(&file, identity, wrapped.len() as u64)?;
    native::verify_private_directory(&preparation.dir)?;
    verify_members(&preparation.dir)?;
    native::verify_named_directory(&parent, &name, &preparation.dir)?;
    Ok(receipt)
}

pub(super) fn show(path: &Path) -> Result<PublicDeviceKey, Failure> {
    load(path, |credential| {
        Ok(credential.receipt("windows_current_user_dpapi"))
    })
}

fn load<R>(
    path: &Path,
    use_key: impl FnOnce(&Credential<'_>) -> Result<R, Failure>,
) -> Result<R, Failure> {
    validate_location(path)?;
    check_ancestry(&path.canonicalize()?)?;
    let parent_path = path
        .parent()
        .ok_or_else(|| refused("a parent directory is required."))?;
    let name = path
        .file_name()
        .ok_or_else(|| refused("a directory name is required."))?;
    let parent = files::open_root(parent_path)?;
    let directory = parent.open_dir_nofollow(name)?;
    native::verify_private_directory(&directory)?;
    verify_members(&directory)?;
    let mut file = open_key(&directory)?;
    let size = file.metadata()?.len();
    if size > MAX_FILE_SIZE as u64 {
        return Err(refused("the protected key exceeds its size limit."));
    }
    let identity = file_identity(&file)?;
    verify_file(&file, identity, size)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(size as usize));
    (&mut file)
        .take(MAX_FILE_SIZE as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != size as usize {
        return Err(refused("the protected key changed while being read."));
    }
    let protected = unwrap(&bytes)?;
    let payload = unprotect(protected, WRAPPER)?;
    let credential = Credential::parse(&payload)?;
    let result = use_key(&credential)?;
    verify_file(&file, identity, size)?;
    let current = open_key(&directory)?;
    verify_file(&current, identity, size)?;
    verify_members(&directory)?;
    native::verify_private_directory(&directory)?;
    recheck_parent(parent_path, &parent)?;
    native::same_directory(&directory, &parent.open_dir_nofollow(name)?)?;
    Ok(result)
}

pub(super) fn prove(
    path: &Path,
    challenge: &super::proof::Challenge,
    now: u64,
) -> Result<super::proof::Proof, Failure> {
    challenge.validate(now)?;
    load(path, |credential| credential.prove(challenge, now))
}

fn validate_location(path: &Path) -> Result<(), Failure> {
    if !path.is_absolute() {
        return Err(refused("an explicit absolute directory is required."));
    }
    let drive = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) => drive,
            _ => return Err(refused("key storage requires a local drive path.")),
        },
        _ => return Err(refused("key storage requires a local drive path.")),
    };
    let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: the drive root is a retained, terminated UTF-16 string.
    let drive_type =
        unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
    if !local_drive_type(drive_type) {
        return Err(refused("key storage requires an available local drive."));
    }
    let device = [u16::from(drive), u16::from(b':'), 0];
    let mut target = [0u16; 1024];
    // SAFETY: both UTF-16 buffers are retained and the output length is exact.
    let count = unsafe {
        windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW(
            device.as_ptr(),
            target.as_mut_ptr(),
            target.len() as u32,
        )
    };
    if count == 0 {
        return Err(refused("the local drive mapping could not be verified."));
    }
    let end = target
        .iter()
        .position(|value| *value == 0)
        .ok_or_else(|| refused("the local drive mapping is invalid."))?;
    let target = String::from_utf16(&target[..end])
        .map_err(|_| refused("the local drive mapping is invalid."))?;
    if !native_drive_target(&target) {
        return Err(refused("key storage cannot use a substituted drive alias."));
    }
    check_ancestry(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| refused("a parent directory is required."))?;
    check_ancestry(&parent.canonicalize()?)
}

fn local_drive_type(kind: u32) -> bool {
    matches!(kind, 2 | 3 | 6)
}

fn native_drive_target(target: &str) -> bool {
    target.starts_with(r"\Device\")
}

fn check_ancestry(path: &Path) -> Result<(), Failure> {
    if path.components().any(|part| match part {
        Component::Normal(value) => {
            let value = value.to_string_lossy();
            let value = value.trim_end_matches(['.', ' ']);
            value.eq_ignore_ascii_case(".vivac") || value.eq_ignore_ascii_case(".git")
        }
        Component::ParentDir => true,
        _ => false,
    }) {
        return Err(refused(
            "key storage must stay outside .git and .vivac directories and parent traversal.",
        ));
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(refused(
                        "key storage cannot traverse links or reparse points.",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Err(refused("key storage must stay outside Git working trees.")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn recheck_parent(path: &Path, retained: &Dir) -> Result<(), Failure> {
    check_ancestry(path)?;
    check_ancestry(&path.canonicalize()?)?;
    native::same_directory(retained, &files::open_root(path)?)
}

fn verify_members(directory: &Dir) -> Result<(), Failure> {
    let mut entries = directory.entries()?;
    let entry = entries
        .next()
        .ok_or_else(|| refused("the protected key is missing."))??;
    if entry.file_name() != FILE_NAME || entries.next().is_some() {
        return Err(refused("the key directory contains unexpected members."));
    }
    Ok(())
}

fn open_key(directory: &Dir) -> Result<File, Failure> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
    let checked = files::regular(directory, FILE_NAME)?;
    let identity = file_identity(&checked)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .follow(FollowSymlinks::No);
    let file = directory.open_with(FILE_NAME, &options)?;
    verify_file(&file, identity, checked.metadata()?.len())?;
    Ok(file)
}

fn verify_contents(file: &mut File, expected: &[u8]) -> Result<(), Failure> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(expected.len()));
    file.take(MAX_FILE_SIZE as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.as_slice() != expected {
        return Err(refused(
            "the protected key bytes changed during the operation.",
        ));
    }
    Ok(())
}

fn verify_file(file: &File, identity: (u64, [u8; 16]), size: u64) -> Result<(), Failure> {
    let metadata = file.metadata()?;
    files::reject_link(&metadata)?;
    files::require_single_link(file)?;
    native::verify_private_file(file)?;
    if !metadata.is_file() || metadata.len() != size || file_identity(file)? != identity {
        return Err(refused(
            "the protected key file changed during the operation.",
        ));
    }
    Ok(())
}

fn file_identity(file: &File) -> Result<(u64, [u8; 16]), Failure> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut information = FILE_ID_INFO::default();
    // SAFETY: the owned handle and correctly sized output buffer remain live.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut information as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((
        information.VolumeSerialNumber,
        information.FileId.Identifier,
    ))
}

fn public_identity(payload: &[u8]) -> Result<PublicDeviceKey, Failure> {
    Ok(Credential::parse(payload)?.receipt("windows_current_user_dpapi"))
}

fn wrap(protected: &[u8]) -> Result<Zeroizing<Vec<u8>>, Failure> {
    if protected.is_empty() || WRAPPER.len() + 4 + protected.len() > MAX_FILE_SIZE {
        return Err(refused("the protected key exceeds its size limit."));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(WRAPPER.len() + 4 + protected.len()));
    bytes.extend_from_slice(WRAPPER);
    bytes.extend_from_slice(&(protected.len() as u32).to_be_bytes());
    bytes.extend_from_slice(protected);
    Ok(bytes)
}

fn unwrap(bytes: &[u8]) -> Result<&[u8], Failure> {
    if bytes.len() > MAX_FILE_SIZE || bytes.len() < WRAPPER.len() + 4 || !bytes.starts_with(WRAPPER)
    {
        return Err(refused(
            "the protected key wrapper has an unsupported size or version.",
        ));
    }
    let size =
        u32::from_be_bytes(bytes[WRAPPER.len()..WRAPPER.len() + 4].try_into().unwrap()) as usize;
    let total = (WRAPPER.len() + 4).checked_add(size);
    if size == 0 || size > MAX_FILE_SIZE - WRAPPER.len() - 4 || total != Some(bytes.len()) {
        return Err(refused("the protected key wrapper length is invalid."));
    }
    Ok(&bytes[WRAPPER.len() + 4..])
}

struct DpapiAllocation(CRYPT_INTEGER_BLOB);

impl Drop for DpapiAllocation {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            // SAFETY: DPAPI owns this output allocation until `LocalFree`. Clear
            // its full reported length before returning it to the native heap.
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                LocalFree(self.0.pbData.cast());
            }
        }
    }
}

fn protect(bytes: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, Failure> {
    crypt(bytes, entropy, true)
}

fn unprotect(bytes: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, Failure> {
    crypt(bytes, entropy, false)
}

fn crypt(bytes: &[u8], entropy: &[u8], encrypt: bool) -> Result<Zeroizing<Vec<u8>>, Failure> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = DpapiAllocation(CRYPT_INTEGER_BLOB::default());
    // SAFETY: DPAPI reads the retained input and entropy slices. Optional
    // arguments are null; the flags forbid prompts and the output is owned.
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
    };
    if result == 0 {
        return Err(refused("Windows current-user key protection failed."));
    }
    if output.0.pbData.is_null() || output.0.cbData == 0 || output.0.cbData as usize > MAX_FILE_SIZE
    {
        return Err(refused("Windows returned an invalid protected key size."));
    }
    // SAFETY: successful DPAPI output owns this many initialized bytes. The
    // native allocation is cleared by its owner after the guarded copy.
    Ok(Zeroizing::new(
        unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) }.to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_policy_rejects_remote_unknown_and_substituted_targets() {
        for kind in [0, 1, 4, 5, 7, u32::MAX] {
            assert!(!local_drive_type(kind));
        }
        for kind in [2, 3, 6] {
            assert!(local_drive_type(kind));
        }
        assert!(native_drive_target(r"\Device\HarddiskVolume4"));
        assert!(!native_drive_target(r"\DeviceAlias\volume"));
        assert!(!native_drive_target(r"\??\C:\project"));
        assert!(!native_drive_target(r"\DosDevices\C:\project"));
        assert!(!native_drive_target(""));
    }

    #[test]
    fn protected_packages_reload_and_are_distinct() {
        let parent = std::env::temp_dir().join(format!("vivac-device-key-{}", crate::id::ulid()));
        std::fs::create_dir(&parent).unwrap();
        let first = parent.join("first");
        let second = parent.join("second");
        let receipt = create(&first).unwrap();
        assert_eq!(receipt, show(&first).unwrap());
        let other = create(&second).unwrap();
        assert_ne!(receipt.device_id, other.device_id);
        assert_ne!(receipt.credential_id, other.credential_id);
        assert_ne!(receipt.public_key, other.public_key);
        assert!(create(&first).is_err());
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let target = parent.canonicalize().unwrap();
        assert!(target.starts_with(&temporary) && target != temporary);
        std::fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn package_changes_during_key_use_refuse_the_result() {
        let parent = std::env::temp_dir().join(format!("vivac-device-key-{}", crate::id::ulid()));
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("key");
        create(&path).unwrap();
        let result = load(&path, |credential| {
            std::fs::write(path.join("unexpected"), b"synthetic test member")?;
            Ok(credential.receipt("windows_current_user_dpapi"))
        });
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let target = parent.canonicalize().unwrap();
        assert!(target.starts_with(&temporary) && target != temporary);
        std::fs::remove_dir_all(target).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn stored_ciphertext_hides_the_seed_and_checks_bytes_and_identity() {
        let parent = std::env::temp_dir().join(format!("vivac-device-key-{}", crate::id::ulid()));
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("key");
        let mut payload = Zeroizing::new([0u8; PAYLOAD_SIZE]);
        payload[..PAYLOAD.len()].copy_from_slice(PAYLOAD);
        payload[PAYLOAD.len()..PAYLOAD.len() + 16].fill(0x11);
        payload[PAYLOAD.len() + 16..PAYLOAD.len() + 32].fill(0x22);
        payload[PAYLOAD.len() + 32..].fill(0x33);
        let receipt = create_with_entropy(&path, |bytes| {
            bytes.copy_from_slice(&payload[PAYLOAD.len()..]);
            Ok(())
        })
        .unwrap();
        assert_eq!(receipt, public_identity(&payload[..]).unwrap());
        let stored = std::fs::read(path.join(FILE_NAME)).unwrap();
        assert!(stored
            .windows(32)
            .all(|part| part != &payload[PAYLOAD_SIZE - 32..]));
        let directory = files::open_root(&path).unwrap();
        let mut file = open_key(&directory).unwrap();
        verify_contents(&mut file, &stored).unwrap();
        assert!(verify_contents(&mut file, b"different protected bytes").is_err());
        let (volume, identifier) = file_identity(&file).unwrap();
        assert!(verify_file(&file, (volume ^ 1, identifier), stored.len() as u64).is_err());
        assert!(verify_file(&file, (volume, identifier), stored.len() as u64 + 1).is_err());
        drop(file);
        drop(directory);
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let target = parent.canonicalize().unwrap();
        assert!(target.starts_with(&temporary) && target != temporary);
        std::fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn entropy_failure_creates_nothing() {
        let parent = std::env::temp_dir().join(format!("vivac-device-key-{}", crate::id::ulid()));
        std::fs::create_dir(&parent).unwrap();
        assert!(create_with_entropy(&parent.join("key"), |_| Err(refused(
            "test entropy failure"
        )))
        .is_err());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        std::fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn protected_payload_roundtrip_and_refusals() {
        let mut payload = Zeroizing::new([0x39; PAYLOAD_SIZE]);
        payload[..PAYLOAD.len()].copy_from_slice(PAYLOAD);
        let protected = protect(&payload[..], WRAPPER).unwrap();
        assert!(protected
            .windows(32)
            .all(|part| part != &payload[PAYLOAD_SIZE - 32..]));
        let bytes = wrap(&protected).unwrap();
        let decoded = unprotect(unwrap(&bytes).unwrap(), WRAPPER).unwrap();
        assert_eq!(&decoded[..], &payload[..]);
        assert!(public_identity(&decoded).is_ok());
        assert!(unprotect(&protected, b"foreign entropy").is_err());
        assert!(unprotect(&protected[..protected.len() / 2], WRAPPER).is_err());
        let mut changed = protected.clone();
        let last = changed.len() - 1;
        changed[last] ^= 0x80;
        assert!(unprotect(&changed, WRAPPER).is_err());
        for cut in 0..bytes.len() {
            assert!(unwrap(&bytes[..cut]).is_err());
        }
        let mut invalid = bytes.clone();
        invalid[0] ^= 1;
        assert!(unwrap(&invalid).is_err());
        let oversized = vec![0; MAX_FILE_SIZE + 1];
        assert!(unwrap(&oversized).is_err());
        assert!(public_identity(&payload[..PAYLOAD_SIZE - 1]).is_err());
        payload[0] ^= 1;
        assert!(public_identity(&payload[..]).is_err());
    }
}
