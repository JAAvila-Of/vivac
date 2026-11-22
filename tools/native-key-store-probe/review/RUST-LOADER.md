# Candidate Rust loader and cancellation fixture

This disposable fixture compiles the shared candidate
`src/device_keys/linux_loader.rs` with the [Rust spawn owner](RUST-LAUNCHER.md).
Both product declarations remain Linux-test-only. Passing this fixture does not
enable Linux credential custody or accredit the complete product session.

## Capability and handoff boundary

The caller admits the retained native executable and encrypted package before
dispatch. The loader accepts a bounded regular ciphertext memfd with WRITE,
GROW, SHRINK and SEAL seals. Only this encrypted descriptor is inherited beyond
the private terminal and standard streams. Noninherited master descriptors stay
owned by the spawn owner until the native exit is observed.

The parent opens the master with CLOEXEC and obtains the slave with
`TIOCGPTPEER`, also with CLOEXEC. Before spawn it sets the slave to mode `0600`
and checks its character-device type, effective UID and exact permissions. It
never changes the shared ptmx node. The native loader gets a new session and
controlling terminal. At the prompt and handoff, the candidate checks executable
device/inode, PID/parent, all native UID/GID fields, NNP, terminal session and
foreground process, and disabled echo/ECHONL.

The mode change excludes subsequent ordinary group access. It does not revoke
a descriptor opened between terminal creation and chmod, nor isolate against a
privileged concurrent actor. The OS and administrator remain trusted; this
fixture does not prove an atomic private-permission transition at creation.

After a complete native passphrase prompt, the owner stops the loader through
its pidfd and observes the stop with `waitid(WNOWAIT)` without consuming its exit.
One logical handoff queues a bounded secret and newline. Partial kernel writes
may complete that handoff, but a failed or partial handoff cannot be retried as a
second submission. Rust payload/output buffers use `Zeroizing`; this is not a
claim to erase all copies in Python, the kernel or OpenSSH. The secret arrives
at the test helper through an anonymous pipe, never argv, environment, reports
or a plaintext file. Terminal output stays bounded and is not exported.

The owner resumes the loader and observes its successful exit. The helper then
requires exactly the expected agent identity and comment, signs a fresh bound
challenge, and verifies the Ed25519 signature. A separate retained Rust verifier
checks the public proof before the observer permits test READY. Cancellation
closes admission and observes both native exits before releasing local leases
and acknowledging closure. A killed supervisor cannot acknowledge closure; the
external observer instead retains pidfds, a master and a connection until it
observes the native processes stop.

## Observed matrix on 2026-11-12

| Case | Required observation |
|---|---|
| Normal | One handoff, loader exit 0, exact identity, independent proof, then READY and observed shutdown |
| Cancel at prompt | No handoff or READY; both native exits before close acknowledgement |
| Cancel at stopped handoff | One queued handoff; no READY; both exits before acknowledgement |
| Cancel before READY | Successful loader/proof; READY withheld; both exits before acknowledgement |
| Supervisor killed at prompt | No handoff; actual supervisor and native SIGKILL; no close acknowledgement |
| Supervisor killed at handoff | Stopped loader observed; one handoff; actual native SIGKILL; no acknowledgement |
| Supervisor killed before READY | Loader already exited 0; proof verified; agent SIGKILL; no READY or acknowledgement |
| Loader killed at prompt | Loader SIGKILL distinguished from cancellation; agent shutdown observed; no handoff or READY |

All eight cases passed on Ubuntu 24.04 amd64 with OpenSSH package
`1:9.6p1-3ubuntu13.20`. Every case externally checked slave mode/UID, retained the
master through exit, revalidated package/provider ownership, and rejected both
retained and new agent connections afterward. An independently owned loaded
sentinel signed and passed the original verifier after every case.

The updated helper also passed the preceding seven-case spawn matrix. The
auxiliary Rust 1.89 build passed 17 unit tests, fmt, Clippy debug/release and
build, offline. Its external dependency versions were already in the reviewed
writer graph, plus the product-locked libc version. The builder uses the Debian
Rust image; native runtime observations use Ubuntu. Python unit verification
passed 260 tests with one POSIX-only case skipped on Windows. These are scoped
checks, not the complete product CI matrix or Cargo-deny.

The runtime was unprivileged, NNP, offline, CPU1, 512 MiB RAM without additional
swap, PID128 and bounded tmpfs. Both runs exited 0 without OOM or selected RDP
error events. This does not rule out every RDP failure or establish its earlier
cause. Owned containers and disposable stores were removed after report export.

## Reproduction and limits

Run through RustWork with `--family rust`, preserving the original writer and
proof verifier volume and using a separate read-only launcher build volume:

```text
python run_lifecycle_probe.py --experiment rust-loader --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

Reports retain case metadata and source/helper identities, without secret bytes,
terminal transcripts or signatures. Incomplete native observation fails the run
and retains fixture ownership until external containment ends. Real SSH
disconnect is outside this matrix; the later [SSH extension](RUST-SSH-LOADER.md)
records its separate transport observations. Full shared-session integration, full-flow
performance and automatic recovery after failed pidfd acquisition remain
unaccredited. The earlier owner limitations in RUST-LAUNCHER.md still apply.
