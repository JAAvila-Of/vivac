use crate::linux_reservation::StoreReservation;
use crate::linux_resident_reader::{opening_permit_frame, OpeningPermitKind};
use crate::linux_session::PackageLease;
use crate::package::{OpeningStorage, PublishedDraft};
use std::ffi::CString;
use std::io;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::FileExt;

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn checked<T>(value: std::result::Result<T, ()>) -> io::Result<T> {
    value.map_err(|()| invalid())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PermitIdentity {
    device: (u32, u32),
    mount: u64,
    inode: u64,
    size: u64,
    modified: (i64, u32),
    changed: (i64, u32),
}

impl PermitIdentity {
    fn capture(file: &std::fs::File, storage: &OpeningStorage<'_>) -> io::Result<Self> {
        checked(storage.verify_object(file.as_raw_fd(), false))?;
        let value = checked(storage.capture(file.as_raw_fd()))?;
        if value.stx_size != 48 {
            return Err(invalid());
        }
        Ok(Self {
            device: (value.stx_dev_major, value.stx_dev_minor),
            mount: value.stx_mnt_id,
            inode: value.stx_ino,
            size: value.stx_size,
            modified: (value.stx_mtime.tv_sec, value.stx_mtime.tv_nsec),
            changed: (value.stx_ctime.tv_sec, value.stx_ctime.tv_nsec),
        })
    }
}

fn create_permit(
    directory: &OwnedFd,
    name: &CString,
    storage: &OpeningStorage<'_>,
) -> io::Result<std::fs::File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (libc::O_RDWR
            | libc::O_CREAT
            | libc::O_EXCL
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK) as u64,
        mode: 0o600,
        resolve: libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV,
    };
    // SAFETY: the terminated component and initialized open_how are live.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat2 returned one fresh owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as RawFd) };
    checked(storage.capture(descriptor.as_raw_fd()))?;
    Ok(std::fs::File::from(descriptor))
}

fn verify_permit(
    file: &std::fs::File,
    identity: PermitIdentity,
    bytes: &[u8; 48],
    storage: &OpeningStorage<'_>,
) -> io::Result<()> {
    if PermitIdentity::capture(file, storage)? != identity {
        return Err(invalid());
    }
    let mut actual = [0; 49];
    let count = file.read_at(&mut actual, 0)?;
    if count != bytes.len() || &actual[..count] != bytes {
        return Err(invalid());
    }
    if PermitIdentity::capture(file, storage)? != identity {
        return Err(invalid());
    }
    Ok(())
}

fn directory_identity(
    directory: &OwnedFd,
    storage: &OpeningStorage<'_>,
) -> io::Result<((u32, u32), u64, u64)> {
    checked(storage.verify_object(directory.as_raw_fd(), true))?;
    let value = checked(storage.capture(directory.as_raw_fd()))?;
    Ok((
        (value.stx_dev_major, value.stx_dev_minor),
        value.stx_mnt_id,
        value.stx_ino,
    ))
}

/// Retains the package reservation and the exact newly published permit.
pub(crate) struct OpeningPublication {
    lease: PackageLease,
    reservation: StoreReservation,
    file: std::fs::File,
    identity: PermitIdentity,
    directory: ((u32, u32), u64, u64),
    bytes: [u8; 48],
}

pub(crate) fn prepare(package: &PublishedDraft) -> io::Result<OpeningPublication> {
    checked(package.validate())?;
    let storage = package.opening_storage();
    let (directory, cipher) = storage.duplicate_material()?;
    let lease = PackageLease::capture(directory, &cipher)?;
    if lease.ipc_identity().map_err(|_| invalid())? != checked(package.ipc_identity())? {
        return Err(invalid());
    }
    let reservation = StoreReservation::acquire(&lease)?;
    let canonical = package.record().canonical_bytes()?;
    let bytes = opening_permit_frame(OpeningPermitKind::FirstOpen, &canonical);
    let name = CString::new(package.record().credential_id.as_bytes()).map_err(|_| invalid())?;
    let publication = storage.with_opening_directory(|directory| {
        reservation.validate(&lease)?;
        checked(package.validate())?;
        let collection_identity = directory_identity(directory, &storage)?;
        let mut file = create_permit(directory, &name, &storage)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let identity = PermitIdentity::capture(&file, &storage)?;
        verify_permit(&file, identity, &bytes, &storage)?;
        std::fs::File::from(directory.try_clone()?).sync_all()?;
        reservation.validate(&lease)?;
        checked(package.validate())?;
        Ok(OpeningPublication {
            lease,
            reservation,
            file,
            identity,
            directory: collection_identity,
            bytes,
        })
    })?;
    publication.validate(package)?;
    Ok(publication)
}

impl OpeningPublication {
    pub(crate) fn validate(&self, package: &PublishedDraft) -> io::Result<()> {
        checked(package.validate())?;
        self.reservation.validate(&self.lease)?;
        if self.lease.ipc_identity().map_err(|_| invalid())? != checked(package.ipc_identity())?
            || self.bytes
                != opening_permit_frame(
                    OpeningPermitKind::FirstOpen,
                    &package.record().canonical_bytes()?,
                )
        {
            return Err(invalid());
        }
        let storage = package.opening_storage();
        storage.with_opening_directory(|directory| {
            if directory_identity(directory, &storage)? != self.directory {
                return Err(invalid());
            }
            verify_permit(&self.file, self.identity, &self.bytes, &storage)?;
            let named = storage.open_named(directory)?;
            verify_permit(
                &std::fs::File::from(named),
                self.identity,
                &self.bytes,
                &storage,
            )?;
            self.reservation.validate(&self.lease)?;
            checked(package.validate())
        })
    }
}
