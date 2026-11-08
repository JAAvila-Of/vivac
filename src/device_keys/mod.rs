#[cfg(any(windows, target_os = "macos", test))]
mod credential;
#[cfg(any(target_os = "macos", test))]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod macos_password;
mod proof;
#[cfg(windows)]
mod windows;

use crate::args::Args;
use crate::artifacts::files;
use crate::failure::Failure;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::de::DeserializeOwned;
#[cfg(any(windows, target_os = "macos", test))]
use serde::Serialize;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_PUBLIC_INPUT: u64 = 8 * 1024;

pub(crate) const USAGE: &str = "vivac device-key create <absolute-new-directory> [--json]\n\
vivac device-key show <absolute-directory> [--json]\n\
vivac device-key prove <absolute-key-directory> --challenge <absolute-file> [--json]\n\
vivac device-key verify-proof <absolute-file> [--json]\n\
vivac device-key lock <absolute-directory> [--json]\n\
vivac device-key unlock <absolute-directory> [--json]\n\n\
Create or read a protected device key on Windows or macOS.\n\
Creation requires a new directory outside Git and .vivac.\n\
macOS requires a private parent and a terminal passphrase for creation or unlock.\n\
Lock and unlock require macOS; show and prove never prompt or unlock.\n\
Proof verification works on every supported platform.\n\
Public receipts grant no membership or access and do not check challenge use.\n";

#[cfg(any(windows, target_os = "macos", test))]
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
    let allowed: &[&str] = match command {
        "create" | "show" | "verify-proof" | "lock" | "unlock" => &["json"],
        "prove" => &["json", "challenge"],
        _ => {
            return Err(refused(
                "expected create, show, prove, verify-proof, lock or unlock.",
            ))
        }
    };
    if !args.unknown(allowed).is_empty() || !args.extra(2).is_empty() {
        return Err(refused("unexpected arguments or options."));
    }
    let path = absolute_path(args.positional(1))?;
    if command == "verify-proof" {
        let document: proof::Proof = read_public_json(path)?;
        document.verify(proof::current_time()?)?;
        print_proof(&document, "verified", args.has("json"));
        return Ok(0);
    }
    if matches!(command, "lock" | "unlock") {
        #[cfg(not(target_os = "macos"))]
        {
            return Err(refused("explicit keychain locking requires macOS."));
        }
        #[cfg(target_os = "macos")]
        {
            let status = if command == "lock" {
                macos::lock(path)?;
                "locked"
            } else {
                macos::unlock(path)?;
                "unlocked"
            };
            if args.has("json") {
                crate::output::outln!(
                    "{}",
                    serde_json::json!({"status": status, "protection": macos::PROTECTION})
                );
            } else {
                crate::output::outln!("Device key store {status}.");
            }
            return Ok(0);
        }
    }
    let challenge_path = if command == "prove" {
        let values = args.list("challenge");
        if values.len() != 1 {
            return Err(refused("exactly one --challenge file is required."));
        }
        Some(absolute_path(Some(&values[0]))?.to_path_buf())
    } else {
        None
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, challenge_path);
        Err(refused(
            "protected device-key storage requires Windows or macOS.",
        ))
    }
    #[cfg(any(windows, target_os = "macos"))]
    {
        #[cfg(target_os = "macos")]
        use macos as backend;
        #[cfg(windows)]
        use windows as backend;
        if let Some(challenge_path) = challenge_path {
            let challenge: proof::Challenge = read_public_json(&challenge_path)?;
            let now = proof::current_time()?;
            challenge.validate(now)?;
            let document = backend::prove(path, &challenge, now)?;
            document.verify(proof::current_time()?)?;
            print_proof(&document, "created", args.has("json"));
            return Ok(0);
        }
        let receipt = match command {
            "create" => backend::create(path)?,
            "show" => backend::show(path)?,
            _ => unreachable!(),
        };
        if args.has("json") {
            crate::output::outln!("{}", serde_json::json!(receipt));
        } else {
            crate::output::outln!("Device {}", receipt.device_id);
            crate::output::outln!("Credential {}", receipt.credential_id);
            crate::output::outln!("Public key {}", receipt.public_key);
            let protection = match receipt.protection {
                "windows_current_user_dpapi" => "Windows current user",
                "macos_classic_keychain_passphrase" => "macOS classic keychain passphrase",
                _ => unreachable!(),
            };
            crate::output::outln!("Protection: {protection}");
            crate::output::outln!("Status: local key only");
        }
        Ok(0)
    }
}

fn absolute_path(value: Option<&str>) -> Result<&Path, Failure> {
    value
        .filter(|value| !value.is_empty())
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| refused("an explicit absolute path is required."))
}

fn print_proof(document: &proof::Proof, operation: &str, json: bool) {
    if json {
        crate::output::outln!("{}", serde_json::json!(document));
    } else {
        crate::output::outln!("Device enrollment possession proof {operation}.");
        crate::output::outln!("Device {}", document.challenge.device_id);
        crate::output::outln!("Credential {}", document.challenge.credential_id);
        crate::output::outln!("Signature {}", document.signature);
        crate::output::outln!("Authority and challenge use were not checked.");
    }
}

fn read_public_json<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    let bytes = read_public_file(path).map_err(|error| match error {
        Failure::Io(error) => Failure::Io(std::io::Error::new(
            error.kind(),
            "public proof input could not be read safely",
        )),
        other => other,
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| refused("the public proof input is malformed or has unsupported fields."))
}

fn read_public_file(path: &Path) -> Result<Vec<u8>, Failure> {
    let mut anchor = PathBuf::new();
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => anchor.push(component.as_os_str()),
            Component::Normal(name) => names.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(refused("public input cannot use parent traversal."))
            }
        }
    }
    let leaf = names
        .pop()
        .and_then(|name| name.to_str())
        .ok_or_else(|| refused("public input requires a named UTF-8 file."))?;
    let mut directory = Dir::open_ambient_dir(&anchor, cap_std::ambient_authority())?;
    for name in names {
        directory = directory.open_dir_nofollow(name)?;
    }
    let mut file = files::regular(&directory, leaf)?;
    files::require_single_link(&file)?;
    let size = file.metadata()?.len();
    if size > MAX_PUBLIC_INPUT {
        return Err(refused("public proof input exceeds 8 KiB."));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file)
        .take(MAX_PUBLIC_INPUT + 1)
        .read_to_end(&mut bytes)?;
    files::require_single_link(&file)?;
    if bytes.len() as u64 != size || file.metadata()?.len() != size {
        return Err(refused("public proof input changed while being read."));
    }
    Ok(bytes)
}
