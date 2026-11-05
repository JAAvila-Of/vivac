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
