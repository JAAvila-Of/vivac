use crate::fixed_root_probe::{finish, identity, scoped, Channel};
use crate::linux_resident_reader::CandidateResident;
use crate::loader_probe as baseline;
use crate::probe::Pressure;
use serde_json::json;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::RawFd;

const CREDENTIAL: &str = "00000000000000000000000004";
const CASES: [&str; 23] = [
    "resident-valid",
    "resident-unrelated",
    "missing-record",
    "missing-package",
    "experimental-record",
    "installation-mismatch",
    "credential-mismatch",
    "manifest-mismatch",
    "ciphertext-mismatch",
    "record-in-place",
    "record-replacement",
    "package-replacement",
    "key-in-place",
    "key-replacement",
    "manifest-replacement",
    "mount-record-slot",
    "mount-record-leaf",
    "mount-package-slot",
    "mount-key-leaf",
    "mount-manifest-leaf",
    "symlink-record",
    "hardlink-key",
    "acl-record",
];
const MUTATIONS: [&str; 6] = [
    "record-in-place",
    "record-replacement",
    "package-replacement",
    "key-in-place",
    "key-replacement",
    "manifest-replacement",
];

fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<(String, RawFd)> {
    let mut args = args.into_iter();
    baseline::require(args.next().as_deref() == Some(OsStr::new("--resident-case")))?;
    let case = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(CASES.contains(&case.as_str()))?;
    baseline::require(args.next().as_deref() == Some(OsStr::new("--observer-fd")))?;
    let text = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(baseline::refused)?;
    baseline::require(!text.is_empty() && text.bytes().all(|value| value.is_ascii_digit()))?;
    let descriptor = text.parse::<RawFd>().map_err(|_| baseline::refused())?;
    baseline::require(descriptor >= 3 && args.next().is_none())?;
    Ok((case, descriptor))
}

fn expected(case: &str) -> Option<(io::ErrorKind, &'static str)> {
    match case {
        "missing-record" | "missing-package" => Some((io::ErrorKind::NotFound, "not-found")),
        "experimental-record"
        | "installation-mismatch"
        | "credential-mismatch"
        | "manifest-mismatch"
        | "ciphertext-mismatch"
        | "hardlink-key"
        | "acl-record" => Some((io::ErrorKind::InvalidData, "invalid-data")),
        "mount-record-slot"
        | "mount-record-leaf"
        | "mount-package-slot"
        | "mount-key-leaf"
        | "mount-manifest-leaf" => Some((
            io::Error::from_raw_os_error(libc::EXDEV).kind(),
            "crosses-devices",
        )),
        "symlink-record" => Some((io::Error::from_raw_os_error(libc::ELOOP).kind(), "symlink")),
        _ => None,
    }
}

fn reader(case: &str, observer: &Channel) -> io::Result<()> {
    let pressure = Pressure::start()?;
    let identity = identity()?;
    observer.send(&json!({"stage":"before","case":case,"identity":identity}))?;
    observer.go()?;
    let mut result = scoped(
        json!({"version":1,"case":case,"status":"observed","identity":identity,
        "initial_opened":false,"initial_validated":false,"selected_installation_id":null,
        "selected_credential_id":null,"initial_error":"none","snapshot_invalidated":false,
        "validation_error":"none","resources_closed_after_exit":false,
        "pressure_threads":4,"pressure_iterations":0}),
    );
    let candidate = match CandidateResident::open_fixed_namespace(CREDENTIAL) {
        Ok(value) => {
            baseline::require(expected(case).is_none())?;
            value.validate()?;
            result["initial_opened"] = true.into();
            result["initial_validated"] = true.into();
            result["selected_installation_id"] = value.record().installation_id.clone().into();
            result["selected_credential_id"] = value.record().credential_id.clone().into();
            Some(value)
        }
        Err(error) => {
            let Some((kind, label)) = expected(case) else {
                return Err(error);
            };
            baseline::require(error.kind() == kind)?;
            result["initial_error"] = label.into();
            None
        }
    };
    observer.send(&json!({"stage":"snapshot-held","payload":result}))?;
    observer.go()?;
    if let Some(value) = &candidate {
        if MUTATIONS.contains(&case) {
            let error = value.validate().err().ok_or_else(baseline::refused)?;
            baseline::require(error.kind() == io::ErrorKind::InvalidData)?;
            result["snapshot_invalidated"] = true.into();
            result["validation_error"] = "invalid-data".into();
        } else {
            value.validate()?;
        }
        baseline::require(result["selected_installation_id"] == value.record().installation_id)?;
        baseline::require(result["selected_credential_id"] == value.record().credential_id)?;
    }
    observer.send(&json!({"stage":"observation","payload":result}))?;
    observer.go()?;
    drop(candidate);
    finish(observer, result, pressure)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    // SAFETY: only the explicitly disposable probe process is bounded by this alarm.
    unsafe { libc::alarm(120) };
    let (case, descriptor) = parse(std::env::args_os().skip(1))?;
    let observer = Channel::new(baseline::take_inherited(descriptor)?)?;
    let outcome = reader(&case, &observer);
    if let Err(error) = &outcome {
        let diagnostic = error.to_string();
        baseline::require(diagnostic.len() <= 512 && diagnostic.is_ascii())?;
        observer.send(&json!({"stage":"reader-refusal","diagnostic":diagnostic}))?;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_cli_accepts_only_known_cases_and_one_observer_without_location_arguments() {
        for case in CASES {
            assert_eq!(
                parse(["--resident-case", case, "--observer-fd", "3"].map(OsString::from)).unwrap(),
                (case.into(), 3)
            );
        }
        for arguments in [
            vec!["--resident-case", "unknown", "--observer-fd", "3"],
            vec!["--resident-case", "resident-valid", "--observer-fd", "2"],
            vec!["--resident-case", "resident-valid", "--observer-fd", "-1"],
            vec![
                "--resident-case",
                "resident-valid",
                "--observer-fd",
                "2147483648",
            ],
            vec![
                "--resident-case",
                "resident-valid",
                "--observer-fd",
                "3",
                "--root",
                "/decoy",
            ],
            vec![
                "--resident-case",
                "resident-valid",
                "--observer-fd",
                "3",
                "--expected-installation-id",
                CREDENTIAL,
            ],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
    }
}
