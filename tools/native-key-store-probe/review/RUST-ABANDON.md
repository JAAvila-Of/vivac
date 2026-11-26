# Candidate native custody abandonment

The shared spawn owner and session adapter remain Linux-test-only in the
product. This contract bounds retained ownership and removes a destructor wait
loop; it does not enable Linux custody or implement self-hosted server recovery.

## Admission and uncertain ownership

Before creating a native child, an owner reserves one of four process-wide
permits. Each owner admits at most four child entries. The admission word holds
occupied permits and an irreversible uncertainty bit. A full healthy pool
returns EBUSY. Uncertainty returns EOWNERDEAD for new owners and new spawn
admissions, including existing owners. A spawn that passed its final admission
check before another owner marks uncertainty can still complete; its reserved
permit and child limit already count toward the maximum sixteen entries.

After successful spawn, Child and every selected capability are stored before
pidfd acquisition. Acquisition failure keeps that entry, closes admission and
returns the actual error. No numeric PID is used as a signaling fallback.
Explicit shutdown still requires observed native exits before releasing entries
and confirming completion. A deadline expiry preserves ownership and returns
an error.

Drop performs one best-effort SIGKILL/try_wait pass. If every exit is observed,
it releases the entries and permit. Otherwise it marks uncertainty and retains
the entire entry vector and occupied permit without repeating the wait. Once
set, uncertainty remains set even when another clean owner releases its permit.
The finite entry count does not establish a byte budget for arbitrary caller
metadata or a complete process-memory budget.

Dropping NativeSession before explicit release marks uncertainty before
retaining its complete resource bundle: spawn owner, permit, PTY, package lease
and selected descriptors. Best-effort signaling uses only that owner's pidfds.
The worker's lease loan and gate ownership remain valid. Abandonment is not lock:
it neither establishes a publication cutoff nor promises to revoke an already
published proof. Only the gate's explicit close protocol establishes that
boundary and joins the worker before accredited release.

## Availability and recovery limits

The creator thread must still remain alive for normal native custody. The new
destructor contains no wait loop or sleep, but kernel calls and scheduling have
no hard real-time bound. Uncertainty can retain native children or zombies and
capabilities until process termination; this contract provides no background
reaper, automatic retry, recovery or successful close acknowledgement.

A self-hosted server needs an explicit isolation and recovery design before
using this candidate. Keeping unrelated serving work responsive while refusing
new custody does not establish availability of the custody service. An external
fixture timeout or container kill is containment, never evidence of successful
closure by the candidate.
The [process isolation contract](../../../docs/LINUX-CUSTODY-ISOLATION.md)
requires a separate explicit foreground custody process and defines replacement
conditions; the cross-process exclusion and recoverable exit evidence are still
implementation gates.

Rust 1.89's [Linux pidfd module](https://github.com/rust-lang/rust/blob/1.89.0/library/std/src/os/linux/process.rs)
is unstable. Its [Unix process implementation](https://github.com/rust-lang/rust/blob/1.89.0/library/std/src/sys/process/unix/unix.rs)
provides a reference for child-side pidfd transfer and exec-error ordering, not
a stable API adopted here. Linux documents possible acquisition failures in
[pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html).

## Disposable failure cases

The separate `rust-abandon` observer exercises three cases: reserving all four
owner permits and reusing clean permits; injecting EMFILE after a real child
has executed and its Child/spec have been stored; and explicitly abandoning an
owner with a valid pidfd. Injection models the post-spawn error branch, not
actual exhaustion of the process descriptor table.

The missing-pidfd case requires an explicit shutdown timeout, a returned Drop,
a second command/response from the still-live helper and repeated refusal of
new owners. The child must still answer its own independent channel and the
marker descriptor must retain its exact identity in the helper. Its only
helper-side owner is the retained SpawnSpec. Only after those observations does
the observer request normal helper exit and collect the descendant's SIGKILL
status through its retained pidfd and subreaper relationship. Killing the
helper or reaching the container deadline cannot satisfy the response checks.

The `rust-concurrent` matrix adds a distinct `session-abandon` case. It starts
and independently verifies a real loaded session, pauses its signer before
native send, drops NativeSession and requires a response while the worker is
still paused. Five selected descriptor identities must survive both Drop and
the later worker join. The fixture observes native agent exit before releasing
the paused worker; that selected request fails without publication. Explicit
lock then receives uncertainty from the disconnected callback, with no close
acknowledgement or accredited resource release. This selected before-send
failure does not establish that Drop generally cancels pending publication.

Reproduce through RustWork with `--family rust`, a separately built helper
volume and a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment rust-abandon --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

Use `--experiment rust-concurrent` for the loaded-session abandonment case and
the existing normal-lock controls. Both experiments retain the same CPU1,
512 MiB RAM, no extra swap, 128 PID, bounded tmpfs, no-network, unprivileged and
NNP limits. The runner observes selected RDP error logs and owns only the
containers and temporary stores it creates.

## Observations on 2026-11-12

All three abandonment cases and the eight local signing-session cases were
observed on Ubuntu 24.04 amd64. The missing-pidfd child remained alive and
responsive across the returned destructor and the helper's next response; its
marker capability remained retained. Both standalone child cases subsequently
produced observed SIGKILL statuses. The loaded-session case retained all five
checked descriptors, joined its worker and returned callback uncertainty with
zero close acknowledgements and zero published proofs.

Regressions also passed all seven spawn, eight loader, ten SSH-loader and twelve
SSH signing-session cases: 48 native case observations in total, all with the
same helper digest. The independent loaded sentinel survived the signing and
loader experiments. Each runner reported exit 0, no OOM kill and no selected
RDP error events. These logs do not diagnose or exclude every possible RDP fault.

The fresh auxiliary build passed 56 Rust tests, formatting and Clippy in debug
and release using Rust 1.89 offline. The final Python checks passed 297 tests,
with one POSIX-only test skipped on Windows. Nine Rust source hashes matched
the retained build manifest after runtime verification. An initial abandonment
attempt was rejected before any cases because the new observer was absent from
the fixture's exact script allowlist. That name was added without changing the
deadline or resource checks; its failed report remains separate from the
successful rerun.

The helper SHA-256 was
`eb740afa072b93d80de11b0c51e656e4b299bd6b979ae00dfe79b82985d45a0b`.
The reports and source identities are listed in
[the performance evidence](../../../docs/PERFORMANCE.md).
This increment did not run the complete product CI matrix, Cargo-deny or
full-flow performance measurements, and does not activate a product backend.
