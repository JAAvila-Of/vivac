use crate::failure::Failure;
use libc::{c_int, c_void};
use std::mem::MaybeUninit;

unsafe extern "C" {
    fn filesec_init() -> *mut c_void;
    fn filesec_free(filesec: *mut c_void);
    fn filesec_query_property(filesec: *mut c_void, property: c_int, present: *mut c_int) -> c_int;
    #[cfg_attr(target_arch = "x86_64", link_name = "fstatx_np$INODE64")]
    fn fstatx_np(fd: c_int, stat: *mut libc::stat, filesec: *mut c_void) -> c_int;
}

struct FileSecurity(*mut c_void);

impl Drop for FileSecurity {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns a non-null descriptor from filesec_init.
        unsafe { filesec_free(self.0) };
    }
}

fn refused() -> Failure {
    Failure::usage("Private storage permissions are unavailable.")
}

pub(super) fn verify(fd: c_int) -> Result<(), Failure> {
    let mut filesystem = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs writes the maintained libc structure only on success.
    if unsafe { libc::fstatfs(fd, filesystem.as_mut_ptr()) } != 0 {
        return Err(refused());
    }
    // SAFETY: the successful fstatfs call initialized the structure.
    let filesystem = unsafe { filesystem.assume_init() };
    let flags = filesystem.f_flags as u64;
    let name = filesystem.f_fstypename.map(|byte| byte as u8);
    let name = std::ffi::CStr::from_bytes_until_nul(&name).map_err(|_| refused())?;
    if flags & libc::MNT_LOCAL as u64 == 0
        || flags & libc::MNT_IGNORE_OWNERSHIP as u64 != 0
        || !matches!(name.to_bytes(), b"apfs" | b"hfs")
    {
        return Err(refused());
    }
    // SAFETY: fpathconf reads the filesystem capability for the retained FD.
    if unsafe { libc::fpathconf(fd, libc::_PC_EXTENDED_SECURITY_NP) } != 1 {
        return Err(refused());
    }
    // SAFETY: filesec_init returns an independently owned descriptor or null.
    let descriptor = unsafe { filesec_init() };
    if descriptor.is_null() {
        return Err(refused());
    }
    let descriptor = FileSecurity(descriptor);
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the live descriptor and maintained libc stat output belong to
    // this call; no stat fields are read and RAII releases the descriptor.
    if unsafe { fstatx_np(fd, stat.as_mut_ptr(), descriptor.0) } != 0 {
        return Err(refused());
    }
    for (property, required) in [(3, 1), (6, 1), (5, 0)] {
        let mut present = -1;
        // SAFETY: these public filesec property identifiers only query
        // presence through an initialized integer output buffer.
        if unsafe { filesec_query_property(descriptor.0, property, &mut present) } != 0
            || present != required
        {
            return Err(refused());
        }
    }
    Ok(())
}

#[test]
fn invalid_descriptor_is_refused() {
    assert!(verify(-1).is_err());
}

#[test]
fn empty_snapshot_private_profile_interoperates() {
    use std::os::fd::AsRawFd;

    fn errno(failed: bool) -> i32 {
        if failed {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        } else {
            0
        }
    }

    fn observe(fd: c_int) -> (String, Option<(c_int, c_int)>) {
        let mut filesystem = MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: the retained empty snapshot FD and maintained output are live.
        let fs_result = unsafe { libc::fstatfs(fd, filesystem.as_mut_ptr()) };
        let fs_errno = errno(fs_result != 0);
        let (fs_type, local, ignores_ownership) = if fs_result == 0 {
            // SAFETY: successful fstatfs initialized the maintained structure.
            let filesystem = unsafe { filesystem.assume_init() };
            let name = filesystem.f_fstypename.map(|byte| byte as u8);
            let name = std::ffi::CStr::from_bytes_until_nul(&name)
                .map(|name| name.to_string_lossy().into_owned())
                .ok();
            (
                name,
                Some(filesystem.f_flags as u64 & libc::MNT_LOCAL as u64 != 0),
                Some(filesystem.f_flags as u64 & libc::MNT_IGNORE_OWNERSHIP as u64 != 0),
            )
        } else {
            (None, None, None)
        };
        // SAFETY: this capability query uses the same retained empty snapshot FD.
        let pathconf = unsafe { libc::fpathconf(fd, libc::_PC_EXTENDED_SECURITY_NP) };
        let pathconf_errno = errno(pathconf == -1);
        // SAFETY: filesec_init returns an owned descriptor or null.
        let descriptor = unsafe { filesec_init() };
        let initialized = !descriptor.is_null();
        let mut statx = None;
        let mut statx_errno = None;
        let mut queries = Vec::new();
        if initialized {
            let descriptor = FileSecurity(descriptor);
            let mut stat = MaybeUninit::<libc::stat>::uninit();
            // SAFETY: both output objects remain owned and the FD stays live.
            let result = unsafe { fstatx_np(fd, stat.as_mut_ptr(), descriptor.0) };
            statx_errno = Some(errno(result != 0));
            statx = Some(result);
            for property in [3, 6, 5] {
                let mut present = -1;
                // SAFETY: public presence queries use a valid descriptor and
                // integer output even when preceding filesystem filters failed.
                let result =
                    unsafe { filesec_query_property(descriptor.0, property, &mut present) };
                let error = errno(result != 0);
                queries.push((property, result, error, present));
            }
        }
        let acl = queries
            .iter()
            .find(|query| query.0 == 5)
            .map(|query| (query.1, query.3));
        let diagnostic = format!(
        "fs_result={fs_result}, fs_errno={fs_errno}, fs_type={fs_type:?}, local={local:?}, ignores_ownership={ignores_ownership:?}, pathconf={pathconf}, pathconf_errno={pathconf_errno}, initialized={initialized}, statx={statx:?}, statx_errno={statx_errno:?}, queries(property,return,errno,presence)={queries:?}");
        (diagnostic, acl)
    }

    let preparation = super::Preparation::snapshot().unwrap();
    let fd = preparation.dir.as_raw_fd();
    let (before, before_acl) = observe(fd);
    let verified = verify(fd).is_ok();
    let injected = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(std::env::temp_dir().join(&preparation.name))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let (after, after_acl) = observe(fd);
    assert!(
        injected && before_acl == Some((0, 0)) && after_acl == Some((0, 1)),
        "Private profile ACL transition: injected={injected}; before=[{before}]; after=[{after}]"
    );
    assert!(
        verified,
        "Private profile probe: before=[{before}]; after=[{after}]"
    );
}
