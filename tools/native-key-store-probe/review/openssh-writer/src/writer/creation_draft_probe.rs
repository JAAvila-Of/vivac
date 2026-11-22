use std::ffi::OsString;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::RunError;

const MAX_PACKET: usize = 8192;

struct Arguments {
    device: String,
    credential: String,
    observer: RawFd,
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<Arguments, ()> {
    let device = arguments.next().ok_or(())?.into_string().map_err(|_| ())?;
    let credential = arguments.next().ok_or(())?.into_string().map_err(|_| ())?;
    if !super::valid_identifier(&device)
        || !super::valid_identifier(&credential)
        || arguments.next().as_deref() != Some(std::ffi::OsStr::new("--observer-fd"))
    {
        return Err(());
    }
    let descriptor = arguments.next().ok_or(())?.into_string().map_err(|_| ())?;
    if descriptor.is_empty() || !descriptor.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    let observer = descriptor.parse::<RawFd>().map_err(|_| ())?;
    if observer < 3 || arguments.next().is_some() {
        return Err(());
    }
    Ok(Arguments {
        device,
        credential,
        observer,
    })
}

fn observer_socket(descriptor: RawFd) -> Result<OwnedFd, ()> {
    // The dedicated probe argument transfers this inherited descriptor to us.
    // Confirm that it is open before constructing its single owning handle.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if descriptor < 3 || flags < 0 {
        return Err(());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(descriptor) };
    if unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } != 0 {
        return Err(());
    }
    let owned_flags = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFD) };
    if owned_flags < 0 || owned_flags & libc::FD_CLOEXEC == 0 {
        return Err(());
    }
    let mut socket_type: libc::c_int = 0;
    let mut socket_type_length = std::mem::size_of_val(&socket_type) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            std::ptr::addr_of_mut!(socket_type).cast(),
            &mut socket_type_length,
        )
    } != 0
        || socket_type_length as usize != std::mem::size_of_val(&socket_type)
        || socket_type != libc::SOCK_SEQPACKET
    {
        return Err(());
    }
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut address_length = std::mem::size_of_val(&address) as libc::socklen_t;
    if unsafe {
        libc::getsockname(
            socket.as_raw_fd(),
            std::ptr::addr_of_mut!(address).cast(),
            &mut address_length,
        )
    } != 0
        || address_length < std::mem::size_of::<libc::sa_family_t>() as libc::socklen_t
        || address.ss_family as libc::c_int != libc::AF_UNIX
    {
        return Err(());
    }
    let timeout = libc::timeval {
        tv_sec: 15,
        tv_usec: 0,
    };
    for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        if unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                option,
                std::ptr::addr_of!(timeout).cast(),
                std::mem::size_of_val(&timeout) as libc::socklen_t,
            )
        } != 0
        {
            return Err(());
        }
    }
    Ok(socket)
}

fn send_packet(socket: &OwnedFd, packet: &[u8]) -> Result<(), ()> {
    if packet.is_empty() || packet.len() > MAX_PACKET {
        return Err(());
    }
    let sent = unsafe {
        libc::send(
            socket.as_raw_fd(),
            packet.as_ptr().cast(),
            packet.len(),
            libc::MSG_NOSIGNAL,
        )
    };
    if sent != packet.len() as libc::ssize_t {
        return Err(());
    }
    Ok(())
}

fn wait_for_validation(socket: &OwnedFd) -> Result<(), ()> {
    let mut acknowledgement = [0u8; 1];
    let received = unsafe {
        libc::recv(
            socket.as_raw_fd(),
            acknowledgement.as_mut_ptr().cast(),
            acknowledgement.len(),
            libc::MSG_TRUNC,
        )
    };
    if received != 1 || acknowledgement != *b"V" {
        return Err(());
    }
    Ok(())
}

pub(super) fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), RunError> {
    let arguments = parse_arguments(arguments)?;
    let observer = observer_socket(arguments.observer)?;
    let location = crate::package::Location::fixed_credential(&arguments.credential)?;
    let comment = format!(
        "vivac.device-key.v1:{}:{}",
        arguments.device, arguments.credential
    );
    let super::AttendedPreparation {
        confirmation,
        package,
    } = super::prepare_attended(location, &comment).map_err(|_| RunError::Refused)?;
    let draft = package
        .into_creation_draft()
        .map_err(|_| RunError::Refused)?;
    let record_canonical = serde_json::to_string(draft.record()).map_err(|_| ())?;
    let mut report = serde_json::json!({
        "version": 1,
        "stage": "draft",
        "pid": unsafe { libc::getpid() },
        "record_canonical": record_canonical,
        "attended_confirmation_completed": true,
        "package_published": false,
        "creation_admission": false,
        "custody_accredited": false,
        "producer_origin_checked": false,
        "native_binding_verified": false,
    });
    let packet = serde_json::to_vec(&report).map_err(|_| ())?;
    send_packet(&observer, &packet)?;
    wait_for_validation(&observer)?;
    let valid = draft.validate().is_ok();
    drop(draft);
    drop(confirmation);
    report["stage"] = serde_json::json!("final");
    report["draft_revalidated"] = serde_json::json!(valid);
    report["preparation_dropped"] = serde_json::json!(true);
    let packet = serde_json::to_vec(&report).map_err(|_| ())?;
    send_packet(&observer, &packet)?;
    let mut output = std::io::stdout().lock();
    output.write_all(&packet).map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())?;
    output.flush().map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_arguments;

    const DEVICE: &str = "00000000000000000000000001";
    const CREDENTIAL: &str = "00000000000000000000000002";

    #[test]
    fn accepts_only_the_fixed_root_probe_arguments() {
        let parsed = parse_arguments(
            [DEVICE, CREDENTIAL, "--observer-fd", "3"]
                .into_iter()
                .map(Into::into),
        )
        .unwrap();
        assert_eq!(parsed.device, DEVICE);
        assert_eq!(parsed.credential, CREDENTIAL);
        assert_eq!(parsed.observer, 3);
    }

    #[test]
    fn refuses_extra_flags_invalid_identifiers_and_standard_descriptors() {
        for arguments in [
            vec![DEVICE, CREDENTIAL, "--observer-fd", "0"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "1"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "2"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "-1"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "+3"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "2147483648"],
            vec!["invalid", CREDENTIAL, "--observer-fd", "3"],
            vec![DEVICE, "invalid", "--observer-fd", "3"],
            vec![DEVICE, CREDENTIAL, "--unknown", "3"],
            vec![DEVICE, CREDENTIAL, "--observer-fd"],
            vec![DEVICE, CREDENTIAL, "--observer-fd", "3", "extra"],
            vec![
                DEVICE,
                CREDENTIAL,
                "--observer-fd",
                "3",
                "--observer-fd",
                "4",
            ],
            vec![DEVICE, CREDENTIAL, "--fixed-root-record-draft", "3"],
        ] {
            assert!(parse_arguments(arguments.into_iter().map(Into::into)).is_err());
        }
    }
}
