# Candidate Rust spawn owner

This fixture uses the same `src/device_keys/linux_spawn.rs` source as the Linux
creation command and private custody session. The helper includes it by path,
using Rust 1.89 and the product's locked libc 0.2.189 binding. The helper
exercises process ownership without supplying a key or passphrase.

## Ownership and execution boundary

`SpawnOwner` is tied to its creating PID and thread ID and is neither `Send` nor
`Sync`. A stable owner thread retains its children, process descriptors and
selected resources. Caller threads request work without becoming the native
process creators. Shutdown closes admission and acknowledges closure only after
exit is observed. Its bounded shutdown returns an error while ownership remains
uncertain. The later [bounded abandonment contract](RUST-ABANDON.md) replaces
the original destructor's indefinite wait: Drop makes one stop/poll pass and
retains entries whose exit remains unobserved. Failure to acquire a pidfd after
successful spawn retains the child and capabilities and permanently closes
process-wide custody admission. There is no numeric-PID signaling fallback.
Four process-wide owner permits, each allowing at most four children, bound
retained ownership. This is containment of uncertainty, not automatic recovery.

All strings, C argument/environment tables, signal data and descriptor selections
are prepared in the parent. The post-fork callback uses fixed syscalls and
prepared memory: parent checks, NNP/PDEATHSIG setup and verification, optional
session/terminal setup, signal restoration and descriptor flags. It performs no
formatting, allocation, environment lookup or mutex operation.

`close_range(CLOSE_RANGE_CLOEXEC)` marks extra descriptors for closure at exec,
while preserving Rust's internal spawn-error channel until exec succeeds. Only
explicitly owned inherited descriptors have CLOEXEC cleared. The callback never
reassigns an inherited capability to a fixed descriptor that could collide with
that internal channel. The final call is raw `execve` through the retained ELF
descriptor, using the prepared C tables. A failed exec returns errno to Rust's
existing channel; it cannot reach `execvp`'s interpreter fallback.

The primitive checks regular ELF shape and descriptor ownership by its API.
Provider selection, trust, executable contents and immutable-object revalidation
remain caller obligations. The native-agent cases use the existing Ubuntu
[provider gate](PROVIDER.md), preserving the installed agent's original mode.
The prototype's argument and environment API is for nonsecret metadata; it does
not establish a supported secret channel.

## Observed cases

| Case | Required observation |
|---|---|
| Stable creator under pressure | Four native receipts from one creator thread, four returned caller threads and four allocator/mutex pressure threads; clean child environment and signal mask; exact FD allowlist; high non-CLOEXEC canary excluded |
| Real exec failure | Executable malformed ELF reports ENOEXEC through spawn; no receipt program runs; owner remains usable |
| Creator-thread exit control | Deliberately forgotten test owner; thread ends while parent process and control channel remain alive; child receives SIGKILL |
| Owner death before arming | B barrier reached; owner killed; retained observer releases G; child reports P for changed parent and exits unsuccessfully without executing the receipt program |
| Owner death after arming | A barrier reached; owner killed while observer retains the gate; child receives SIGKILL before exec |
| Native empty agent | Admitted agent executes under allocator pressure; credentials and empty identities checked; shutdown observed; retained/new connections refuse |
| Native agent owner death | Native credentials and empty identities checked after exec; observer retains a connection, kills owner and observes agent SIGKILL before checking connection refusal |

The creator-exit control intentionally leaks its test owner's Rust object to
exercise the kernel link independently of its Drop cleanup. It is bounded to a
disposable case process. This is not a production ownership technique.

Barrier events are eight-byte packets: phase, three zero bytes and big-endian
child PID. G releases a barrier. P identifies the changed-parent rejection before
arming; an unsuccessful exit alone is insufficient. The observed prearm exit can
be SIGABRT from a failure path rather than SIGKILL, so the matrix records the
actual signal and requires P. Armed/native-death cases require actual SIGKILL.
The observer is a subreaper only inside its disposable container and reaps
specific owned descendants, retaining pidfds and channels through exit.

The fixture inherits NNP=1 from its container. It observes NNP after exec and
reviews the set/check path, without claiming a measured transition from NNP=0.
It does not test a loaded key, PTY secret delivery, startup cancellation, shared
READY publication, retained package leases, SSH disconnect or the full product
runtime/CI matrix. Those remain integration gates.

## Reproduction and evidence

Build the auxiliary crate into a new volume and run through RustWork with
`--family rust`, using a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment rust-launcher --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner verifies read-only helper mounts, CPU1, 512 MiB RAM with no extra swap,
128 PIDs, bounded tmpfs, no network, unprivileged credentials and NNP. It observes
selected RDP events and controls only its own container. Reports preserve
partial cases, source/helper identities and process metadata. They contain no
credentials or terminal transcripts. Uncertain native exit prevents a successful
closure report and preserves fixture ownership until external containment ends.

Primary references: Linux documents the [creator-thread meaning of PDEATHSIG](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
and [CLOEXEC marking with close_range](https://man7.org/linux/man-pages/man2/close_range.2.html).
The reviewed [Rust 1.89 implementation](https://github.com/rust-lang/rust/blob/1.89.0/library/std/src/sys/process/unix/unix.rs)
defines callback/error-channel ordering; its normal execvp path motivates the
explicit execve boundary here. These references support the selected mechanism,
not admission of the complete credential backend.
