# Candidate connected-peer identity

This is an identity component for the Linux-test-only
[control channel](LINUX-CUSTODY-IPC.md). It does not authorize a discovered
custody endpoint. Normal Linux custody, independent-client signing and
same-store replacement remain disabled.

## Identity from the connected socket

The inherited channel obtains an expected process from an owned-child receipt
or a stable kernel parent. A separate client has neither relationship to an
existing foreground session. Reading a numeric PID from runtime metadata and
opening a pidfd would leave the selection unauthenticated and introduce a
process-selection race.

The candidate instead obtains SO_PEERCRED and SO_PEERPIDFD from the same
connected AF_UNIX SOCK_SEQPACKET socket. Linux's implementation derives both
from the socket's retained peer PID reference. The pidfd is obtained directly
from the kernel; there is no pidfd_open fallback using an advertised PID. An
unsupported option or unavailable peer refuses the operation.

Acquisition requires a close-on-exec descriptor, SO_PASSCRED, stable matching
real/effective local UID and GID, and peer credentials for the same UID/GID.
The peer must be another live process. Credentials are checked around pidfd
acquisition, and the returned owned pidfd must be close-on-exec and live.
These checks acquire no store lock, credential or human approval.

Linux records connection credentials at connect, listen or socketpair time.
In particular, a listener created by one process and inherited by another can
identify the original listener rather than the process now sending messages.
SO_PEERPIDFD retains this historical relationship too. It is not proof of the
current sender. The existing channel therefore continues to compare every
SCM_CREDENTIALS sender to the acquired expected peer and checks pidfd liveness
before and after I/O. Forwarding a socket does not transfer the expected
process identity to its new sender.

Primary references: Linux v6.8
[socket option implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/net/core/sock.c),
[Unix connection/listener implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/net/unix/af_unix.c)
and [UAPI option definitions](https://raw.githubusercontent.com/torvalds/linux/v6.8/include/uapi/asm-generic/socket.h).

## Authority remains separate

This operation answers which process the socket identifies and whether that
process is alive. It does not establish that this process is the selected
store's authorized custodian. Same-UID processes and endpoint administrators
remain trusted by the profile; a kernel PID identity does not add organizational
permission, a trusted executable revision or fresh terminal consent.

Likewise, agreement on generation and anchored store words binds an exchange
only after an appropriate bootstrap. Those public values are not bearer
capabilities. A pathname locates a socket; its presence, owner or apparent
freshness does not provide the missing authority. A peer can also proxy someone
else's request, which message credentials do not reveal.

No production discovery path, automatic cleanup or reconnect is introduced.
Operational admission still needs an explicit trusted endpoint bootstrap, the
[cooperative store reservation](LINUX-CUSTODY-RESERVATION.md), an admitted
package/provider and the independently preserved exit evidence required by the
[isolation contract](LINUX-CUSTODY-ISOLATION.md). Losing a connection or acquiring
a new pidfd cannot stand in for those conditions.

## Acceptance boundary

The disposable experiment must use actual listen/connect between sibling
processes, rather than a socketpair created by their parent. It checks normal
control exchanges, mismatched generation/store/roles, forwarded senders and
peer exit. A deliberately inherited listener supplies a negative control for
historical credentials. External pidfds and owned receipts establish process
lineage and exits independently of the channel's reports.

The experiment uses public marker packages and control probes only. It carries
no unlock input, signing request or credential, enables no handle transfer,
and must keep endpoint-authority, custody, recovery, server and lock
acknowledgement claims false. Its pathname is fixture plumbing, not an
accredited discovery mechanism.

The initial Ubuntu 24.04 container shared a WSL2 5.15 kernel that rejected
SO_PEERPIDFD with ENOPROTOOPT. A subsequent isolated QEMU guest with Ubuntu
kernel 6.8.0-149-generic observed all seven cases successfully. The unchanged
acceptance checks and deadlines applied inside that guest; only failure
diagnostics were extended. See the
[fixture observations](../tools/native-key-store-probe/review/RUST-CONNECTED.md)
for the incomplete attempts, final kernel-specific evidence and resource limits.
This establishes connected-peer identity behavior, not endpoint authority.

The [local bootstrap contract](LINUX-CUSTODY-BOOTSTRAP.md) defines the independent
creation-admission expectation required before authenticating a custodian.
That producer and reader remain implementation prerequisites.
