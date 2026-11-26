# Candidate custody process boundary

The `rust-isolation` experiment checks the process boundary required by the
[Linux custody isolation contract](../../../docs/LINUX-CUSTODY-ISOLATION.md).
It uses an ordinary persistent Rust host, not the product MCP loop or a central
server. Linux custody remains unavailable in normal product commands.

## Causal sequence

The host runs four allocator/mutex pressure threads and retains a stable
SpawnOwner on its creator thread. It launches two disposable worker executors
through the shared spawn source and retains their process identities.

Worker A reuses the missing-pidfd [abandonment case](RUST-ABANDON.md): after
spawning a real self-exec child and storing Child/spec, it injects EMFILE,
observes an explicit shutdown timeout and returns from Drop. Its process-wide
custody admission remains uncertain, its child remains responsive and its
marker descriptor remains retained. A does not exit yet.

While A is still present, an external observer commands a host health response.
The host checks that its own admission is healthy and actually creates and
drops an empty owner. It then launches worker B, whose separate process reserves
all four owner permits, observes EBUSY on a fifth, releases the clean owners and
successfully admits another owner. The host observes B's exit 0 while A remains
alive. The external observer checks A through a retained pidfd, pings A's native
child and verifies A's marker descriptor identity before permitting A to end.
For each worker, the host observes its retained pidfd and checks the exit receipt
against the expected token and PID. Final release requires exactly one matching
exit receipt for A and one for B; substitutions and duplicates refuse.

A answers another command before the host permits its normal exit. The observer
then collects its child's SIGKILL status through a retained pidfd and subreaper
relationship. Only after that observation does it request a second host health
response and explicit host cleanup. The host observes both owned worker exits,
releases its resources, joins its pressure threads and returns normally.
An external kill or fixture deadline cannot substitute for these responses.

Private bounded control packets keep the worker payloads separate from host
observations. No inherited descriptor is remapped to a fixed numeric target.
The outer observer independently checks process liveness and retained marker
identity; it does not infer either from a host boolean alone.

## What this checks

The shared Rust source's process-wide refusal remains local to the uncertain
worker. A resident host and another process can continue while that worker is
still uncertain. The experiment uses a public self-exec responder, not a loaded
OpenSSH agent, signing key or unlock secret. It does not exercise same-store
replacement, credential reauthentication or authorization.

The experiment does not implement authenticated custody IPC, store exclusion,
readiness discovery, generation rollover, recovery after losing exit evidence,
actual MCP integration or central server operation. Those remain separate
product requirements. A fresh process with free owner slots cannot authorize a
second session of an uncertain store. The host's use of SpawnOwner to launch
disposable workers is fixture orchestration, not an operational MCP design.

## Reproduction

Build the auxiliary helper in a separate target volume. Run through RustWork
with `--family rust`, an isolated Ubuntu image and a new absolute report path:

```text
python run_lifecycle_probe.py --experiment rust-isolation --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner preserves CPU1, 512 MiB RAM, no extra swap, 128 PIDs, bounded tmpfs,
no network, unprivileged credentials and NNP. The observer's exact script and
deadline command must pass the same resource checks as the other fixtures.
Reports contain only public control metadata and source identities. Selected
RDP error logs are observed; their absence does not diagnose all remote-desktop
failure modes.

## Observation on 2026-11-12

The final `process-boundary` case was observed on Ubuntu 24.04 amd64. A refused
three new owners and retained its native child and marker while B reserved four
owners, refused a fifth, readmitted after clean release and exited 0. The host
answered two health requests, remained healthy and actually admitted an empty
owner. The outer observer verified A/C process lineage, credentials, NNP,
capabilities, executable identity and stable start times around retained live
pidfds. It collected C's SIGKILL status before the final host shutdown. The host
observed exact worker identities and exits through retained pidfds and returned 0.

The clean final auxiliary build passed 59 Rust tests, formatting and Clippy in
debug and release offline with Rust 1.89. The unchanged final Python sources
passed 306 tests with one POSIX-only test skipped on Windows. Product CI,
Cargo-deny, full-flow performance and operational MCP/IPC recovery were not run
or accredited by this increment.

The helper SHA-256 was
`a2f0d4b4b76e0e95678bf0c8c5e32589471f7a0dd384efcc549f675f842bddb9`;
the observer SHA-256 was
`fa693964a85b486068cfa3fdde516cf540b596c6a681019ed9231e29eb662569`.
The final source manifest and selected regression reports are indexed in
[PERFORMANCE.md](../../../docs/PERFORMANCE.md). These observations establish
the fixture's process boundary, not the same-store recovery conditions in the
product contract.
