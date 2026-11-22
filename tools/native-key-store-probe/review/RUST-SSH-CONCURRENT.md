# Shared Rust session during SSH disconnect

This fixture composes the [shared session adapter](RUST-SESSION.md),
[signing gate](RUST-CONCURRENT.md) and real SSH transport. It exercises active
signing and transport termination in the same supervisor. This is a prerequisite
for the self-hosted team service; it does not provide that server or enable
operational Linux custody.

## Lifetime and closure boundary

`src/device_keys/linux_ssh_lifetime.rs` contains the detector previously held in
the loader fixture. It owns a CLOEXEC duplicate of SSH stdin and records observed
SIGHUP, read EOF or terminal EIO. The signal handler only marks an atomic flag.
Cause is sticky; unexpected data and invalid descriptors refuse. A process may
claim only one monitor, including after Drop. Drop restores the prior signal
disposition, without clearing the recorded signal or permitting recreation.
This is one SSH lifetime in one supervisor process, not a multi-client daemon
implementation.

The disposable bootstrap transfers six capabilities through SCM_RIGHTS and execs
the helper in the same PID. The observer verifies peer credentials, PID/start,
executable inode, ancestry to its private sshd and actual PTY/pipe mode. Ciphertext
and public fixture metadata are sealed; the generated unlock secret travels by
anonymous pipe and the loader's separate private terminal. SSH stdin carries
only connection lifetime, never that secret.

At a selected signing seam the worker remains paused. The observer kills only
the owned SSH client. On an observed disconnect, the coordinator starts a caller
of the shared gate's lock while the creator thread continues servicing the
shared native adapter. The gate closes admission and observes native stop, then
waits for the active worker. The observer requires Closing, active work, no ACK,
closed late admission and native exits before releasing the worker's barrier.

The barrier release uses a private local control packet after SSH has ended.
That path cannot inject a disconnect or authorize a new signing operation or
readiness. The worker checks epoch and lease after release; its uncommitted
result is cancelled. Worker join, lease release and acknowledged CLOSED retain
the earlier ordering. The final result travels only on this independent control
channel; the SSH mode prints no stdout after losing its transport.

A published control follows a separate path. The observer verifies the published
proof, then triggers normal close or transport loss. The publication remains
published. Lock does not undo an already committed proof or a native computation.
No observation here establishes delivery to a future remote authority.

## Observed matrix on 2026-11-12

Each row passed once with a remote PTY and once with a pipe:

| Selected trigger | Sent / reply / verified / published | Required outcome |
|---|---|---|
| Normal close after publication | 1 / 1 / 1 / 1 | Published, then observed closure |
| Disconnect before native send | 0 / 0 / 0 / 0 | Cancelled by observed SSH loss |
| Disconnect with native reply pending | 1 / 0 / 0 / 0 | Cancelled before consuming reply |
| Disconnect after native reply | 1 / 1 / 0 / 0 | Reply withheld from publication |
| Disconnect before publication | 1 / 1 / 1 / 0 | Verified result withheld |
| Disconnect after publication | 1 / 1 / 1 / 1 | Publication preserved, then closure |

All twelve cases passed on Ubuntu 24.04 amd64 with OpenSSH
`1:9.6p1-3ubuntu13.20`. Among the ten disconnects, the monitor observed one
SIGHUP and nine EOF causes. A PTY does not require a particular cause label;
the recorded supported cause is retained. EIO has unit coverage without a
native EIO observation in this run. Each case observed one joined worker, one
close acknowledgement, expected agent SIGKILL and loader exit 0, supervisor
exit, retained/new connection refusal and a sentinel signature afterward.
All eight uncommitted requests were withheld from publication.

The observer preserves pidfds, master and package lease through native exit.
Uncertainty retains references and prevents a successful report. Reports contain
metadata and counters, without secret bytes, terminal transcripts or signatures.
Public proof verification uses the original separate Rust verifier.

The final offline Rust 1.89 build passed 52 unit tests, fmt, Clippy
development/release and build after clean. Python checks passed 286 tests with
one POSIX-only Windows skip. The same helper passed the ten existing
loader-over-SSH cases and the seven local concurrency cases. All three native
runs exited 0 without OOM or selected RDP error events and preserved their
sentinels. Those regressions were repeated because their control/parser code is shared.
The full product CI matrix, Cargo-deny and full-flow performance are outside
this increment.

## Reproduction and limits

Use the SSH-enabled local Ubuntu image, original writer/proof verifier volume,
a separate launcher build volume and a new absolute report directory. On the
shared Windows host, run through RustWork with `--family rust`:

```text
python run_lifecycle_probe.py --experiment rust-ssh-concurrent --image LOCAL_SSH_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runtime retains CPU1, RAM512 MiB without additional swap, PID128, bounded
tmpfs, NNP, unprivileged execution, read-only helpers and external-network
isolation. SSH is confined to container loopback. The run exited 0 without OOM
or selected RDP error events. This does not measure input responsiveness or
identify the cause of an earlier RDP failure.

These selected barriers do not accredit every interleaving, instantaneous
disconnect detection, silent network partitions, automatic recovery or a
complete product backend. Earlier pidfd acquisition, abandoned custody, terminal
permission-transition, admission and cooperative-deadline limits remain. The
next adoption step must review those remaining conditions explicitly.
