# Candidate inherited IPC fixture

This experiment imports the shared Linux-test-only `linux_ipc.rs` source.
The [control-channel contract](../../../docs/LINUX-CUSTODY-IPC.md) defines its
known-process bootstrap, fixed frame and trust boundary. It does not enable
normal Linux custody, endpoint discovery or credential signing over IPC.

## Native cases

An ordinary Rust host starts four allocator/mutex pressure threads and launches
its owned responder through SpawnOwner. Both sides receive public fixture
package descriptors and the prepared private channel. PackageLease checks
anchored objects containing public markers, not an admitted encrypted credential.
The generation is public bootstrap metadata and is omitted from reports.

| Case | Required observation |
|---|---|
| `roundtrip` | Bilateral hello and two correctly sequenced probe/reply exchanges |
| `wrong-generation` | Both sides refuse hello; no probe accepted |
| `wrong-store` | Different anchored objects with matching marker bytes still refuse hello |
| `replay` | First probe succeeds; repeated sequence is refused by the responder |
| `forwarded-sender` | A separate owned injector sends through a forwarded socket FD; its message credentials are refused |
| `ancillary-rights` | Responder rejects a received SCM_RIGHTS descriptor and its FD count does not increase |
| `peer-exit` | Responder exit is observed before the initiator's next operation refuses |
| `receive-timeout` | Live nonresponding peer causes an explicit deadline error and unusable initiator channel |
| `truncated-frame` | A 111-byte packet is refused |
| `oversized-frame` | A 113-byte packet is refused |

The observer independently checks host, responder and applicable injector
process lineage, credentials, NNP, capabilities, executable and stable start
identity while retaining pidfds. Processes wait for observer release through a
separate fixture channel, so readiness and refusal can be observed before normal
exit. Owned exits must match their exact receipts and pidfds before final
resource release; an external kill or fixture timeout cannot count as success.

For raw fault injection the host retains a duplicate transport descriptor.
Those sends deliberately bypass its Channel object. The fixture therefore
requires responder failure and persistent refusal, and does not claim that an
initiator which has not observed an error also became unusable. Peer-exit and
timeout cases instead require the initiator's own error and persistent refusal.

SCM_PIDFD cleanup and truncated ancillary FD cleanup have synthetic parser tests
using real descriptors. They are separate from the native SCM_RIGHTS receive
case. This experiment does not enable SO_PASSPIDFD or claim a native kernel
SCM_PIDFD receive test.

## Reproduction

Build the auxiliary crate in a separate clean target. Run via RustWork with
`--family rust`, an isolated Ubuntu image and a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment rust-ipc --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner keeps the existing CPU1, 512 MiB RAM, no extra swap, 128 PID, bounded
tmpfs, no-network, unprivileged and NNP checks, including the exact observer
script/deadline allowlist. Evidence contains public counters and identities,
never generation values, package contents, unlock secrets or private keys.

## Observation on 2026-11-12

All ten cases were observed on Ubuntu 24.04 amd64. Roundtrip accepted two probes;
replay accepted its first probe and refused the repeat. The remaining eight
cases accepted no probes. The SCM_RIGHTS case retained the same responder FD
count, 11 before and after rejection. Host/responder and the injector, where
used, had observed normal exits. All product-custody, recovery, MCP/server and
lock-acknowledgement flags remained false.

The fresh auxiliary build passed 66 Rust tests, formatting and Clippy in debug
and release offline with Rust 1.89. Final Python checks passed 312 tests with one
POSIX-only test skipped on Windows. Selected loaded-session and process-boundary
regressions are indexed with source/helper identities in
[PERFORMANCE.md](../../../docs/PERFORMANCE.md). Full product CI, Cargo-deny,
full-flow performance and operational session replacement remain unaccredited.
