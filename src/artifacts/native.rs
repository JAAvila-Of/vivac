use crate::failure::Failure;
#[cfg(not(windows))]
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::io;

pub(super) const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos", windows));

pub(super) fn sync_preparation(dir: &Dir) -> Result<(), Failure> {
    #[cfg(windows)]
    {
        // The retained native handle has FILE_ALL_ACCESS. Reopening a directory
        // with `cap-std`'s `maybe_dir` would remove sharing of its held DELETE
        // access; duplicating this owned handle preserves both access and pin.
        dir.try_clone()?.into_std_file().sync_all()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        super::files::sync_directory(dir)
    }
}

pub(super) struct Preparation {
    parent: Dir,
    pub(super) name: String,
    pub(super) dir: Dir,
    published: bool,
}

impl Preparation {
    pub(super) fn new(parent: &Dir) -> Result<Self, Failure> {
        if !SUPPORTED {
            return Err(Failure::usage(
                "Exclusive native bundle publication is unavailable on this platform.",
            ));
        }
        let name = format!(".tmp-bundle-{}", crate::id::ulid());
        let dir = create_private(parent, &name)?;
        Ok(Self {
            parent: parent.try_clone()?,
            name,
            dir,
            published: false,
        })
    }

    pub(super) fn publish(&mut self, destination: &Dir, name: &str) -> Result<(), Failure> {
        self.same_directory()?;
        publish_exclusive(&self.parent, &self.name, &self.dir, destination, name)?;
        self.published = true;
        super::files::sync_directory(destination)?;
        Ok(())
    }

    fn same_directory(&self) -> Result<(), Failure> {
        #[cfg(windows)]
        let current = windows::open_existing(&self.parent, &self.name)?;
        #[cfg(not(windows))]
        let current = self.parent.open_dir_nofollow(&self.name)?;
        same_directory(&self.dir, &current)
    }
}

impl Drop for Preparation {
    fn drop(&mut self) {
        if !self.published && self.same_directory().is_ok() {
            // `cap-std` recursively removes beneath directory handles without
            // following links. Only this preparation's identity is eligible.
            if let Ok(parent_handle) = self.parent.try_clone() {
                // The original Windows handle denies external deletion during
                // preparation; close it before our capability-based cleanup.
                let original = std::mem::replace(&mut self.dir, parent_handle);
                drop(original);
                let _ = self.parent.remove_dir_all(&self.name);
            }
        }
    }
}

