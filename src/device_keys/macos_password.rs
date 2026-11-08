use crate::failure::Failure;
use zeroize::Zeroizing;

pub(super) struct Password {
    bytes: Zeroizing<[u8; 1025]>,
    len: usize,
}

fn parse(bytes: Zeroizing<[u8; 1025]>) -> Result<Password, Failure> {
    let len = bytes
        .iter()
        .rposition(|byte| *byte == 0)
        .ok_or_else(invalid)?;
    if !(16..=512).contains(&len)
        || bytes[..len].contains(&0)
        || bytes[len + 1..].iter().any(|byte| *byte != 0xff)
        || std::str::from_utf8(&bytes[..len]).is_err()
    {
        return Err(invalid());
    }
    Ok(Password { bytes, len })
}

fn invalid() -> Failure {
    super::refused("the device key passphrase is invalid or unavailable.")
}

#[cfg(target_os = "macos")]
impl Password {
    pub(super) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("validated passphrase")
    }
}

#[cfg(target_os = "macos")]
pub(super) fn read_confirmed() -> Result<Password, Failure> {
    let password = read(c"New device key passphrase: ")?;
    let confirmation = read(c"Confirm device key passphrase: ")?;
    if password.as_str() != confirmation.as_str() {
        return Err(invalid());
    }
    Ok(password)
}

#[cfg(target_os = "macos")]
pub(super) fn read_unlock() -> Result<Password, Failure> {
    read(c"Device key passphrase: ")
}

#[cfg(target_os = "macos")]
fn read(prompt: &std::ffi::CStr) -> Result<Password, Failure> {
    unsafe extern "C" {
        fn readpassphrase(
            prompt: *const libc::c_char,
            buffer: *mut libc::c_char,
            size: libc::size_t,
            flags: libc::c_int,
        ) -> *mut libc::c_char;
    }

    const RPP_REQUIRE_TTY: libc::c_int = 0x02;
    let mut bytes = Zeroizing::new([0xff; 1025]);
    // A larger buffer makes native truncation exceed the accepted byte limit.
    // Padding is checked, rather than assumed to be preserved by the API.
    // SAFETY: the prompt is terminated, the initialized output spans its exact
    // supplied length, and both remain live throughout this synchronous call.
    let result = unsafe {
        readpassphrase(
            prompt.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            RPP_REQUIRE_TTY,
        )
    };
    if result.is_null() {
        return Err(invalid());
    }
    parse(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(value: &[u8]) -> Zeroizing<[u8; 1025]> {
        let mut bytes = Zeroizing::new([0xff; 1025]);
        bytes[..value.len()].copy_from_slice(value);
        bytes[value.len()] = 0;
        bytes
    }

    #[test]
    fn accepts_valid_utf8_at_both_byte_limits() {
        for value in ["a".repeat(16), "a".repeat(512), "é".repeat(8)] {
            let parsed = parse(buffer(value.as_bytes())).expect("valid passphrase");
            assert_eq!(&parsed.bytes[..parsed.len], value.as_bytes());
        }
    }

    #[test]
    fn refuses_short_long_and_full_native_buffers() {
        for len in [0, 15, 513, 1024] {
            assert!(parse(buffer(&vec![b'a'; len])).is_err());
        }
        assert!(parse(Zeroizing::new([b'a'; 1025])).is_err());
    }

    #[test]
    fn refuses_embedded_nuls_invalid_utf8_and_changed_padding() {
        for position in [0, 8, 16] {
            let mut value = [b'a'; 17];
            value[position] = 0;
            assert!(parse(buffer(&value)).is_err());
        }
        let mut invalid = [b'a'; 16];
        invalid[5] = 0xff;
        assert!(parse(buffer(&invalid)).is_err());
        let mut changed = buffer(&[b'a'; 16]);
        changed[20] = b'a';
        assert!(parse(changed).is_err());
    }
}
