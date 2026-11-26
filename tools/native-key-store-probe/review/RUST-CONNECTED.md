# Candidate connected-peer identity fixture

This experiment tests the shared Linux-test-only
[connected-peer identity component](../../../docs/LINUX-CUSTODY-CONNECTED-PEER.md).
It does not accredit a custody endpoint or enable discovery and signing.

An ordinary Rust host runs four pressure threads and starts a server and client
as separately owned sibling processes. The server creates its listener and the
client connects independently. Both acquire identity from their connected
socket using SO_PEERCRED and SO_PEERPIDFD. The existing fixed control channel
continues to require SCM_CREDENTIALS and live peer pidfds around I/O.

| Case | Required observation |
|---|---|
| `roundtrip` | Both handshakes and two sequenced control probe/reply exchanges succeed |
| `wrong-generation` | Different public generations refuse the handshake |
| `wrong-store` | Different anchored marker packages refuse the handshake |
| `wrong-role` | Matching roles refuse the handshake |
| `forwarded-sender` | After handshake, a separately owned injector sends through a duplicate client socket; the server refuses its message credentials |
| `peer-exit` | Server exit is observed independently before the client's next operation refuses |
| `historical-listener` | A listener deliberately created by the still-live host identifies that host, while messages from the inheriting server are refused by the client |

The historical listener is a negative control. It does not provide an accepted
production launch configuration. The original host remains alive, so refusal
must distinguish the current sender from the historical peer identity. Neither
this case nor the forwarded-sender case claims that a side which has not
observed an error must already have poisoned its own channel.

An independent observer checks process lineage, executable, credentials, NNP,
capabilities and stable start identities while retaining pidfds. Server, client
and any injector remain observable until release of their corresponding
protocol phase. Exact owned receipts and observed exits precede final resource
release. Intermediate reports credit only exits already observed. A deadline
or external kill cannot replace the expected normal exits.

The store files are public markers. Reports contain counters, transient process
identities and artifact hashes, never package contents or generation values.
Endpoint-authority, custody, recovery, MCP/server and lock-acknowledgement flags
must all remain false. The socket pathname is disposable fixture plumbing.

## Reproduction

Build the auxiliary crate in a clean separate target, then run through RustWork
with `--family rust`, an isolated Ubuntu image and a new absolute report path:

```text
python run_lifecycle_probe.py --experiment rust-connected --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner requires CPU1, 512 MiB RAM with no extra swap, 128 PIDs, bounded
tmpfs, no network, unprivileged execution and NNP, including its exact observer
script/deadline allowlist. Unsupported SO_PEERPIDFD is an explicit failure;
there is no numeric-PID fallback or silent skip.

## Incomplete native observation on 2026-11-13

The local Ubuntu 24.04 container shares kernel
`5.15.167.4-microsoft-standard-WSL2` with its host. It does not supply an
independent Ubuntu kernel. The explicit capability diagnostic returned errno
92 (ENOPROTOOPT) for SO_PEERPIDFD. Both native attempts remained incomplete,
with exit 1 and zero accredited cases. The later diagnostic preserved the
option and original errno instead of the first attempt's generic ValueError.
No unsupported case was skipped or converted into success.

The auxiliary build passed 74 Rust tests, formatting and Clippy in debug and
release with Rust 1.89. Python checks passed 342 tests with one existing
POSIX-only test skipped on Windows. All ten inherited-channel regression cases
were observed successfully on the same helper. Those checks do not accredit
the seven connected-peer cases. A kernel exposing the required option is still
needed to execute them. Full product CI was not run.

The [evidence index](../../../docs/PERFORMANCE.md) records separate successful
regressions and incomplete attempts. Host WSL/RDP configuration was unchanged;
normal Linux custody and endpoint authority remain unavailable.

## Subsequent guest observation on 2026-11-13

All seven cases were observed with Ubuntu kernel 6.8.0-149-generic in a QEMU
TCG guest. Roundtrip accepted two sequenced probes. Generation, store and role
mismatches refused hello. A forwarded sender was refused despite possession of
the socket. Server exit was observed before the client's next operation refused.
The historical-listener control retained the live host's connection identity
and refused the inheriting server's current message credentials. Exact owned
receipts and independent observer pidfds established exits before final closure.
All endpoint-authority, custody, recovery, MCP/server and lock ACK flags stayed
false.

The guest used one vCPU and 256 MiB RAM. Its observer ran as UID/GID 65534 with
NNP and capabilities removed, under the original PID-1 timeout and unchanged
acceptance checks. Its effective cgroup limited work to CPU1, 128 MiB memory,
no additional swap and 128 PIDs. Root was read-only; `/tmp` was a 128 MiB tmpfs;
the disposable store was a dedicated ext4 disk snapshot. There was no guest
network interface beyond loopback.

The outer container retained CPU1, 512 MiB RAM with no additional swap, 128
PIDs, NNP, no capabilities, read-only root and no network. Its bounded 128 MiB
temporary mount was `/var/tmp`, where QEMU creates its disposable disk snapshot.
TCG used one thread and an explicit 16 MiB translation cache. A 448 MiB sampled
memory threshold rejected excessive use before crediting a run. The final six
samples ranged from 167.9 to 261.3 MiB; these are samples, not an absolute peak.
Guest and outer process both exited 0, without OOM in this final attempt.

An earlier guest attempt recorded OOM and exit 137 with the initial TCG
configuration; it remains a failed attempt. Initial preparation and bootstrap
failures also remain separate. No WSL/RDP setting or installed product was
changed. The emulator is test infrastructure, not a product dependency or a
proposed production containment boundary.

Kernel and module packages came from APT-authenticated Ubuntu indexes. The
selected preparation image, assembly receipt, explicit manifest digest and
kernel/initramfs/store hashes were checked before QEMU execution. The exact
helper and observer identities were checked again against the guest report.
The setup assumes a trusted host and no concurrent writer to its dedicated
asset volume; it is not remote attestation against a hostile host.

The same unchanged Rust helper had passed 74 tests, formatting and debug/release
Clippy with Rust 1.89. Final Python checks passed 344 tests with one existing
POSIX-only omission on Windows; two auxiliary integrity tests separately
rejected payload substitution and manifest rewriting. Failure diagnostics now
report bounded code-frame identifiers, checkpoint and errno without exception
messages or full paths. Successful schemas, seven cases, guard checks and
deadlines were not relaxed. The [performance evidence index](../../../docs/PERFORMANCE.md)
records final identities and the retained local reproduction scripts.
