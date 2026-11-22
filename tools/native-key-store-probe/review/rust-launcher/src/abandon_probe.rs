use crate::linux_spawn::{SpawnOwner, SpawnSpec};
use crate::loader_probe as baseline;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

const CASES: [&str; 3] = ["admission-limit", "pidfd-failure", "owner-abandon"];

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<(String, [RawFd; 3])> {
    let mut arguments = arguments.into_iter();
    baseline::require(arguments.next().as_deref() == Some(std::ffi::OsStr::new("--abandon-case")))?;
    let case = arguments
        .next()
        .ok_or_else(baseline::refused)?
        .into_string()
        .map_err(|_| baseline::refused())?;
    baseline::require(CASES.contains(&case.as_str()))?;
    let mut descriptors = [None; 3];
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or_else(baseline::refused)?;
        let index = ["--observer-fd", "--child-fd", "--marker-fd"]
            .iter()
            .position(|name| option == *name)
            .ok_or_else(baseline::refused)?;
        baseline::require(descriptors[index].is_none())?;
        let descriptor = value
            .into_string()
            .map_err(|_| baseline::refused())?
            .parse::<RawFd>()
            .map_err(|_| baseline::refused())?;
        baseline::require(descriptor >= 3 && !descriptors.contains(&Some(descriptor)))?;
        descriptors[index] = Some(descriptor);
    }
    Ok((
        case,
        descriptors
            .map(|value| value.ok_or_else(baseline::refused))
            .into_iter()
            .collect::<io::Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| baseline::refused())?,
    ))
}

fn errno<T>(result: io::Result<T>, expected: i32) -> io::Result<()> {
    match result {
        Err(error) if error.raw_os_error() == Some(expected) => Ok(()),
        _ => Err(baseline::refused()),
    }
}

fn child(arguments: Vec<OsString>) -> io::Result<()> {
    baseline::require(
        arguments.len() == 3 && arguments[0] == "--abandon-child" && arguments[1] == "--control-fd",
    )?;
    let fd = arguments[2]
        .to_str()
        .ok_or_else(baseline::refused)?
        .parse::<RawFd>()
        .map_err(|_| baseline::refused())?;
    baseline::require(fd >= 3)?;
    let control = baseline::Control::new(baseline::take_inherited(fd)?, false)?;
    // SAFETY: getpid only queries the calling process.
    let pid = unsafe { libc::getpid() };
    control.send(&json!({"stage":"child-ready","pid":pid}))?;
    loop {
        baseline::require(control.command()? == b'P')?;
        control.send(&json!({"stage":"child-alive","pid":pid}))?;
    }
}

fn null() -> io::Result<OwnedFd> {
    Ok(File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into())
}

fn spec(control: OwnedFd, marker: OwnedFd) -> io::Result<SpawnSpec> {
    let inherited_fd = control.as_raw_fd().to_string();
    Ok(SpawnSpec {
        executable: File::open("/proc/self/exe")?.into(),
        arg0: "rust-launcher-probe".into(),
        arguments: vec![
            "--abandon-child".into(),
            "--control-fd".into(),
            inherited_fd.into(),
        ],
        environment: vec![("VIVAC_OPENSSH_DISPOSABLE".into(), "1".into())],
        stdio: [null()?, null()?, null()?],
        inherited: vec![control],
        retained: vec![marker],
        new_session: true,
        controlling_tty: None,
        probe: None,
    })
}

fn send_stage(control: &baseline::Control, report: &Value, stage: &str) -> io::Result<()> {
    let mut packet = report.clone();
    packet["stage"] = stage.into();
    control.send(&packet)
}

