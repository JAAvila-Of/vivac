use super::deliver_linux_reply;
use crate::args::Args;
use crate::failure::Failure;
use crate::linux_custody_provision::{ProvisionError, Provisioner};
use crate::linux_custody_root::{
    optional_collection::{CollectionProvisionError, ENDPOINTS, SESSIONS},
    CandidateRoot,
};
use crate::linux_user_manager::current_account;
use crate::linux_user_manager::linger_enabled;
use serde::Serialize;
use std::io;

pub(super) const USAGE: &str =
    "vivac device-key provision-linux --uid <uid> --gid <gid> [--yes] [--json]\n\
Preview the fixed Linux custody installation for a numeric account.\n\
Pass --yes after reviewing the plan to provision the account as an administrator.";

pub(super) const SESSIONS_USAGE: &str =
    "vivac device-key provision-linux-sessions [--yes] [--json]\n\
Preview the session directory for the current provisioned Linux account.\n\
Pass --yes to add the directory without elevation. Closing and reopening a key\n\
session also needs lingering enabled: loginctl enable-linger <account>.";

const USERS: &str = "/var/lib/vivac/custody-v1/users";

pub(super) const ENDPOINTS_USAGE: &str =
    "vivac device-key provision-linux-endpoints [--yes] [--json]\n\
Preview the endpoint directory for the current provisioned Linux account.\n\
Pass --yes to add the directory without elevation.";

#[derive(Serialize)]
struct Entry {
    path: String,
    uid: u32,
    gid: u32,
    mode: &'static str,
    action: &'static str,
}

fn selector(args: &Args, name: &str) -> Result<u32, Failure> {
    let values = args.list(name);
    let invalid = || {
        Failure::usage(format!(
            "Specify --{name} once as a canonical decimal value from 1 to 4294967294."
        ))
    };
    if values.len() != 1 || args.has_missing_value(name) {
        return Err(invalid());
    }
    let value = values[0].parse::<u32>().map_err(|_| invalid())?;
    if value == 0 || value == u32::MAX || value.to_string() != values[0] {
        return Err(invalid());
    }
    Ok(value)
}

fn plan(uid: u32, gid: u32) -> Vec<Entry> {
    let account = format!("{USERS}/{uid}");
    let mut entries = Vec::new();
    for path in ["/var/lib/vivac", "/var/lib/vivac/custody-v1", USERS] {
        entries.push(Entry {
            path: path.into(),
            uid: 0,
            gid: 0,
            mode: "0755",
            action: "validate-or-create",
        });
    }
    for (suffix, owner, group, mode) in [
        ("", 0, 0, "0755"),
        ("/binding.json", 0, 0, "0644"),
        ("/state", uid, gid, "0700"),
        ("/state/packages", uid, gid, "0700"),
        ("/state/admissions", uid, gid, "0700"),
        ("/state/openings-v1", uid, gid, "0700"),
        ("/state/endpoints-v1", uid, gid, "0700"),
        ("/state/sessions-v1", uid, gid, "0700"),
    ] {
        entries.push(Entry {
            path: format!("{account}{suffix}"),
            uid: owner,
            gid: group,
            mode,
            action: "create",
        });
    }
    entries
}

pub(super) fn run(args: &Args) -> Result<i32, Failure> {
    if !args.unknown(&["uid", "gid", "yes", "json"]).is_empty() || !args.extra(1).is_empty() {
        return Err(Failure::usage("Unexpected Linux provisioning arguments."));
    }
    let uid = selector(args, "uid")?;
    let gid = selector(args, "gid")?;
    if !args.has("yes") {
        let entries = plan(uid, gid);
        let output = if args.has("json") {
            serde_json::json!({"status": "plan", "uid": uid, "gid": gid, "entries": entries})
                .to_string()
        } else {
            let mut lines = vec![format!("Linux custody plan for UID {uid} and GID {gid}.")];
            lines.extend(entries.iter().map(|entry| {
                format!(
                    "  {} {} ({}:{}, mode {})",
                    entry.action, entry.path, entry.uid, entry.gid, entry.mode
                )
            }));
            lines.push("Review these paths and rerun as an administrator with --yes to provision the account.".into());
            lines.join("\n")
        };
        deliver_linux_reply(output).map_err(Failure::Io)?;
        return Ok(0);
    }
    let provisioner = Provisioner::open_fixed_namespace()
        .map_err(|error| Failure::Model(ProvisionError::Unpublished(error).to_string()))?;
    let publication = provisioner
        .provision(uid, gid)
        .map_err(|error| match error {
            ProvisionError::Unpublished(_) => Failure::Model(error.to_string()),
            ProvisionError::PublishedDurabilityUnknown(_) => {
                Failure::Io(io::Error::other(error.to_string()))
            }
        })?;
    let slot = format!("{USERS}/{uid}");
    let output = if args.has("json") {
        serde_json::json!({"status": "durable", "slot": slot, "binding": publication.binding})
            .to_string()
    } else {
        format!("Provisioned Linux custody for UID {uid} and GID {gid} at {slot}.")
    };
    deliver_linux_reply(output).map_err(|error| {
        Failure::Io(io::Error::other(format!(
            "Installation was published but its confirmation could not be delivered: {error}"
        )))
    })?;
    Ok(0)
}

