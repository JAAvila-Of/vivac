# Candidate Rust loader across real SSH disconnect

This extends the [Rust loader fixture](RUST-LOADER.md) with an actual isolated
OpenSSH connection. The Rust helper runs inside that connection and observes its
stdin and SIGHUP directly. Killing a local supervisor alone is not credited as
an SSH disconnect. Operational Linux custody remains unavailable.

## Transport and ownership

The existing disposable SSH transport starts a private sshd on container
loopback, with forwarding disabled, a pinned fixture host key and an ephemeral
transport client key. It changes no host account, SSH, RDP or VPN setting. The
container has no external network interface. The modes use `ssh -tt` and
`ssh -T`; the observer checks the actual remote stdin kind in both modes.

A short Python bootstrap connects to a private local SOCK_SEQPACKET socket. The
observer verifies its peer credentials, PID/start identity and ancestry back to
the owned SSH server before transferring exactly five descriptors with
SCM_RIGHTS: admitted agent and loader executables, sealed ciphertext, anonymous
secret pipe and sealed public fixture metadata. The bootstrap checks transfer
shape, truncation, credentials and CLOEXEC, then execs the helper in the same
PID. The observer subsequently verifies the Rust executable device/inode,
unchanged PID/start/parent and SSH ancestry. The secret is never sent through
SSH stdin, argv, environment, a plaintext file or a report.

With the explicit `--ssh-lifetime stdin` option, a SIGHUP handler only stores an
atomic flag. The helper polls SSH stdin and its independent control socket in
bounded intervals. It records SIGHUP, read EOF or terminal EIO; unexpected input
or invalid descriptors fail the experiment. SSH termination takes priority over
a control command, with checks again before handoff and READY. The checks and
side effects are not atomic against every possible signal interleaving; this
matrix exercises the selected, externally held seams.

The external observer kills only its own SSH client at the selected seam. Rust
must observe the transport cause, close native admission, shut down its spawn
owner, observe both native exits and release its local resources before sending
the close acknowledgement. A final packet carries the cause and pressure
counter over the independent local control socket. The helper emits no stdout
in this mode, so reporting does not depend on the disconnected SSH channel.
The observer retains pidfds, package lease, PTY master and agent connection
through native exit, then observes supervisor exit and connection refusal.

## Observed matrix on 2026-11-12

Each row ran once with a remote PTY and once without one:

| Selected seam | Trigger | Secret handoffs | Proof before trigger | READY published |
|---|---|---:|---|---|
| READY | Normal close command | 1 | Verified independently | Yes |
| Prompt | SSH client SIGKILL | 0 | No | No |
| Stopped handoff | SSH client SIGKILL | 1 | No | No |
| Before READY | SSH client SIGKILL | 1 | Verified independently | No |
| READY | SSH client SIGKILL | 1 | Verified independently | Yes |

All ten cases passed on Ubuntu 24.04 amd64 with OpenSSH package
`1:9.6p1-3ubuntu13.20`. The four PTY disconnects produced observed SIGHUP; the
four pipe disconnects produced observed EOF. EIO remains a supported refusal/
disconnect classification, without a native EIO observation in this run.
Every case acknowledged closure after native exits, observed supervisor exit,
checked private loader slave mode/UID, revalidated leases and refused retained
and new agent connections. The independent loaded sentinel signed and passed
the original proof verifier after every case. No secret or terminal transcript
is exported; public signatures remain on the control/proof verification path.

The helper passed 18 Rust unit tests, fmt, Clippy development/release and build
offline on Rust 1.89, after package clean. Python verification passed 270 tests
with one POSIX-only case skipped on Windows. These checks do not replace the
complete product CI matrix or Cargo-deny. The same helper also passed the eight
local loader cases, preserving the mode without SSH monitoring. The runtime used CPU1, 512 MiB memory
without additional swap, PID128, bounded tmpfs, unprivileged credentials, NNP and
read-only helper mounts. The SSH run exited 0 without OOM or selected RDP error
events. This does not identify the cause of an earlier RDP failure.

## Reproduction and limits

Use the local SSH-enabled Ubuntu image, original writer/proof verifier volume
and a separate launcher build volume. Run through RustWork with `--family rust`
and a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment rust-ssh-loader --image LOCAL_SSH_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

An uncertain exit keeps the report incomplete and retains resource references
until external containment ends. This experiment does not accredit silent
network partitions, every cancellation interleaving, a complete shared signing
session, full-flow performance or operational credential custody. The spawn
owner's pidfd-failure limitation and the PTY permission-transition limitation
documented in the earlier fixtures remain applicable.
