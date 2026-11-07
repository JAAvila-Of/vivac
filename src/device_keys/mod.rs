#[cfg(windows)]
mod windows;

use crate::args::Args;
use crate::failure::Failure;
#[cfg(windows)]
use serde::Serialize;
use std::path::Path;

pub(crate) const USAGE: &str = "vivac device-key create <absolute-new-directory> [--json]\n\
vivac device-key show <absolute-directory> [--json]\n\n\
Create or read a private Windows current-user protected device key.\n\
Creation requires a new directory outside Git and .vivac.\n\
The public receipt describes a local key; it grants no membership or access.\n";

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PublicDeviceKey {
    pub device_id: String,
    pub credential_id: String,
    pub public_key: String,
    pub protection: &'static str,
    pub status: &'static str,
}

fn refused(message: &str) -> Failure {
    Failure::usage(format!("Device key refused: {message}"))
}

pub(crate) fn run(args: &Args) -> Result<i32, Failure> {
    if args.has("help") {
        if !args.unknown(&["help"]).is_empty()
            || !args.extra(1).is_empty()
            || !args.list("help").is_empty()
        {
            return Err(refused("unexpected arguments with --help."));
        }
        crate::output::outln!("{USAGE}");
        return Ok(0);
    }
    let command = args.positional(0).unwrap_or_default();
    if !matches!(command, "create" | "show") {
        return Err(refused("expected create or show."));
    }
    if !args.unknown(&["json"]).is_empty() || !args.extra(2).is_empty() {
        return Err(refused("unexpected arguments or options."));
    }
    let path = args
        .positional(1)
        .filter(|path| !path.is_empty())
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| refused("an explicit absolute directory is required."))?;
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(refused("protected device-key storage requires Windows."))
    }
    #[cfg(windows)]
    {
        let receipt = match command {
            "create" => windows::create(path)?,
            "show" => windows::show(path)?,
            _ => unreachable!(),
        };
        if args.has("json") {
            crate::output::outln!("{}", serde_json::json!(receipt));
        } else {
            crate::output::outln!("Device {}", receipt.device_id);
            crate::output::outln!("Credential {}", receipt.credential_id);
            crate::output::outln!("Public key {}", receipt.public_key);
            crate::output::outln!("Protection: Windows current user");
            crate::output::outln!("Status: local key only");
        }
        Ok(0)
    }
}
