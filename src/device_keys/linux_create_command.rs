use super::{deliver_linux_reply, PublicDeviceKey};
use crate::args::Args;
use crate::failure::Failure;
use crate::linux_creation_producer::{self, PublicationOutcome};
use crate::package::CreateError;
use std::io;

pub(super) const USAGE: &str = "vivac device-key create-linux [--json]\n\
Create a local key for the current Linux account in its provisioned custody store.\n\
Creation requires a trusted foreground terminal to confirm a generated unlock secret.";

fn identifier() -> Result<String, Failure> {
    let mut bytes = [0u8; 16];
    // SAFETY: the fixed buffer is writable for the exact requested length.
    let received =
        unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), libc::GRND_NONBLOCK) };
    if received != bytes.len() as isize {
        return Err(Failure::Io(io::Error::other(
            "Secure randomness is unavailable for a Linux device key.",
        )));
    }
    Ok(crate::id::encode(u128::from_be_bytes(bytes)))
}

fn refused() -> Failure {
    Failure::Model("Linux device key creation refused. Creation requires a complete provisioned account, the verified Ubuntu OpenSSH provider and a trusted foreground terminal.".into())
}

fn partial(outcome: PublicationOutcome, credential: &str) -> Failure {
    Failure::Io(io::Error::other(format!(
        "Linux device key outcome {} for credential {credential}.",
        outcome.as_str()
    )))
}

pub(super) fn run(args: &Args) -> Result<i32, Failure> {
    if !args.unknown(&["json"]).is_empty() || !args.extra(1).is_empty() {
        return Err(Failure::usage(
            "Unexpected Linux device-key creation arguments.",
        ));
    }
    let device = identifier()?;
    let credential = identifier()?;
    let result = linux_creation_producer::create_attended(&device, &credential).map_err(
        |error| match error {
            CreateError::Refused => refused(),
            CreateError::CommittedIndeterminate => {
                partial(PublicationOutcome::PackageDurabilityUnknown, &credential)
            }
        },
    )?;
    let record = match result.outcome() {
        PublicationOutcome::Durable => result
            .record()
            .map_err(|_| partial(PublicationOutcome::RecordDurabilityUnknown, &credential))?,
        PublicationOutcome::Unpublished => return Err(refused()),
        outcome => return Err(partial(outcome, &credential)),
    };
    let receipt = PublicDeviceKey {
        device_id: record.device_id.clone(),
        credential_id: record.credential_id.clone(),
        public_key: record.public_key.clone(),
        protection: "linux_openssh",
        status: "local_key_only",
    };
    let delivery_error = |error| {
        Failure::Io(io::Error::other(format!("Linux device key {credential} was created but its receipt could not be delivered: {error}")))
    };
    let output = if args.has("json") {
        serde_json::to_string(&receipt).map_err(|error| delivery_error(error.to_string()))?
    } else {
        format!("Device {}\nCredential {}\nPublic key {}\nProtection Linux OpenSSH\nStatus local key only", receipt.device_id, receipt.credential_id, receipt.public_key)
    };
    deliver_linux_reply(output).map_err(|error| delivery_error(error.to_string()))?;
    Ok(0)
}