#[cfg(unix)]
pub(super) fn same_directory(first: &Dir, second: &Dir) -> Result<(), Failure> {
    use cap_std::fs::MetadataExt;
    let first = first.dir_metadata()?;
    let second = second.dir_metadata()?;
    if first.dev() != second.dev() || first.ino() != second.ino() {
        return Err(Failure::usage(
            "The bundle directory identity changed during the operation.",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn same_directory(_: &Dir, _: &Dir) -> Result<(), Failure> {
    Err(Failure::usage(
        "Exclusive native bundle publication is unavailable on this platform.",
    ))
}

#[cfg(not(any(unix, windows)))]
fn create_private(_: &Dir, _: &str) -> Result<Dir, Failure> {
    Err(Failure::usage(
        "Exclusive native bundle publication is unavailable on this platform.",
    ))
}

#[cfg(windows)]
pub(super) fn same_directory(first: &Dir, second: &Dir) -> Result<(), Failure> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let identity = |dir: &Dir| -> Result<(u64, [u8; 16]), Failure> {
        let mut info = FILE_ID_INFO::default();
        // SAFETY: Dir keeps the handle alive and the maintained structure
        // matches the exact output buffer length passed to Windows.
        if unsafe {
            GetFileInformationByHandleEx(
                dir.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok((info.VolumeSerialNumber, info.FileId.Identifier))
    };
    if identity(first)? != identity(second)? {
        return Err(Failure::usage(
            "The bundle directory identity changed during the operation.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn require_controlled_parent(parent: &Dir) -> Result<(), Failure> {
    use cap_std::fs::MetadataExt;
    let parent_metadata = parent.dir_metadata()?;
    // SAFETY: `geteuid` has no arguments or borrowed buffers.
    if parent_metadata.uid() != unsafe { libc::geteuid() } || parent_metadata.mode() & 0o022 != 0 {
        return Err(Failure::usage(
            "Private bundle preparation requires an owner-controlled parent directory.",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn require_controlled_parent(_: &Dir) -> Result<(), Failure> {
    Ok(())
}

#[cfg(unix)]
fn create_private(parent: &Dir, name: &str) -> Result<Dir, Failure> {
    use cap_std::fs::{DirBuilder, DirBuilderExt, MetadataExt};
    require_controlled_parent(parent)?;
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    parent.create_dir_with(name, &builder)?;
    let directory = parent.open_dir_nofollow(name)?;
    let metadata = directory.dir_metadata()?;
    // SAFETY: `geteuid` has no arguments or borrowed buffers.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(Failure::usage(
            "Private bundle preparation permissions are unavailable.",
        ));
    }
    Ok(directory)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn publish_exclusive(
    parent: &Dir,
    source: &str,
    _prepared: &Dir,
    destination: &Dir,
    name: &str,
) -> Result<(), Failure> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    let source = CString::new(source).map_err(|_| Failure::usage("Invalid preparation name."))?;
    let name = CString::new(name).map_err(|_| Failure::usage("Invalid destination name."))?;
    // SAFETY: both directory descriptors remain owned by their Dir objects,
    // and the nul-terminated names remain alive throughout the native call.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent.as_raw_fd(),
            source.as_ptr(),
            destination.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            source.as_ptr(),
            destination.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn publish_exclusive(_: &Dir, _: &str, _: &Dir, _: &Dir, _: &str) -> Result<(), Failure> {
    Err(Failure::usage(
        "Exclusive native bundle publication is unavailable on this platform.",
    ))
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::ptr;
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::*;
    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, HANDLE, OBJ_DONT_REPARSE, UNICODE_STRING,
    };
    use windows_sys::Win32::Security::Authorization::*;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeInformationByHandleW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    struct Allocation(*mut core::ffi::c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            // SAFETY: the Windows conversion/security APIs allocate with `LocalAlloc`.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns the successful token open's handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn nt_result(status: i32) -> Result<(), Failure> {
        if status < 0 {
            // SAFETY: status conversion does not access caller memory.
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosErrorNoTeb(status) } as i32,
            )
            .into());
        }
        Ok(())
    }
    pub(super) fn open_existing(parent: &Dir, name: &str) -> Result<Dir, Failure> {
        let mut name: Vec<u16> = name.encode_utf16().collect();
        let unicode = UNICODE_STRING {
            Length: (name.len() * 2) as u16,
            MaximumLength: (name.len() * 2) as u16,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: parent.as_raw_handle(),
            ObjectName: &unicode,
            Attributes: OBJ_DONT_REPARSE,
            ..Default::default()
        };
        let mut status = IO_STATUS_BLOCK::default();
        let mut handle = ptr::null_mut();
        // SAFETY: this single-component metadata open stays relative to the
        // parent handle and shares the already-held preparation's DELETE
        // access, without itself requesting DELETE or following reparse points.
        nt_result(unsafe {
            NtCreateFile(
                &mut handle,
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &attributes,
                &mut status,
                ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                ptr::null(),
                0,
            )
        })?;
        // SAFETY: the successful open returns a new owned directory handle.
        let dir = Dir::from_std_file(unsafe { std::fs::File::from_raw_handle(handle) });
        super::super::files::reject_link(&dir.dir_metadata()?)?;
        Ok(dir)
    }
    fn user_descriptor() -> Result<Allocation, Failure> {
        let mut token = ptr::null_mut();
        // SAFETY: -1 is the documented current-process pseudohandle. Only
        // TOKEN_QUERY is requested, and the output handle has owned storage.
        nt_result(unsafe { NtOpenProcessToken((-1isize) as HANDLE, TOKEN_QUERY, &mut token) })?;
        let token = Token(token);
        let mut required = 0;
        // SAFETY: the first call only obtains the bounded required length.
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 64 * 1024 {
            return Err(Failure::usage("The bundle owner identity is unavailable."));
        }
        let mut buffer = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the aligned buffer has at least the requested bytes; the
        // token remains owned, and TOKEN_USER is read only after success.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = ptr::null_mut();
        // SAFETY: `TOKEN_USER` owns a valid SID within the live token buffer.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let sid_allocation = Allocation(sid.cast());
        let mut length = 0;
        // SAFETY: conversion returns a nul-terminated SID string; SID strings
        // have a bounded format and the allocation is retained until copied.
        while unsafe { *sid.add(length) } != 0 {
            length += 1;
            if length > 256 {
                return Err(Failure::usage("The bundle owner identity is unavailable."));
            }
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, length) })
            .map_err(|_| Failure::usage("Invalid bundle owner identity."))?;
        drop(sid_allocation);
        let text: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: the UTF-16 descriptor is terminated and its allocation stays
        // alive; Windows supplies an independently allocated descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Allocation(descriptor))
    }
    fn verify_security(dir: &Dir, expected: &Allocation) -> Result<(), Failure> {
        let mut actual = ptr::null_mut();
        // SAFETY: the directory handle remains live and Windows allocates a
        // self-contained descriptor. No inherited permissions are accepted.
        let result = unsafe {
            GetSecurityInfo(
                dir.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut actual,
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32).into());
        }
        let actual = Allocation(actual);
        let mut control = 0;
        let mut revision = 0;
        let mut owner = ptr::null_mut();
        let mut expected_owner = ptr::null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut acl = ptr::null_mut();
        // SAFETY: both descriptors are allocated by Windows and retained.
        let valid = unsafe {
            GetSecurityDescriptorControl(actual.0, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0
                && GetSecurityDescriptorOwner(actual.0, &mut owner, &mut defaulted) != 0
                && GetSecurityDescriptorOwner(expected.0, &mut expected_owner, &mut defaulted) != 0
                && EqualSid(owner, expected_owner) != 0
                && GetSecurityDescriptorDacl(actual.0, &mut present, &mut acl, &mut defaulted) != 0
                && present != 0
                && !acl.is_null()
                && (*acl).AceCount == 1
        };
        if !valid {
            return Err(Failure::usage(
                "Private bundle preparation permissions are unavailable.",
            ));
        }
        let mut ace = ptr::null_mut();
        // SAFETY: the validated ACL has exactly one ACE, whose maintained
        // `ACCESS_ALLOWED_ACE` layout has a trailing SID beginning at `SidStart`.
        let valid = unsafe {
            GetAce(acl, 0, &mut ace) != 0
                && !ace.is_null()
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceType == 0
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask == FILE_ALL_ACCESS
                && EqualSid(
                    ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast(),
                    expected_owner,
                ) != 0
        };
        if !valid {
            return Err(Failure::usage(
                "Private bundle preparation permissions are unavailable.",
            ));
        }
        Ok(())
    }
    pub(super) fn create(parent: &Dir, name: &str) -> Result<Dir, Failure> {
        let mut flags = 0;
        // SAFETY: the live parent directory determines the volume; optional
        // string outputs are null and the flags output has valid storage.
        if unsafe {
            GetVolumeInformationByHandleW(
                parent.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut flags,
                ptr::null_mut(),
                0,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if flags & 0x0000_0008 == 0 {
            return Err(Failure::usage(
                "Private bundle preparation requires persistent filesystem ACLs.",
            ));
        }
        let descriptor = user_descriptor()?;
        let mut name: Vec<u16> = name.encode_utf16().collect();
        let unicode = UNICODE_STRING {
            Length: (name.len() * 2) as u16,
            MaximumLength: (name.len() * 2) as u16,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: parent.as_raw_handle(),
            ObjectName: &unicode,
            Attributes: OBJ_DONT_REPARSE,
            SecurityDescriptor: descriptor.0.cast(),
            ..Default::default()
        };
        let mut status = IO_STATUS_BLOCK::default();
        let mut handle = ptr::null_mut();
        // SAFETY: creation is a single component relative to the owned parent
        // handle, with exclusive disposition, no reparse traversal and the
        // protected owner-only ACL supplied at creation, never after creation.
        nt_result(unsafe {
            NtCreateFile(
                &mut handle,
                FILE_ALL_ACCESS,
                &attributes,
                &mut status,
                ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_CREATE,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                ptr::null(),
                0,
            )
        })?;
        // SAFETY: a successful `NtCreateFile` transfers a new owned handle.
        let dir = Dir::from_std_file(unsafe { std::fs::File::from_raw_handle(handle) });
        super::super::files::reject_link(&dir.dir_metadata()?)?;
        verify_security(&dir, &descriptor)?;
        Ok(dir)
    }
    pub(super) fn publish(prepared: &Dir, destination: &Dir, name: &str) -> Result<(), Failure> {
        let name: Vec<u16> = name.encode_utf16().collect();
        let offset = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
        let length = offset + name.len() * 2;
        let mut buffer = vec![0usize; length.div_ceil(std::mem::size_of::<usize>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: the `usize` allocation provides native alignment and enough
        // trailing UTF-16 storage. The binding supplies the header layout.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = false;
            (*info).RootDirectory = destination.as_raw_handle();
            (*info).FileNameLength = (name.len() * 2) as u32;
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: the original preparation handle is retained continuously;
        // both handles and the aligned variable-length buffer stay live. This
        // operation rejects occupied destinations without a path-based rename.
        nt_result(unsafe {
            NtSetInformationFile(
                prepared.as_raw_handle(),
                &mut status,
                info.cast(),
                length as u32,
                FileRenameInformation,
            )
        })
    }
}

#[cfg(windows)]
fn create_private(parent: &Dir, name: &str) -> Result<Dir, Failure> {
    windows::create(parent, name)
}
#[cfg(windows)]
fn publish_exclusive(
    _: &Dir,
    _: &str,
    prepared: &Dir,
    destination: &Dir,
    name: &str,
) -> Result<(), Failure> {
    windows::publish(prepared, destination, name)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use super::*;
    #[test]
    fn simultaneous_preparations_publish_one_complete_winner_without_overwriting() {
        use std::sync::{Arc, Barrier};
        let path = std::env::temp_dir().join(format!("vivac-native-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut prepared = Vec::new();
        for payload in [
            b"First complete delivery.".as_slice(),
            b"Second complete delivery.".as_slice(),
        ] {
            let stage = Preparation::new(&parent).unwrap();
            stage.dir.write("index.html", payload).unwrap();
            stage.dir.write("style.css", payload).unwrap();
            sync_preparation(&stage.dir).unwrap();
            assert_eq!(stage.dir.read("index.html").unwrap(), payload);
            assert_eq!(stage.dir.read("style.css").unwrap(), payload);
            assert_eq!(stage.dir.entries().unwrap().count(), 2);
            prepared.push((stage, payload, parent.try_clone().unwrap()));
        }
        let mut workers = Vec::new();
        for (mut stage, payload, parent) in prepared {
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let result = stage.publish(&parent, "restored");
                drop(stage);
                (payload, result)
            }));
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_err()).count(),
            1
        );
        let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap().0;
        assert_eq!(parent.read("restored/index.html").unwrap(), winner);
        assert_eq!(parent.read("restored/style.css").unwrap(), winner);
        let restored = parent.open_dir("restored").unwrap();
        assert_eq!(restored.entries().unwrap().count(), 2);
        assert_eq!(parent.entries().unwrap().count(), 1);
        drop(restored);
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn held_preparation_publishes_complete_directory_and_preserves_it_on_drop() {
        let path = std::env::temp_dir().join(format!("vivac-native-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        let mut stage = Preparation::new(&parent).unwrap();
        stage
            .dir
            .write("index.html", b"Prepared complete payload.")
            .unwrap();
        let temporary = stage.name.clone();
        stage.publish(&parent, "complete").unwrap();
        drop(stage);
        assert!(!path.join(temporary).exists());
        assert_eq!(
            parent.read("complete/index.html").unwrap(),
            b"Prepared complete payload."
        );
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn preparation_rejects_shared_parents_and_cleanup_never_follows_links() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("vivac-native-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Preparation::new(&parent).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        parent.create_dir("outside").unwrap();
        parent
            .write("outside/kept.txt", b"Keep this outside the preparation.")
            .unwrap();
        let stage = Preparation::new(&parent).unwrap();
        std::os::unix::fs::symlink(path.join("outside"), path.join(&stage.name).join("link"))
            .unwrap();
        let stage_name = stage.name.clone();
        drop(stage);
        assert!(!path.join(stage_name).exists());
        assert_eq!(
            parent.read("outside/kept.txt").unwrap(),
            b"Keep this outside the preparation."
        );
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn publication_never_replaces_an_occupied_directory() {
        let path = std::env::temp_dir().join(format!("vivac-native-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        parent.create_dir("destination").unwrap();
        let mut stage = Preparation::new(&parent).unwrap();
        assert!(stage.publish(&parent, "destination").is_err());
        assert!(parent
            .open_dir("destination")
            .unwrap()
            .entries()
            .unwrap()
            .next()
            .is_none());
        drop(stage);
        assert_eq!(parent.entries().unwrap().count(), 1);
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }
}
