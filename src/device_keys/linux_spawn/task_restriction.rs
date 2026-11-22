use std::io;

pub(super) fn require_supported() -> io::Result<()> {
    if cfg!(all(target_arch = "x86_64", target_pointer_width = "64")) {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

#[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
pub(super) fn install() -> io::Result<()> {
    const LOAD_WORD_ABSOLUTE: u16 = 0x20;
    const JUMP_EQUAL: u16 = 0x15;
    const JUMP_GREATER: u16 = 0x25;
    const JUMP_GREATER_EQUAL: u16 = 0x35;
    const JUMP_BITS_SET: u16 = 0x45;
    const RETURN: u16 = 0x06;
    const KILL_PROCESS: u32 = 0x8000_0000;
    const ALLOW: u32 = 0x7fff_0000;
    const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;

    const fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }

    // seccomp_data begins with nr at byte 0 and arch at byte 4. Every
    // alternative ABI and the historical x32 syscall range fail closed.
    let mut filter = [
        instruction(LOAD_WORD_ABSOLUTE, 0, 0, 4),
        instruction(JUMP_EQUAL, 1, 0, AUDIT_ARCH_X86_64),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(LOAD_WORD_ABSOLUTE, 0, 0, 0),
        instruction(JUMP_BITS_SET, 0, 1, X32_SYSCALL_BIT),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_GREATER_EQUAL, 0, 2, 512),
        instruction(JUMP_GREATER, 1, 0, 547),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_EQUAL, 0, 1, libc::SYS_fork as u32),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_EQUAL, 0, 1, libc::SYS_vfork as u32),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_EQUAL, 0, 1, libc::SYS_clone as u32),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_EQUAL, 0, 1, libc::SYS_clone3 as u32),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(JUMP_EQUAL, 0, 1, libc::SYS_io_uring_setup as u32),
        instruction(RETURN, 0, 0, KILL_PROCESS),
        instruction(RETURN, 0, 0, ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: the fixed filter and program remain live for the scalar prctl
    // syscall. The child already set no_new_privs, and the kernel copies the
    // complete filter before returning. This path allocates no memory.
    let result = unsafe {
        libc::syscall(
            libc::SYS_prctl,
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            std::ptr::addr_of!(program),
            0usize,
            0usize,
        )
    };
    if result < 0 {
        Err(super::last_error())
    } else {
        Ok(())
    }
}

#[cfg(not(all(target_arch = "x86_64", target_pointer_width = "64")))]
pub(super) fn install() -> io::Result<()> {
    require_supported()
}