pub(super) fn run_endpoints(args: &Args) -> Result<i32, Failure> {
    if !args.unknown(&["yes", "json"]).is_empty() || !args.extra(1).is_empty() {
        return Err(Failure::usage(
            "Unexpected Linux endpoint provisioning arguments.",
        ));
    }
    let root = CandidateRoot::open_fixed_namespace().map_err(|error| {
        Failure::Model(CollectionProvisionError::Unpublished(error).message(&ENDPOINTS))
    })?;
    let binding = root.binding();
    let path = format!("{USERS}/{}/state/endpoints-v1", binding.owner_uid);
    if !args.has("yes") {
        let exists = root
            .endpoint_directory()
            .map_err(|error| {
                Failure::Model(CollectionProvisionError::Unpublished(error).message(&ENDPOINTS))
            })?
            .is_some();
        let output = if args.has("json") {
            serde_json::json!({"status": "plan", "exists": exists, "entry": Entry {
                path,
                uid: binding.owner_uid,
                gid: binding.owner_gid,
                mode: "0700",
                action: "create",
            }})
            .to_string()
        } else if exists {
            format!("The Linux endpoint directory already exists at {path}.")
        } else {
            format!(
                "Create {path} ({}:{}, mode 0700). Rerun with --yes to publish the directory.",
                binding.owner_uid, binding.owner_gid
            )
        };
        deliver_linux_reply(output).map_err(Failure::Io)?;
        return Ok(0);
    }
    root.provision_endpoint_directory()
        .map_err(|error| match error {
            CollectionProvisionError::Unpublished(_) => Failure::Model(error.message(&ENDPOINTS)),
            CollectionProvisionError::PublishedDurabilityUnknown(_) => {
                Failure::Io(io::Error::other(error.message(&ENDPOINTS)))
            }
        })?;
    let output = if args.has("json") {
        serde_json::json!({"status": "durable", "path": path}).to_string()
    } else {
        format!("Provisioned the Linux endpoint directory at {path}.")
    };
    deliver_linux_reply(output).map_err(|error| {
        Failure::Io(io::Error::other(format!(
            "Endpoint directory was published but its confirmation could not be delivered: {error}"
        )))
    })?;
    Ok(0)
}

pub(super) fn run_sessions(args: &Args) -> Result<i32, Failure> {
    if !args.unknown(&["yes", "json"]).is_empty() || !args.extra(1).is_empty() {
        return Err(Failure::usage(
            "Unexpected Linux session provisioning arguments.",
        ));
    }
    let unpublished =
        |error| Failure::Model(CollectionProvisionError::Unpublished(error).message(&SESSIONS));
    let root = CandidateRoot::open_fixed_namespace().map_err(unpublished)?;
    let binding = root.binding();
    let path = format!("{USERS}/{}/state/sessions-v1", binding.owner_uid);
    let account = current_account().map_err(Failure::Io)?;
    let name = account.name.to_string_lossy().into_owned();
    let linger = linger_enabled(&account).map_err(Failure::Io)?;
    if !args.has("yes") {
        let exists = root.session_directory().map_err(unpublished)?.is_some();
        let output = if args.has("json") {
            serde_json::json!({"status": "plan", "exists": exists, "linger": linger,
                "account": name, "entry": Entry {
                path,
                uid: binding.owner_uid,
                gid: binding.owner_gid,
                mode: "0700",
                action: "create",
            }})
            .to_string()
        } else {
            let mut text = if exists {
                format!("The Linux session directory already exists at {path}.")
            } else {
                format!(
                    "Create {path} ({}:{}, mode 0700). Rerun with --yes to publish the directory.",
                    binding.owner_uid, binding.owner_gid
                )
            };
            if !linger {
                text.push_str(&format!(
                    "\nLingering is off for {name}. An administrator can enable it with: loginctl enable-linger {name}"
                ));
            }
            text
        };
        deliver_linux_reply(output).map_err(Failure::Io)?;
        return Ok(0);
    }
    root.provision_session_directory()
        .map_err(|error| match error {
            CollectionProvisionError::Unpublished(_) => Failure::Model(error.message(&SESSIONS)),
            CollectionProvisionError::PublishedDurabilityUnknown(_) => {
                Failure::Io(io::Error::other(error.message(&SESSIONS)))
            }
        })?;
    let output = if args.has("json") {
        serde_json::json!({"status": "durable", "path": path, "linger": linger, "account": name})
            .to_string()
    } else if linger {
        format!("Provisioned the Linux session directory at {path}.")
    } else {
        format!(
            "Provisioned the Linux session directory at {path}.\nLingering is off for {name}. An administrator can enable it with: loginctl enable-linger {name}"
        )
    };
    deliver_linux_reply(output).map_err(|error| {
        Failure::Io(io::Error::other(format!(
            "Session directory was published but its confirmation could not be delivered: {error}"
        )))
    })?;
    Ok(0)
}
