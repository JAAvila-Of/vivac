use super::deliver_linux_reply;
use crate::linux_resident_reader;
use crate::linux_session_opening::{self, OpeningOutcome, RefusalHint};
use crate::linux_supervisor_start;
use crate::linux_user_manager::SessionLimits;
use std::ffi::OsString;
use std::io::{self, Write};

pub(super) const USAGE: &str = "vivac device-key open-linux <credential-id> [--json]\n\
vivac device-key verify-linux-possession <credential-id> [--json]\n\
Open attended Linux custody or verify fresh local possession by credential ID.";

fn parse(arguments: impl Iterator<Item = OsString>) -> Option<(String, bool)> {
    let mut credential = None;
    let mut json = false;
    for argument in arguments {
        if argument == "--json" {
            if json {
                return None;
            }
            json = true;
        } else {
            let value = argument.into_string().ok()?;
            if credential.is_some() || !linux_resident_reader::identifier(&value) {
                return None;
            }
            credential = Some(value);
        }
    }
    Some((credential?, json))
}

fn opening_result(
    credential: &str,
    limits: SessionLimits,
) -> (&'static str, i32, Option<RefusalHint>) {
    match linux_session_opening::open(credential, limits) {
        OpeningOutcome::Open => ("open", 0, None),
        OpeningOutcome::Reopened => ("reopened", 0, None),
        OpeningOutcome::Refused(hint) => ("refused", 1, hint),
        OpeningOutcome::OpeningUncertain => ("opening_uncertain", 5, None),
        OpeningOutcome::Interrupted => ("interrupted", 5, None),
        OpeningOutcome::PublicationUncertain => ("publication_uncertain", 5, None),
    }
}

/// Writes the bounded receipt, then the remedy of a refusal, if it has one,
/// as one line on stderr whether or not the receipt is JSON.
fn report(credential: &str, status: &str, code: i32, json: bool, hint: Option<RefusalHint>) -> i32 {
    let output = if json {
        serde_json::json!({"credential_id": credential, "status": status}).to_string()
    } else {
        format!("Linux custody {status} for credential {credential}.")
    };
    let delivered = if json || code == 0 {
        deliver_linux_reply(output)
    } else {
        let mut stderr = io::stderr().lock();
        writeln!(stderr, "{output}").and_then(|()| stderr.flush())
    };
    if let Some(hint) = hint {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{}", hint.remedy(credential)).and_then(|()| stderr.flush());
    }
    if delivered.is_err() {
        let _ = io::stderr()
            .lock()
            .write_all(b"Linux custody receipt delivery failed.\n");
        return 5;
    }
    code
}

pub(crate) fn dispatch() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next()?.as_os_str() != "device-key" {
        return None;
    }
    let command = arguments.next()?;
    let opening = match command.to_str() {
        Some("open-linux") => true,
        Some("verify-linux-possession") => false,
        Some(_) => return None,
        None => {
            let _ = io::stderr()
                .lock()
                .write_all(b"Specify a valid device-key command.\n");
            return Some(2);
        }
    };
    let Some((credential, json)) = parse(arguments) else {
        let _ = io::stderr()
            .lock()
            .write_all(b"Specify one canonical credential ID and at most one --json flag.\n");
        return Some(2);
    };
    let (status, code, hint) = if opening {
        let Ok(limits) = SessionLimits::from_environment() else {
            let _ = io::stderr()
                .lock()
                .write_all(b"Session limits can only be shortened, to no less than 60 seconds.\n");
            return Some(2);
        };
        opening_result(&credential, limits)
    } else if linux_supervisor_start::verify_discovered_endpoint(&credential).is_ok() {
        ("possession_verified", 0, None)
    } else {
        ("refused", 1, None)
    };
    Some(report(&credential, status, code, json, hint))
}
