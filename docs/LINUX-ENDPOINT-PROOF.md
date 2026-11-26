# Local endpoint possession proof

This selects the first endpoint-proof profile under the
[local bootstrap contract](LINUX-CUSTODY-BOOTSTRAP.md). It is separate from
enrollment and the [initial creation binding](LINUX-CREATION-BINDING.md).
The Linux binary implements the proof in its private foreground custody and
[discovery paths](LINUX-CUSTODY-IPC.md#candidate-independent-verifier-rendezvous).
Public reopening, enrollment signing and recovery remain separate work.

## Authority and retained context

The verifier independently opens `CurrentAdmission` using only the intended
credential ID. It retains that value, its canonical record and expected public
key throughout the attempt. The endpoint cannot supply a replacement record,
key, installation ID or package path. The selected installation must use the
trusted attended producer; experimental metadata does not supply that history.
The Linux creation command writes this admission and the private verifier opens it.

Both roles retain the connected Unix `SOCK_SEQPACKET` socket with CLOEXEC and its kernel peer
reference. They obtain stable local credentials and the expected peer through
the [connected-peer checks](LINUX-CUSTODY-CONNECTED-PEER.md). Each message requires
the exact current `SCM_CREDENTIALS` sender and live pidfd checks before and after
I/O. Unsupported peer acquisition, forwarded descriptors, ancillary data or
changed credentials refuse without fallback. Neither a PID file nor an endpoint
claim can construct this context.

The custodian must already own an explicitly unlocked native session, its
reservation, provider and resident package. It derives its own generation,
admission, physical store selection, local identity and connected verifier
identity. It does not sign the verifier's claimed description of these facts.
The generation is a fresh nonzero 32-byte OS draw for that activation, retained
by the session rather than reconstructed from a request or runtime file.

The physical selection is six words in this exact order: package directory
device, package directory inode, key device, key inode, manifest device,
manifest inode. It must come from the current
retained resident objects and agree with the native session's lease. Mandatory
mount/profile and byte revalidation remain separate from these public numbers.
They are not durable identities, portable selectors or bearer capabilities.

The experimental context sources have separate lifetimes. A native session
creates its generation internally. `EndpointSessionWitness` can be issued only
while the session is ready and idle; an issued witness remains usable during
work in that same ready epoch. It holds a weak gate reference, not a package
lease or a borrow that prevents session work. Closing, dropping or changing the
epoch invalidates it. Its getters check that lifetime before and after access;
they do not independently check the native agent or resident lease.
`NativeSession::validate_endpoint_witness` checks the witness against the owning
session and its actual lease. A failed check permanently invalidates the witness.

`CurrentAdmission::store_identity` derives the six physical words from its
retained directory, key and manifest descriptors, with resident validation
before and after access. The attended producer compares this selection with
the original published descriptors. No record field supplies those identities.

`ConnectedPeerContext` retains a CLOEXEC duplicate of the connected socket and
its kernel peer reference. Its checked identities describe that connection's
historical peer and the current local process. They cannot replace the current
message's `SCM_CREDENTIALS` checks. Failed validation permanently refuses further
access. These context sources do not implement endpoint signing, verification
or proof-slot admission; the complete proof still requires the protocol below.

Peer tuples are PID, UID and GID, each encoded as unsigned 32-bit big endian.
PID is 1 through `i32::MAX`; UID/GID are nonzero and exclude `u32::MAX`.
Verifier and custodian must be distinct processes with matching stable UID/GID.
Both roles must observe compatible kernel identity representations. A namespace
mapping that produces different tuples cannot be repaired by accepting peer
numbers from the request; the attempt refuses.

An expected-peer connection compares the pidfd supplied by the kernel socket
with a live pidfd retained from the launch context. It checks both references
before and after reading their process identities from retained procfs. This
comparison precedes the challenge and leaves the per-message sender checks
in force. The launcher fixture tests two independent connections with the
expected references and with the references exchanged.

## One attempt, two public frames

The verifier generates a nonzero 32-byte challenge from the OS. The custodian
generates a separate nonzero 32-byte connection nonce for this accepted proof
attempt. Neither nonce is a secret or grants authority. There is one challenge,
at most one native signing request, and at most one response on this proof
connection. There is no signed ACK or retry message.

The custodian admits at most one pending proof attempt per native session.
An occupied proof slot refuses before random acquisition or native work; it
does not create a queue or automatic retry. This profile adds no multi-client
accept loop or discovery mechanism.

The experimental `EndpointProofAttempt` implements only this temporal
reservation. Its session-owned atomic slot refuses contention immediately with
`WouldBlock`. The attempt holds a session witness and a slot guard, without a
package lease or a strong session-lifetime reference. Its monotonic deadline is
the earlier of the caller's deadline and 30 seconds after reservation; later
checks cannot extend it. Consumption occurs before validating the operation
against the owning session. A second consumption or any failed validation
permanently invalidates that attempt. The slot remains occupied until the
attempt is dropped, including after consumption or failure, so post-operation
checks cannot overlap a newly admitted attempt.

Successful reservation, consumption or lifetime validation supplies no signature
or authenticated result. This temporal API does not receive a challenge, draw
nonces, perform I/O or close a proof connection. The custodian retains the
consumed attempt through the last check and shuts down the connection on
refusal. Dropping an attempt permits a fresh reservation only while the same
native session remains eligible; it does not retry or revive the old attempt.

Both frames start with this 16-byte header:

| Bytes | Value |
|---|---|
| 0–7 | `VVEP01` followed by two NUL bytes |
| 8 | Endpoint-proof version 1 |
| 9 | Kind: challenge 1 or proof 2 |
| 10 | Sender role: verifier 1 or custodian 2 |
| 11–15 | Zero |

A challenge is exactly 48 bytes: kind 1, role 1, followed by the verifier's
32-byte nonce. It contains no key, record, store, peer or generation claim.

A proof is exactly 176 bytes: kind 2, role 2, then the echoed challenge nonce
(32 bytes), custodian generation (32), connection nonce (32), and raw Ed25519
signature (64). The generation and connection nonce remain untrusted response
data until verification succeeds. The response carries no verification key or
context selected by the endpoint for the verifier to adopt.

All nonces/generations must be nonzero. Unknown versions, lengths, kinds, roles,
reserved bits, extra fields and descriptor-bearing messages refuse. A decoded
signature is only data; parsing never produces an authenticated result. These
frames are not accepted by the existing 112-byte control protocol, and that
protocol gains no variable payload or signing operation.

Transport must receive with `MSG_CMSG_CLOEXEC`, require exactly one correctly
sized `SCM_CREDENTIALS` record and reject data/control truncation. Every visible
`SCM_RIGHTS` or `SCM_PIDFD` descriptor is closed before rejection; unknown or
duplicate ancillary records also refuse. Kernel-discarded truncated descriptors
do not replace cleanup of those already delivered. These are mandatory transport
obligations, not guarantees supplied by parsing a byte slice.

## Exact signed bytes

The signature is plain Ed25519 over exactly 238 bytes, with no SSHSIG envelope,
JSON, hexadecimal wire fields, variable strings or caller-supplied signing data.

| Bytes | Value |
|---|---|
| 0–29 | ASCII `vivac.linux.endpoint-proof.v1` followed by NUL |
| 30–31 | Fixed verifier role 1, custodian role 2 |
| 32–37 | Zero |
| 38–69 | SHA-256 of the complete canonical version 2 admission record |
| 70–117 | Six physical store-selection u64 words, big endian |
| 118–149 | Custodian activation generation |
| 150–181 | Verifier challenge nonce |
| 182–213 | Custodian connection nonce |
| 214–225 | Verifier PID, UID, GID |
| 226–237 | Custodian PID, UID, GID |

The custodian constructs these bytes internally from its retained context and
the received challenge nonce. The verifier reconstructs them from its own
retained admission, store and connected peers, its original challenge and the
two candidate response values. Neither role accepts a serialized transcript as
authority. Role positions cannot be exchanged or supplied by a caller.

The proof's echoed nonce must equal the verifier's original challenge byte for
byte. An altered echo refuses even if reconstructing a message from the original
nonce could otherwise verify. Only the generation and connection nonce are
candidate context values taken from the response.

The expected key comes only from the verifier's retained admission. Verification
must be strict and must reject weak keys, altered signatures, the enrollment
purpose, creation purpose, wrong credential/store/record, wrong peers or roles,
stale challenges, changed generation, forwarded contexts and lost liveness.

## Consumption, ordering and failure

The verifier attempt expires 30 seconds after local challenge issuance. The
custodian has a separate at-most-30-second local budget from accepting the
challenge. No wall-clock timestamp or asserted remaining time travels over the
wire. Operations use the earlier caller deadline; this is a cooperative deadline,
not a hard syscall or scheduling bound. Random acquisition is nonblocking with
no weaker fallback.

Each role consumes its attempt before performing its one proof operation.
Malformed input, timeout, verification failure, cancellation or any other error
permanently closes that attempt and connection. There is no nonce reuse, silent
reconnect, implicit reread of admission or refresh of the selected peer.

The native identity must match the exact expected public key and protected
device/credential comment. A purpose-specific typed constructor must connect
the internally built message to the native signing identity. It must not accept
arbitrary bytes through the enrollment or creation entry point. The existing
signing gate orders work and publication against lock, cancellation and lease
invalidation. Admission, provider, lease, connection and deadline are checked
again before exposing any proof or successful verification. A signature produced
after an invalidating event must be discarded; proof bytes and a successful
verification result must not be exposed.

No request may prompt, unlock, reload, spawn a replacement session or retry
native work. A verified proof only binds this connection to possession of the
independently expected local credential at the checked instant. It does not
establish organizational permission, fresh human approval, previous-session exit,
readiness or safe replacement. Liveness checks cannot prevent a later exit.

The signing worker checks the retained custody independently of the verifier.
When it detects peer loss before starting the native exchange, it can discard
the attempt without sending a native request. Once that exchange has started,
the worker can discard a result on peer loss only after receiving and
verifying the complete native reply under the existing custody checks. This
releases the attempt after the worker is joined. Loss of the lease, failure of
the provider, or uncertainty about a partial exchange still closes admission.
The result is taken from the gate before the final peer and transcript checks,
so a peer lost at that point cannot leave a result occupying the next turn.

Observing a close stops admission under the same gate lock that orders an
endpoint send. A result that has not crossed that boundary cannot be sent.
The creator thread still owns native teardown and the existing close
acknowledgement. The independent listener's I/O thread watches the authenticated
close channel while the native worker is in flight.

## Candidate codec boundary

The [candidate codec](../src/device_keys/linux_endpoint_codec.rs) implements
only the two fixed frames and signed-byte encoding. It is shared by the
experimental helper library and its executables. Its public data structs accept
fixture fields; they
cannot supply the locally derived context required by a native signing
constructor. It performs no random acquisition, expiry, single-use accounting,
I/O, signature verification or construction of an authenticated result.

Tests compare fixed hexadecimal vectors and field offsets, reject malformed
headers, sizes, nonces and peer representations, and check that the existing
enrollment identity constructor refuses the endpoint purpose. Signature bytes
remain uninterpreted frame data, including an all-zero candidate signature;
decoding such a frame establishes no possession. Validation of a complete
exchange requires the native signing and transport context described below.

## Sealed native identity preparation

`NativeSession::serve_endpoint_proof` reserves the temporal attempt before
receiving a challenge. It takes ownership of a `CurrentAdmission` and uses the
connected peer context captured by the endpoint transport. Preparation
consumes the attempt before native work. It derives the record digest, physical
store selection, session generation and kernel peer tuples from those retained
objects. It requires the admission's physical selection to agree with the
session witness, and draws a fresh nonzero connection nonce without a weaker
fallback. Roles and the endpoint domain are fixed by the encoder.

Each context checkpoint matches the physical store to the session witness with
one bracketed resident validation. The key, comment and canonical bytes come
from the same validated record. Session and peer checks surround the physical
read, and no admission validation is cached across an I/O boundary.

The preparation asks the existing native agent for its identity list. There
must be exactly one Ed25519 identity, with the admission's exact public key and
protected device/credential comment. The endpoint message cannot enter the
enrollment or creation constructor. The resulting `EndpointNativeIdentity`
retains its attempt, admission and connection context, with irreversible
invalidation on a failed check. Its fields are private and the prepared
identity does not expose a generic signer. It retains no additional package
lease or strong session-lifetime reference.

The new identity exchange uses a nonblocking CLOEXEC socket. Connection and all
partial reads and writes share the earlier of the attempt deadline and a
three-second exchange deadline. Interrupted operations do not renew it; an
occupied Unix socket backlog refuses without reconnecting. Context, peer and
deadline checks surround I/O and preparation. These are cooperative checks,
not a hard scheduling guarantee.

The custodian's `EndpointProofConnection` takes ownership of a connected Unix
packet socket. Its single receive authenticates the current message sender,
accepts no ancillary descriptors and derives the peer context from that same
socket. A failure consumes the attempt and shuts down the connection.

## Candidate signature extraction

`NativeSession::serve_endpoint_proof` runs the existing native signer under
the owning session's gate. Its signing checkpoints revalidate the retained endpoint
context, and collection uses the attempt's remaining deadline. Extracting
the result requires a joined worker and a published request in the same
ready epoch. Closing or changing the epoch refuses extraction through this
path, including a result waiting to be taken. It checks the signature against
the retained transcript and revalidates the session and connection. The
generation in the frame comes from those signed bytes. The gate orders one
nonblocking response send on the original socket against closure. A blocked
send refuses the attempt. The caller can retain the spent connection so the
verifier can complete its liveness checks.

A deterministic signature test preserves every transcript field except the
generation. A valid previous signature fails both as an unchanged frame and
when relabelled for the successor. A freshly signed frame for the successor
passes. A native gate fixture separately exercises the ready-epoch extraction
and closing race with OpenSSH. The connected-peer fixture exercises the
transport with disposable frame data.

## Candidate verifier

`EndpointProofConnection::verify_endpoint_possession` owns the verifier's
independently opened admission for the exchange. Before sending, it checks the
retained record, physical store selection and expected public key. It issues a
fresh challenge, authenticates a single proof message and requires its exact
echo. The verifier reconstructs the transcript from its admission, challenge
and kernel identities. It accepts the candidate generation and connection
nonce only after strict signature verification. Before returning the verified
generation, it checks the original admission, peer and deadline again.

The result describes possession at the checked instant. Operational session
replacement still requires its separate custody and closure evidence.

## Acceptance and trust limits

Before operational use, tests must cover the complete admitted producer/reader,
typed native signer, actual connected peers, message credentials and gate
ordering. Required negatives include field mutation, role reflection, wrong
key/comment, replay, cross-connection/context substitution, forwarded sender,
peer death, deadline/cancellation/lock, descriptor transfer and changes to any
retained admission/store/provider object. Tests must distinguish a malformed
frame, kernel refusal, signature refusal and uncertain cleanup.

A codec fixture establishes canonical bytes and refusals. Synthetic peer
tuples or records cannot mint connection authority or authenticate an endpoint.
The connected-peer guest with kernel 6.8 exercises the authenticated exchange
with disposable proof bytes. The separate native creation guest can exercise
the full candidate exchange with a synthetic admitted credential. It closes
the producer, loads that credential through the restricted OpenSSH pair and
requires an independent verifier process to reopen the admission and confirm
the same signed generation. The synthetic secret moves through a private pipe
from the fixture observer and through the private loader terminal. The probe
checks session closure and both process exits.

In that fixture the supervisor remains single-threaded through the native
launch, role registration and load. It enters the worker-thread phase only
after its owner has frozen the registered child set. This disposable path
supplies no keeper handoff or operational authority to reopen a store.

The private foreground path also has native acceptance runs with the Vivac
binary on isolated Ubuntu 24.04. A separate process verified possession from
its own admission while the keeper held the active session. The runs observed
contending requests, explicit close with a signing worker active, and loss of
the controlling terminal. The listener also refused a challenge with the wrong
role and a second request on a spent connection. A fresh verifier still
completed the exchange. The explicit close returned a clean opening permit
after teardown. Terminal loss removed the listener without returning the permit.

An isolated product run also verified two successive generations for the same
admitted credential. A second keeper was refused before unlock while the first
held the store. The observer checked the first keeper, supervisor and agent
exits before starting a new attended unlock. The two native signatures verified
against independently reconstructed transcripts and the admission's public key.
The first signature failed against the successor's transcript and also failed
when only the generation in the first transcript was changed.

Same-UID processes and the endpoint administrator remain trusted. Binding peers
rejects a proof relayed through a different process or mismatched connection
context; it does not identify the original requester behind a trusted process
that deliberately proxies work, or protect against that process's internal
descriptor sharing. Request authorization remains separate.
