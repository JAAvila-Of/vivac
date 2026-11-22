# Candidate inherited custody control channel

This document specifies the first bootstrap component of the
[custody isolation contract](LINUX-CUSTODY-ISOLATION.md). The inherited channel
runs in the private startup and attended reopening paths of the Linux binary
and in the disposable probe. The same binary also connects the private listener
and independent verifier described below. The [public Linux custody CLI](LINUX-CUSTODY-CLI.md)
uses those paths for attended reopening and fresh local possession checks.

## Bootstrap authority

The initiator creates a private AF_UNIX SOCK_SEQPACKET socket pair with CLOEXEC
and enables SO_PASSCRED on both ends before launching an owned executor. Its
expected peer comes from that SpawnOwner's actual child receipt and retained
pidfd. The executor accepts only its stable kernel parent, retaining a pidfd
and checking the parent relationship around acquisition. This first component
does not accept a PID or socket pathname supplied by a discovery file as
authority. It does not define how a separate command discovers a live session.

The separate [connected-peer identity candidate](LINUX-CUSTODY-CONNECTED-PEER.md)
obtains a stable kernel peer reference from a connected socket. That identity
operation does not provide the authority missing from endpoint discovery.
The [local bootstrap contract](LINUX-CUSTODY-BOOTSTRAP.md) separately requires
an independently admitted credential expectation; the package manifest and
enrollment possession proof cannot supply their own authority.

Both sides require stable real/effective UID and GID and the expected peer's
credentials on every received message. Linux's SO_PEERCRED for a socket pair
describes credentials at creation; an inherited or forwarded descriptor can
therefore identify the original creator rather than the current sender.
SO_PASSCRED/SCM_CREDENTIALS supplies the current message's kernel-checked sender
PID, UID and GID. The channel compares those values to its expected live peer.
See [unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html).

This binds a message to an expected process within the trusted endpoint. It
does not establish organizational identity, permission, fresh human approval
or resistance to an endpoint administrator able to forge credentials or
control the application. Same-UID processes and endpoint administrators remain
inside the profile's trust boundary. A trusted process can proxy someone else's
request; this transport cannot determine that request's original authority.

The existing [enrollment possession proof](DEVICE-ENROLLMENT-PROOFS.md) is not
an endpoint handshake and is not repurposed. No new signature domain, arbitrary
signing operation, unlock command or secret-carrying message is introduced.

## Keeper supervisor startup

The keeper can start a supervisor from a retained descriptor for its running
image. The child checks the direct parent against the inherited socket's kernel
peer identity and compares the live executables by device and inode. The keeper
holds the spawned child's pidfd and sends a fixed authorization frame without
descriptors. The child checks the frame and its kernel sender credentials before
opening the store. Store admission, generation binding and registration follow
as independent steps before unlock input. The disposable SSH probe exercises
this startup path.

The Vivac binary has private keeper, supervisor and verifier entry points for
startup. The keeper sends each child the retained pidfd of its counterpart and
an open descriptor for their private runtime directory. The supervisor creates
the listener and the verifier connects from its own process. Each compares the
socket's peer with the reference received from the keeper before confirming
startup. In the startup-only trial, the keeper waits for both confirmations,
then closes the verifier's write-side channel and observes its successful exit.
It releases the supervisor to remove the identified socket after that
observation. Once the supervisor also exits successfully, the keeper removes
the empty directory. The attended reopening trial continues from the same
startup into store admission, possession proof and foreground close. The
socket identity check and unlink are separate kernel operations within the
same-UID trust boundary.

The private startup transport binds and listens before its nonblocking accept.
It pins the named socket leaf with a retained descriptor, checks the same leaf
before and after connecting, and compares it again before unlink. The startup
verifier can still retry a transient absence, refused connection or busy queue
within its original deadline. The independent client will use the single-attempt
connection path.

## Store and generation binding

The store selection consists of the device/inode pairs of the anchored package
directory, key and manifest. It is captured only after the retained PackageLease
validates. These six numbers select local objects; they are neither a portable
store identifier nor proof of package admission, confidentiality or authority.
The caller must retain and revalidate its lease before any future native
operation and publication. Transport equality cannot detect an in-place content
change by itself or replace those lease checks.

A new bootstrap generation uses 32 bytes from the operating-system random
source. Acquisition is nonblocking and may fail; there is no weaker fallback.
The all-zero representation refuses. Bootstrap transfer of these public bytes
must use the already trusted launch context. Their shape or equality does not
authenticate a discovered endpoint, and the generation is not a secret bearer
capability. No generation values or store identifiers are included in probe
reports.

