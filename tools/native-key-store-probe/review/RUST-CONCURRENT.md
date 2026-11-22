# Candidate Rust signing and lock gate

This fixture compiles the shared `src/device_keys/linux_gate.rs` with the
[Rust spawn owner](RUST-LAUNCHER.md) and [private loader](RUST-LOADER.md). The
product declaration remains Linux-test-only. It transfers the ordering contract
of the earlier [Python concurrency fixture](CONCURRENT.md) to Rust ownership;
it does not enable operational Linux custody or authenticate clients.
The later [shared session extraction](RUST-SESSION.md) moves the native adapter
out of this fixture and records its separate source/build/runtime revision.

## Admission, publication and worker ownership

The gate owns one signing worker and a single optional 64-byte public signature.
Admission, closing and local publication share a state mutex and epoch. Active
work or an occupied published slot causes an explicit busy refusal. Closing
advances the epoch and refuses new work. A signature published before closing
remains published; a result not yet published is cancelled when closing wins.
This local slot is a publication boundary, not evidence of remote delivery.

Native signing, verification, lease access and waiting run outside the state
mutex. The worker records the ordered send, reply and verification events. Only
the verified sequence can publish. Ordinary failure and lease invalidation close
admission; panic, poisoned synchronization and uncertain outcomes prevent a
successful close acknowledgement.

Completing the callback does not clear active work. The gate keeps its
JoinHandle until termination is observed and join succeeds. Lock closes
admission first, serializes cleanup, requests native stop, joins the worker,
releases capabilities and then commits CLOSED and its acknowledgement. Concurrent
lock callers share cleanup and each must meet its own monotonic deadline.
Waiting never holds the state/publication mutex; a separate cleanup mutex
serializes the stop/release callbacks. A worker cannot lock or wait for itself.

The gate keeps a self-reference from creation until joined, observed closure.
This prevents dropping the last caller from silently detaching the worker or
releasing ready-session resources. Abandonment, panic or permanent uncertainty
can therefore retain custody until process termination. This candidate does not
provide automatic recovery or justify accumulating abandoned sessions in a
product. The disposable fixture permits only one session at a time.
The later [native abandonment contract](RUST-ABANDON.md) adds a process-wide
owner limit and irreversibly refuses new native custody after uncertainty;
it does not change the gate's publication or explicit lock protocol.

Deadlines are checked through lock and after join/callback completion; late
completion cannot produce a successful acknowledgement. Callbacks must cooperate
with their deadline. They are not preempted by this gate. Rust documents
[is_finished and join](https://doc.rust-lang.org/std/thread/struct.JoinHandle.html#method.is_finished)
as supporting a nonblocking-join pattern with a short possible termination gap;
this is not a hard real-time thread-cancellation guarantee.

## Native adapter and independent observer

The main thread creates the native agent and loader and remains their stable
creator. Signing and lock callers run in other threads. A bounded command
channel asks the main thread to stop the native processes through retained
pidfds and observe their exits, while retaining the spawn owner's entries.
Only after the signing worker has joined does release call owner shutdown and
drop the retained capabilities. The adapter also requires unique ownership of
the package lease before dropping it.

The caller admits the encrypted package/profile and native provider. The Rust
adapter binds an anchored package-directory descriptor, retained key/manifest
identities and bounded exact bytes. It compares the key with the sealed
ciphertext and checks those bindings around signing and before publication.
This is not a replacement for the full package-admission reader. The native
reply must contain one exact Ed25519 signature, verified against the selected
public key and fixture challenge.

The Python observer retains independent pidfds, package lease, PTY master and a
connection which signed before the race. It verifies readiness and a published
control proof using the original separate Rust verifier. Barriers hold work
before native send, after a complete send but before reading its reply, after
reply receipt and after verification but before publication. It observes
closing with work still paused and no acknowledgement before releasing that
work. A sent frame does not establish when OpenSSH executed its signing code.

The lease-negative control mutates the owned encrypted key only after the
selected pre-publication seam, independently detects lease invalidation and
requires that specific outcome. The sentinel belongs to a separate loaded
agent and must still sign after every case. Reports contain process/counter
metadata, no secrets, terminal transcripts or signatures.

## Observed cases on 2026-11-12

All seven cases passed on Ubuntu 24.04 amd64 with OpenSSH
`1:9.6p1-3ubuntu13.20`:

| Case | Sent / reply / verified / published | Required outcome |
|---|---|---|
| Completed before lock | 1 / 1 / 1 / 1 | Independent published proof verified; busy request refused |
| Lock before native send | 0 / 0 / 0 / 0 | Cancelled before native request |
| Lock with reply pending | 1 / 0 / 0 / 0 | Cancelled before consuming reply |
| Lock after native reply | 1 / 1 / 0 / 0 | Received result withheld |
| Lock before publication | 1 / 1 / 1 / 0 | Verified result withheld |
| Parallel lock callers | 0 / 0 / 0 / 0 | One worker joined; both callers acknowledged closure |
| Lease changed before publication | 1 / 1 / 1 / 0 | Independently detected lease invalidation; no publication |

Every case observed exactly one joined signing worker, agent SIGKILL and loader
exit 0 before acknowledged closure, retained/new connection refusal, a retained
master through native exit, and an unaffected sentinel signature afterward.
The fixture exited 0 without OOM or selected RDP error events. That RDP
observation does not measure input responsiveness or diagnose an earlier failure.

After clean, the offline Rust 1.89 auxiliary build passed 36 unit tests, fmt,
Clippy development/release and build. An initial Clippy failure for excessive
arguments in the coordinator was retained; an identity struct removed it without
a lint suppression, followed by clean and repeated verification. Python checks
passed 277 tests with one POSIX-only case skipped on Windows. An additional
Windows product nextest attempt was stopped during compilation after exceeding
the read-only review scope; it is incomplete and supplies no product-CI verdict.
Its recorded launcher processes and Windows Job were subsequently absent.
The complete product CI matrix was not completed; Cargo-deny and performance
measurements were not run.

## Reproduction and limits

Use existing local helpers and a separate launcher build volume, through
RustWork with `--family rust` and a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment rust-concurrent --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner retains CPU1, RAM512 MiB without additional swap, PID128, bounded
tmpfs, NNP, unprivileged execution, read-only helpers, external-network isolation
and selected RDP event monitoring. An uncertain exit remains incomplete and
retains resource references until external containment ends. This fixture does
not accredit every interleaving, client authorization, request freshness, silent
SSH partitions, full-flow performance or the operational product backend.