pub fn run() -> io::Result<()> {
    baseline::require(std::env::var("VIVAC_OPENSSH_DISPOSABLE").as_deref() == Ok("1"))?;
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    // SAFETY: the alarm bounds only this explicitly disposable helper.
    unsafe { libc::alarm(40) };
    if arguments
        .first()
        .is_some_and(|value| value == "--abandon-child")
    {
        return child(arguments);
    }
    let (case, [observer_fd, child_fd, marker_fd]) = parse(arguments)?;
    let observer = baseline::Control::new(baseline::take_inherited(observer_fd)?, false)?;
    let child = baseline::take_inherited(child_fd)?;
    let marker = baseline::take_inherited(marker_fd)?;
    let mut report = json!({"version":1,"case":case,"status":"observed","custody_accredited":false,
        "product_recovery":false,"close_ack":false,"owners_reserved":1,"fifth_owner_errno":null,
        "readmission_after_release":false,"admission_poisoned":false,"poison_refusals":0,
        "spawn_errno":null,"shutdown_errno":null,"drop_elapsed_ns":0,"marker_retained":null,
        "marker_fd":null,"marker_dev":null,"marker_ino":null,"child_pid":null});
    if case == "admission-limit" {
        let mut owners = Vec::new();
        for _ in 0..4 {
            owners.push(SpawnOwner::new()?);
        }
        errno(SpawnOwner::new(), libc::EBUSY)?;
        drop(owners);
        drop(SpawnOwner::new()?);
        baseline::require(!SpawnOwner::admission_uncertain())?;
        drop(child);
        drop(marker);
        report["owners_reserved"] = 4.into();
        report["fifth_owner_errno"] = libc::EBUSY.into();
        report["readmission_after_release"] = true.into();
        observer.send(&json!({"stage":"admission-ready","case":case}))?;
        baseline::require(observer.command()? == b'G')?;
    } else {
        let marker_number = marker.as_raw_fd();
        let metadata = File::from(baseline::duplicate(marker_number)?).metadata()?;
        let mut owner = SpawnOwner::new()?;
        let child_spec = spec(child, marker)?;
        let receipt = if case == "pidfd-failure" {
            errno(
                owner.spawn_with_pidfd_probe_failure(child_spec),
                libc::EMFILE,
            )?;
            let receipt = owner.receipt(1)?;
            errno(owner.duplicate_pidfd(receipt.token), libc::ENOTSUP)?;
            errno(owner.shutdown(Duration::from_millis(100)), libc::ETIMEDOUT)?;
            report["spawn_errno"] = libc::EMFILE.into();
            report["shutdown_errno"] = libc::ETIMEDOUT.into();
            receipt
        } else {
            owner.spawn(child_spec)?
        };
        baseline::require(owner.try_wait(receipt.token)?.is_none())?;
        observer.send(&json!({"stage":"before-drop","case":case,"pid":receipt.pid,"shutdown_errno":report["shutdown_errno"]}))?;
        baseline::require(observer.command()? == b'G')?;
        if case == "owner-abandon" {
            owner.abandon();
        }
        let started = Instant::now();
        drop(owner);
        let elapsed =
            u64::try_from(started.elapsed().as_nanos()).map_err(|_| baseline::refused())?;
        let retained = match baseline::duplicate(marker_number) {
            Ok(fd) => {
                let actual = File::from(fd).metadata()?;
                baseline::require(
                    actual.dev() == metadata.dev() && actual.ino() == metadata.ino(),
                )?;
                true
            }
            Err(error) if error.raw_os_error() == Some(libc::EBADF) => false,
            Err(error) => return Err(error),
        };
        baseline::require(case != "pidfd-failure" || retained)?;
        baseline::require(SpawnOwner::admission_uncertain())?;
        for _ in 0..3 {
            errno(SpawnOwner::new(), libc::EOWNERDEAD)?;
        }
        report["admission_poisoned"] = true.into();
        report["poison_refusals"] = 3.into();
        report["drop_elapsed_ns"] = elapsed.into();
        report["marker_retained"] = retained.into();
        report["marker_fd"] = marker_number.into();
        report["marker_dev"] = metadata.dev().into();
        report["marker_ino"] = metadata.ino().into();
        report["child_pid"] = receipt.pid.into();
        send_stage(&observer, &report, "drop-returned")?;
        baseline::require(observer.command()? == b'G')?;
    }
    send_stage(&observer, &report, "responsive")?;
    baseline::require(observer.command()? == b'X')?;
    println!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandonment_cli_requires_distinct_non_stdio_descriptors() {
        let good = [
            "--abandon-case",
            "pidfd-failure",
            "--observer-fd",
            "3",
            "--child-fd",
            "4",
            "--marker-fd",
            "5",
        ];
        assert!(parse(good.map(OsString::from)).is_ok());
        for value in ["2", "3", "invalid"] {
            let mut args = good;
            args[5] = value;
            assert!(parse(args.map(OsString::from)).is_err());
        }
        let mut args = good;
        args[1] = "unknown";
        assert!(parse(args.map(OsString::from)).is_err());
        assert!(parse(good[..6].iter().map(OsString::from)).is_err());
    }
}