This version has one inherited channel instance and one handshake. Reconstructing
an object, reconnecting or reusing an old generation is not a recovery procedure.
Multiple clients and session discovery require their own protocol before
product integration. A client does not participate in store exclusion or hold
exit evidence.
The separate [directory reservation candidate](LINUX-CUSTODY-RESERVATION.md)
provides cooperative physical-directory exclusion. It does not authenticate a
discovered endpoint or preserve the exit evidence needed for replacement.

## Candidate independent verifier rendezvous

The first independent client starts with a credential ID. It opens and retains
its own current admission from the fixed installation before looking for a
live endpoint. The current admission can be read during an active session,
when the opening permit has already been consumed. Discovery does not consume
or recreate an opening permit.

The Linux binary has a private verifier entry point for this exchange. It
derives the socket name and the endpoint directory from the same retained
admission. The verifier retains the directory and socket leaf across one
connection attempt and accepts the result only after revalidating both and
the admission. The private discoverable session publishes the listener only
after the keeper observes both startup proofs and the startup verifier's exit.
The keeper sends an authenticated activation and waits for the supervisor's
listener acknowledgement before displaying the foreground close prompt.

A locator gives the client one connection candidate for that credential. It
carries no authority to identify a process, establish readiness, select a
public key or supply admission. See the [candidate socket locator](#candidate-socket-locator)
for the address and lifecycle.

The active custodian may grant a live peer in the same provisioned account one
attempt of the [endpoint possession proof](LINUX-ENDPOINT-PROOF.md). Both sides
require the profile's stable UID and GID and check the live kernel peer for
the connected socket against the sender credentials on the received message.
The custodian checks the active generation, reservation and admission before
serving. The client verifies the fresh signed transcript on that same connection
with its retained admission.
A successful exchange establishes possession by that endpoint at the checked
instant. Account eligibility authorizes this one proof and conveys no authority
to unlock, close, reopen, recover or sign enrollment requests.

The custodian can admit independent connections only after the private startup
proof completes and the foreground session enters its active state. The close
or cancellation transition stops new admission before native teardown and
invalidates attempts that have yet to send a result. A result sent before that
transition describes its checked instant and confers no continuing lease. The
endpoint proof's existing 30-second cap and single active attempt slot apply.
The listener retains at most four connections waiting for a request, each for
at most three seconds under cooperative deadlines. The creator handles one
ready connection at a time under the same 15-second proof deadline as startup.
The I/O thread refuses ready contenders while a proof is in flight and watches
the keeper's control channel throughout. It stops admission under the gate
before handing the close result back to the creator, which owns native
teardown. Both proof paths validate the challenge before taking the native
attempt slot. Each independent connection has one challenge and one result.
The client has 20 seconds to locate the admission and verify a proof on one
connection attempt. It does not reconnect, start a custodian, or repair runtime
or store state.

After sending a result, the creator can retain the consumed connection until
its peer closes or its proof deadline expires. This gives the verifier time
to check the proof against the still-live connection. At most four completed
connections are retained, and none can hold or use the native attempt slot.

Native acceptance requires a client launched independently of the keeper,
with only the credential ID and without inherited custody descriptors. Use the
same private Linux binary and an isolated provisioned store for these cases.

| Case | Required observation |
|---|---|
| Active session and matching admission | The separate client verifies one fresh proof while the keeper retains the reservation and the opening permit stays absent. |
| Private startup proof incomplete | The independent client receives no proof and cannot make the custodian enter its active state. |
| Locator absent or endpoint gone | The client fails within its deadline without prompting, retrying or launching custody. |
| Locator replaced or selected admission mismatches | Location alone does not let a connection verify. The client refuses before accepting a result under a key from the responder. |
| Peer outside the selected account, sender mismatch or disconnected peer | The exchange refuses. No operation can be authorized from that channel. |
| Wrong request purpose or a second request on the connection | The custodian refuses without beginning another native operation. |
| Connected peer sends no challenge | The connection expires without holding the native attempt slot. Within the listener's capacity, a separate valid request can still take the slot. |
| Another proof already holds the active slot | The contending request refuses within its deadline without queuing native work. |
| Close or cancellation before the proof result is sent | Invalidation prevents the result from being sent. Native teardown and permit publication retain the keeper's existing rules. |
| Locator left after exit, or cached result from an earlier generation | The client cannot use it to report a live session or to reopen custody. |

### Candidate socket locator

The locator is the Unix `SOCK_SEQPACKET` socket at
`state/endpoints-v1/<credential_id>` under the validated account installation.
The client uses the canonical credential ID as the single filename component.
It opens the private directory through the retained installation, validates
its owner, mode, ACL and mount like the other account directories, and retains
it throughout use. There is no pointer file, PID file or caller-selected path.
The [endpoint directory extension](LINUX-CUSTODY-PROVISIONING.md#endpoint-directory-extension)
defines how the directory is added to new and existing accounts.
A missing directory is unavailable and the client never creates it.

Only the supervisor for the active, reserved generation may bind the name.
An occupied name refuses publication. No path unlinks an occupant to make
binding succeed. After binding, the supervisor retains the named socket object
and checks its identity before enabling the listener. A client requires an
owned, same-mount socket leaf with one link. It opens that leaf without
following it, retains its identity and makes one nonblocking connection by name
through the validated `/proc/self/fd` of the retained directory. It rechecks the
named and retained objects after the connection. A successful connection is
only a candidate for the peer checks and possession proof above.

On close or cancellation the supervisor stops admission and closes the listener.
The publishing supervisor checks the named leaf against its retained object
before unlinking it through the retained directory as part of shutdown. The
check and unlink are separate operations within the same-account trust boundary.
A missing or changed name cannot authorize removal of a different object. A
crash may leave a socket inode with no listener. A client refuses a failed
connection and leaves that inode alone. Socket presence, disappearance or
connection refusal never restores an opening permit or establishes that the
previous processes exited. Existing opening and closure rules still apply.

The locator check covers absence, one candidate connection, a duplicate bind,
replacement after the reader pins the leaf, replacement before cleanup, clean
unlink, a dead publisher's residual inode, an invalid credential ID and a
symlink. It exercises filesystem and socket behavior in an isolated account
fixture. Installation validation, generation ownership and proof require their
own integration checks.

## Fixed control frame

Every packet is exactly 112 bytes; there is no variable payload.

| Bytes | Meaning |
|---|---|
| 0–7 | Magic `VVIPC01` followed by NUL |
| 8 | Version 1 |
| 9 | Sender role: initiator 1, responder 2 |
| 10 | Kind: hello 1, probe 2, reply 3 |
| 11–15 | Reserved, all zero |
| 16–63 | Six store-selection u64 values, big endian |
| 64–95 | Bootstrap generation, 32 bytes |
| 96–103 | Per-direction sequence u64, big endian |
| 104–111 | Public control value u64, big endian |

Both sides send hello with sequence/value zero, then validate the opposite
role's hello against their independently selected binding. Successful handshake
permits only an initiator probe and matching responder reply. Subsequent
per-direction sequences start at one, advance without wrapping and must match
exactly. Replays, skipped sequences, unexpected kinds/roles, altered bindings,
reserved bytes or lengths refuse. Repeating the handshake also refuses.

This is a control probe, not readiness for credential use. Its public value is
not an enrollment challenge or a container for files, private data or secrets.

## Failure and descriptor ownership

The channel checks its peer pidfd before and after I/O and kernel credentials
for each received packet. Nonblocking send/recv and poll use a monotonic
deadline, including checks after I/O before reporting success. A check does not
atomically prevent the peer from exiting afterward. Scheduling and kernel calls
do not provide a hard real-time cancellation guarantee.

An error, timeout or invalid packet permanently makes the channel unusable;
there is no silent retry handshake or generation replacement. This error is
not a successful lock, native-exit receipt or resource-release acknowledgement.
The channel does not own the signing gate's publication or cleanup protocol.

Reception requires exactly one correctly sized SCM_CREDENTIALS record. This
version rejects descriptor transfer and unknown ancillary records. It receives
with MSG_CMSG_CLOEXEC, closes every visible SCM_RIGHTS descriptor before any
rejection, and rejects truncated data or ancillary control. Linux closes file
descriptors discarded by ancillary truncation; that does not close descriptors
already delivered to the receiver. See
[recvmsg(2)](https://man7.org/linux/man-pages/man2/recvmsg.2.html).
An unexpected SCM_PIDFD is also closed and rejected. The pair factory does not
enable SO_PASSPIDFD; recognizing that descriptor-bearing kernel record is cleanup,
not permission to transfer descriptors in this protocol.

## Acceptance scope

The native fixture must check valid round trips and rejection of wrong store,
wrong generation, replay, forwarded-descriptor sender, received file descriptors,
peer death, timeout, short packet and oversized packet. It must independently
observe owned process identities and native exits, and check descriptor retention
or closure without treating an external timeout as successful cleanup.

Passing these checks does not enable signing over this channel or authenticate
future pathname-based discovery. Replacing a session of the same store still
requires the prior native exits, fresh explicit unlock, admission and generation
conditions in the isolation contract. Local capture and the central encrypted
service receive no new dependency on this control channel.
