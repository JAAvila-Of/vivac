# Shared candidate native session adapter

`src/device_keys/linux_session.rs` now holds the package lease, signing work and
native stop/release bridge previously implemented in the
[concurrent Rust fixture](RUST-CONCURRENT.md). The product module remains
Linux-test-only. The fixture imports this source and keeps startup, barriers,
external observation and report orchestration. It no longer contains a second
lease, signing-worker or native cleanup implementation.

## Admission and identity boundaries

The caller admits the provider and encrypted package profile, creates the agent
and loader, checks the exact loaded identity/comment and independently verifies
readiness before constructing this loaded-session adapter. PackageLease is an
anchored-object/content lease, not the full package-admission reader. It keeps
the earlier limits: key at most 65,536 bytes and manifest at most 8,192 bytes,
private regular objects with one link, owned directory and exact retained bytes
and metadata. The key must match the selected ciphertext descriptor.

The constructor obtains actual receipts from the thread-affine spawn owner,
requires distinct tokens from the current creator thread, loader exit 0 and a
live agent, and checks the retained descriptor allowlist. It takes up to eight
extra owned CLOEXEC capabilities. The fixture transfers its five existing
master/executable/ciphertext/public-metadata capabilities through that boundary.

The socket is fixed at construction. SO_PEERCRED must match the owned agent PID
and the caller's effective UID/GID. Every signing connection checks that tuple
again before sending. A signer cannot select a different socket from the session
whose agent lock will stop. SigningIdentity bounds and binds the public key blob
and challenge shape; it does not authenticate a client or establish freshness.
Full provider/profile admission and the exact loaded identity remain caller
obligations, not consequences of constructing a typed object.

## Ownership and closure

The adapter stays on the native creator thread. A one-place command channel
connects the gate's stop/release callbacks to that owner. Stop uses only retained
process tokens/pidfds, observes agent SIGKILL and loader exit 0, and keeps the
resources. Release additionally checks that the linked gate is Closing with no
active worker, the lease has one owner, and both receipts match the observed
exits. Only then does owner shutdown release the bundle. The gate confirms
CLOSED after that callback. An already completed release can acknowledge a
retry without repeating resource destruction.

NativeSigner consumes its lease loan. Its four test hooks can pause work, but
each hook is followed by epoch and lease checks; the native reply must still
pass strict Ed25519 signature parsing and verification. The hook cannot bypass
those checks. The [gate's publication and join contract](RUST-CONCURRENT.md)
remains the same.

Dropping an adapter before accredited release is not lock. It permanently closes
process-wide native custody admission, attempts SIGKILL only through the spawn
owner's retained pidfds and retains the entire bundle and owner permit without
waiting or acknowledging exit. A disconnected callback channel returns
uncertainty. The [bounded abandonment contract](RUST-ABANDON.md) limits owners
admitted before uncertainty; retained resources can remain until process
termination. It provides no automatic recovery or accumulated session retries.
The earlier creator/pidfd, PTY permission-transition and cooperative-deadline
limitations still apply.

## Observations on 2026-11-12

The same seven native cases passed using the shared adapter on Ubuntu 24.04
amd64 with OpenSSH `1:9.6p1-3ubuntu13.20`. The observer and control protocol were
unchanged. A direct comparison with the preceding gate run matched every causal
case field after excluding only process identities and the pressure counter.
The completed control published; the cancellation and lease-negative cases did
not. Each worker joined, both parallel lock callers acknowledged closure, native
exits and retained/new connection refusal were observed, and the independent
sentinel signed after every case. The run exited 0 without OOM or selected RDP
error events. This neither measures input responsiveness nor diagnoses an
earlier RDP failure.

The final clean, offline Rust 1.89 auxiliary build passed 44 unit tests, fmt,
Clippy development/release and build. An intermediate build passed 43 tests
before adding the extra-borrower guard check; its evidence was retained. Python
checks passed 277 tests with one POSIX-only case skipped on Windows. No new
dependency version was introduced. The full product CI matrix, Cargo-deny and
full-flow performance measurements were not run in this increment.

The extra-borrower unit test invokes the same private authorization function as
actual release, with a real gate and lease plus a controlled close resource. It
checks uncertainty with no cleanup/ACK while another loan exists, and successful
closure with a unique owner. It is not an eighth native case. Peer credential
rejection also has unit coverage; the seven native cases use the admitted agent,
not an injected foreign agent. Abandonment/recovery and every possible race are
not accredited by these observations.

Use the unchanged `rust-concurrent` experiment and a separate launcher build
volume as described in RUST-CONCURRENT.md. Runtime limits remain CPU1, RAM512 MiB
without extra swap, PID128, bounded tmpfs, NNP, unprivileged execution, read-only
helpers and no external network. Operational Linux custody remains unavailable.
The later [SSH/signing composition](RUST-SSH-CONCURRENT.md) records actual
transport loss while this adapter has signing work in progress.
