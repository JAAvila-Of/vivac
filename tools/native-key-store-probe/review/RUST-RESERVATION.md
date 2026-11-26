# Candidate cooperative reservation fixture

This experiment imports the shared Linux-test-only reservation, package lease
and SpawnOwner sources. The
[reservation contract](../../../docs/LINUX-CUSTODY-RESERVATION.md) defines the
physical-directory scope and release boundary. It enables no product custody
or operational recovery.

## Native observations

The Rust host runs four pressure threads. It starts an owned worker with
prepared public package descriptors. The observer checks process lineage,
stable identity, executable, credentials, capabilities and NNP using retained
pidfds before releasing each protocol phase. The fixture uses public marker
files rather than encrypted credentials or unlock secrets.

| Case | Required observation |
|---|---|
| `contention` | An independent open of the same directory is busy while another directory can be reserved, even when the original descriptors share an open file description |
| `normal-release` | Dropping the reservation permits a contender while the worker remains alive |
| `abandonment-retains` | Uncertain child ownership poisons admission and retains the reservation while worker and child remain alive |
| `death-is-not-exit` | Killing the owned worker releases its reservation while a deliberately unlinked public child remains alive and answers a probe |
| `replacement-binding` | Renaming and replacing a pathname preserves exclusion for the original anchored directory and refuses a different package with identical marker bytes |
| `invalidation-retains` | An in-place marker mutation invalidates validation while the existing reservation continues to exclude a contender using a fresh lease |

The unlinked child is exclusively a negative control. The observer retains its
pidfd, observes survival after the worker's signal exit, then kills and reaps
that exact child. It is not an allowed production fallback. The abandonment
child likewise requires independent observed exit. A deadline or external kill
cannot count as a successful normal exit. Final resource-release claims follow
actual owned-worker receipts and shutdown; intermediate observations do not
claim future exits.

All custody, product recovery, same-store recovery, MCP/server and lock
acknowledgement flags must remain false. Lock acquisition alone is never
native-exit evidence. Full package admission and rendezvous authority remain
outside this experiment.

## Reproduction

Build the auxiliary crate in a clean separate target. Run through RustWork
with `--family rust`, an isolated Ubuntu image and a new absolute report path:

```text
python run_lifecycle_probe.py --experiment rust-reservation --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner requires CPU1, 512 MiB RAM with no extra swap, 128 PIDs, bounded
tmpfs, no network, an unprivileged identity and NNP. It validates the exact
observer script and deadline and uses disposable store volumes. Runtime
evidence applies only to the observed ext-family filesystem and environment.

## Observation on 2026-11-13

All six cases were observed on Ubuntu 24.04 amd64. Uncertain abandonment
retained exclusion while both processes were alive. The negative death control
readmitted a contender while the unlinked child remained alive and answered
the observer; that child was subsequently killed through its verified pidfd
and reaped. No custody or recovery acknowledgement was issued.

The clean auxiliary build passed 71 Rust tests, formatting and Clippy in debug
and release with Rust 1.89. Python checks passed 325 tests with one POSIX-only
test skipped on Windows. Native regressions covered another 59 cases, including
loaded signing sessions and actual SSH transport loss. Source/helper identities,
retained evidence and limits are indexed in
[PERFORMANCE.md](../../../docs/PERFORMANCE.md). Full product CI and operational
same-store replacement remain unaccredited.
