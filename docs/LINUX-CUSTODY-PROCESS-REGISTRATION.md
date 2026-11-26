# Process-reference registration transport

The experimental registration channel lets an external observer retain process
references supplied by an authenticated supervisor. It is a transport boundary,
not a complete process roster or permission to unlock a credential. Linux
custody remains unavailable in the product.

## Separate channel

The existing custody `Channel` continues to reject transferred `SCM_RIGHTS` and
`SCM_PIDFD` descriptors. Registration uses a separate inherited Unix sequenced
packet socket with credential delivery enabled. The receiver's expected
supervisor comes from its actual `SpawnOwner` child entry; the sender derives
its expected parent from the kernel. Historical socket credentials alone cannot
authenticate a message sent through an inherited endpoint.

Each exchange uses a fresh operating-system random challenge and one absolute
deadline, capped at three seconds. A challenge, response and acknowledgement
each carry the protocol tag, version, message kind and challenge. The response
has exactly two transferred references. The other messages have none. Each
message must carry the current expected sender's kernel credentials. Successful
acknowledgement confirms transport acceptance only.

The sender duplicates references from two different entries in its own
`SpawnOwner`. The receiver checks descriptor ownership, close-on-exec, the
pidfd interface and live readiness. Truncated packets, unexpected ancillary
data, extra references, mismatched challenges and invalid credentials refuse.
Every visible transferred descriptor must be adopted or closed, including on
rejection. An error invalidates the exchange permanently; a successful exchange
cannot be consumed again. The protocol endpoints are bound to their original
process and thread.

## No inferred session authority

### Complete owned-child capture

`SpawnOwner::prepare_child_registration` captures all entries of one owner,
without accepting a caller-selected pair of tokens. The current registration
format requires exactly two distinct, live owned children. An attempted capture
permanently prevents that owner from launching another child, including after
a failed capture or after the returned registration is dropped. It does not
start shutdown, release resources or replace the reserved session generation.

The opaque `OwnedChildRegistration` borrows its owner and retains duplicates of
its process references. Validation checks the original process and thread,
owner health, the complete entry set and live references. A validation failure
is permanent. `RegistrationSender::send_registered` sends this retained pair
through the existing exchange, validating it before and after the exchange.
The earlier token-selected `send` remains a candidate transport operation.

This establishes a closed set of entries in that owner, not the complete set of
processes capable of using a credential. It does not assign roles, establish
acquisition conditions or descendant coverage, bind the received candidates to
an admitted store or generation, preserve an independent keeper, or authorize
secret entry. Those remain separate integration requirements.

### Received candidates

The received result owns opaque candidate references. It exposes no descriptor,
exit observation, signing operation or constructor for a closure capability.
Two received references do not establish different process identities,
process-wide observation semantics, their parentage, their roles, or the
complete set of processes able to use a key.
The transport does not bind them to an admitted store or a session generation.
It neither holds the store reservation nor preserves exclusion after losing
the observer. No credential is unlocked by this exchange.

The [complete prior-session closure requirements](LINUX-CUSTODY-EXIT-WITNESS.md#complete-prior-session-closure)
remain mandatory. Integration must establish acquisition conditions, actual
launch lineage, complete coverage, admitted-store and generation binding,
registration before secret access, and independent evidence and exclusion
across supervisor failure. Losing evidence must refuse replacement. A received
candidate cannot be promoted through a public constructor accepting caller
claims. No service installation, restart recovery or reboot exception is
introduced here.

## Generation must precede secret access

The session generation originates when `SpawnOwner::new_reserved` validates and
takes ownership of the store reservation, before that owner can launch a native
process. Before launch, it remains private to that owner. `NativeSession::new`
consumes the owner and derives the same generation after checking its context
and reservation; it does not create a replacement generation after the loader
has used the secret.
An unreserved owner cannot supply a session generation. Closing the owner or
releasing its reservation prevents further derivation.

This establishes the generation's ordering and provenance only. The transport
challenge remains separate from the session generation. The owner exposes no
public pre-launch generation getter, registration permit or secret-handoff
permission. The existing `EndpointSessionWitness::generation` reads the value
only after validating a live, ready session; it cannot establish registration
before secret access. Complete registration must bind its retained references
to this actual generation and the admitted store before permitting an attended
unlock. Cross-process reservation
delegation and the external keeper remain integration requirements.

## Preparation from retained admission

`ReservedAdmittedSessionMaterial::prepare_launch` consumes the admitted material
and its existing reservation into an opaque `AdmittedLaunchPreparation`. It
creates the reserved owner internally and retains the original admission,
package lease and sealed ciphertext. Its private binding derives the physical
store identity, the owner's actual generation and the SHA-256 digest of the
retained canonical admission record. The caller supplies none of those values.

`validate` checks the retained admission and material, the owner's process and
thread context, its reservation, and the complete binding before and after
validation. A failed check remains failed even if the namespace is restored.
The preparation keeps exclusion while retained; creating a second independent
reservation must still refuse. The public preparation API exposes no
constituent resources or binding values and does not launch a process, load a
key, sign or authorize secret input.

The laboratory exercises an internal launch operation on the preparation. It
uses the retained reserved owner to start two fixture processes and freeze
their registration. The registration borrows the preparation and rechecks its
admission and generation together with the live process references. A launch
failure consumes the attempt. The same owner keeps its reservation and any
processes already started. Exit observation covers the owner's direct children
and is available after admission invalidation. It retains the reservation
and does not acknowledge complete session closure. Production provider and
role selection still require a trusted launch path.

The child registration also retains the device and inode of each selected
executable from the owner's retained launch descriptors. An admitted
registration can compare two expected executable descriptors against those
identities in registration order. A mismatch invalidates the registration.
This records the initially selected file objects. Provider admission, role
selection, arguments, library provenance and descendant coverage still require
separate evidence. The check does not inspect the running process image.

The experimental OpenSSH creation writer selects the agent and loader files
together from its retained Ubuntu provider. Before handing the generated test
secret to the stopped loader, it freezes the owner and checks both receipts
and executable objects in agent, loader order against that provider. Its
local session and exit checks refer to those same receipts. The existing
fixed foreground agent and loader launch configurations are used in this
path.

The admitted launch probe uses the same Ubuntu provider selection and the
retained sealed ciphertext of an existing credential. Its reserved owner
starts the foreground agent, verifies the private socket peer and empty agent,
and starts the loader. At the private prompt it checks the two ordered receipts
and executable objects against the retained provider. The probe then verifies
exit of both direct children.

On the Ubuntu amd64 profile, the admitted probe starts both provider processes
with a seccomp filter installed by the trusted child setup before exec. The
filter checks the system call ABI, rejects task creation and prohibits creating
an io_uring instance. The owner records that the filter was required by each
successful launch. The frozen registration requires both launches to have that
restriction before accepting the pair at the prompt. A disposable marker tests
the restriction across exec. This covers task creation by the registered
provider pair in the pre-unlock probe.

The disposable crash case adopts the same restricted pair through the
authenticated keeper exchange while both provider processes are still alive.
A separate authenticated channel gives the keeper a duplicate of the private
PTY master. After confirming the offer, the keeper signals the supervisor
through the supervisor's retained pidfd. The keeper acts as a subreaper and
checks the recorded SIGKILL status of both provider processes using the adopted
pidfds while retaining the master. A separate reservation attempt remains
excluded even after the keeper drops its protocol handle. The fixture can
acquire the reservation after the dedicated keeper exits. This teardown
carries no operational closure or unlock authority.

This preparation joins admission and generation locally. It does not identify
the transferred candidate processes for an external keeper. Integration must
still verify the transferred references, process roles and coverage, and
preserve exclusion and evidence across supervisor failure. The reservation
remains bound to its acquiring process; this preparation does not delegate it
to an external keeper or establish prior-session closure.

`AdmittedLaunchPreparation::keeper_admission_commitment` derives the admission
contribution for a keeper offer. It validates the preparation before and after
derivation and accepts only a nonzero 32-byte challenge. A zero challenge
returns `InvalidData` before accessing the preparation's validity latch.
SHA-256 covers the ASCII domain `vivac.linux.keeper-admission.v1`, one NUL,
the challenge, the reserved owner's generation, the six physical identity
values as consecutive big-endian `u64` values, and the canonical record digest.
The commitment exposes the digest alone. The exchange remains responsible for
challenge freshness, peer authentication, and binding the actual process
references into the offer.

## Keeper confirmation contract

A keeper confirmation requires a separate exchange. The registration exchange
above continues to acknowledge only its two candidate process references.
Before opening the keeper exchange, a trusted launch path must establish the
expected keeper and supervisor and bind the attempt to the retained admission,
physical store, and the reserved owner's actual generation. The supervisor
derives the offer from that preparation and the frozen process roster. An
offer cannot establish its own expected identity or supply an arbitrary roster
in place of the launch path. The keeper also needs an independently acquired
reference to the supervisor. Establishing these inputs is part of integration.

The exchange uses a fresh, single-use challenge, current peer credentials, and
an absolute deadline. The keeper checks the binding, the owned process
references and the exclusive lock on the received directory description before
accepting the offer. Descriptor identity and lock state are separate checks.
An independent open of the same directory can have the same device and inode
without carrying the lock. A descriptor still queued in a socket can also keep
the lock held before the keeper has accepted it.

The experimental `verify_retained_flock` check observes an exclusive advisory
flock on the supplied close-on-exec directory descriptor from the approved
ext filesystem family. It reads a bounded entry from the receiver's own
`fdinfo` on procfs in a trusted mount namespace. The lock row must match the
device and inode of the descriptor. The check ignores the PID printed on that
row as a source of identity. It observes the lock at the time of the read and
leaves the lock and its ownership unchanged.

`CurrentAdmission::retain_keeper_directory` consumes the current admission and
the received directory descriptor. It compares the descriptor's device and
inode with the retained package's identity and checks the exclusive flock.
The returned guard owns both objects. A failed revalidation invalidates the
guard permanently, even if the lock is restored. The guard retains the
descriptor until it is dropped. Offer authentication, generation and process
registration require their own checks.

The keeper adopts the whole validated set locally before attempting to send a
positive confirmation. The confirmation binds the challenge and the exact
accepted offer. Once adopted, the keeper retains the lock description and
process references across failure to send, timeout, disconnect, or supervisor
death. The supervisor must receive and validate the confirmation before it can
advance. It must not retry the same attempt with a different keeper. A live
binding must still be checked at the point of use, since a confirmation records
acceptance at one point in time.

The release condition is the complete prior-session closure requirement, with
the exclusion and evidence still retained at the transition. A missing reply,
EOF, a failed check, or a newly available directory lock cannot substitute for
it. If the keeper or its evidence is lost, the supervisor refuses further
advancement and a replacement refuses reopening. The shared directory
description supplies cooperative exclusion while held. The `StoreReservation`
remains bound to the acquiring process.

### Keeper launch bootstrap

A private bootstrap sends the reserved generation, physical store identity and
canonical admission record digest together with two live process references.
The sender derives the frame and the ordered descriptors from the admitted
child registration. The keeper authenticates the expected live supervisor
through the existing private transport and compares that context to its own
retained admission. It keeps the two expected references and derives the
admission commitment for a supplied nonzero challenge.

Within a launch checkpoint, one validation brackets the physical store
identity, record and ciphertext comparison. Generation and reservation checks
surround that checkpoint. The transport checks peer and deadline at each
send and receive.

The child registration permits a single bootstrap attempt across all channels.
Once the attempt starts, a send failure consumes it as well.

The bootstrap frame is 128 bytes. The header contains `VVKBOOT1`, version 1,
a launch kind byte, and six zero bytes. Kind 0 identifies a generic pair.
Kind 1 identifies the restricted OpenSSH launch at its private prompt in the
pre-unlock probe. The restricted commitment uses the SHA-256 domain
`vivac.linux.keeper-pre-unlock-restricted.v1` followed by NUL. Kind 2 registers
an OpenSSH pair eligible for attended activation and uses the domain
`vivac.linux.keeper-active-restricted.v1` followed by NUL. The body
contains the 32-byte generation, six big-endian `u64` identity words and the
32-byte record digest, in that order. The trusted launch path selects the
supervisor and opens the private channel. The supervisor keeps its original
admitted child registration for the next phase.

### Keeper frames

The separate keeper codec uses fixed frames with a 16-byte header. Bytes 0–7
are the ASCII tag `VVKEEP01`, byte 8 is version 1, byte 9 is the message kind,
and bytes 10–15 are zero. Every frame starts its payload with a nonzero 32-byte
challenge. An offer appends the nonzero 32-byte admission commitment. A
confirmation appends SHA-256 over the ASCII domain
`vivac.linux.keeper-offer.v1`, one NUL and the complete canonical offer frame.
An active closure appends that offer digest and the closed generation.

| Kind | Frame | Bytes | Descriptor count |
| --- | --- | --- | --- |
| 1 | Challenge | 48 | 0 |
| 2 | Offer | 80 | 3 |
| 3 | Confirmation | 80 | 0 |
| 4 | Active closure | 112 | 0 |

An offer and its three ordered references (locked directory, then the two
registered process references) must arrive on the same packet from the
expected peer. The keeper authenticates the current sender and checks the
received objects before confirming. Decoding validates the frame shape. The
caller must also compare the offer to the locally expected challenge and
admission commitment, and compare the confirmation to the current offer.
Matching bytes alone grant no permission.

The initial challenge uses the same bounded packet I/O as process
registration. The keeper derives its expected peer from the retained direct
child, and the child derives its expected peer from the kernel parent and
checks the current sender's credentials on receipt. The keeper generates the
challenge from operating-system randomness. Each endpoint consumes one
attempt, and the challenge accepts zero transferred descriptors. A disposable
launcher fixture creates the private pair, starts its own ELF as the direct
child, and checks that the child receives the challenge before it exits. The
fixture's control response reports completion of the test and carries no
keeper confirmation.

The challenge endpoints can transition to a single offer attempt over the same
socket. The transition preserves the expected peer and the original deadline.
The supervisor sends the offer and all three descriptors in one packet. The
keeper checks the credentials, frame and challenge and retains the received
descriptors with close-on-exec set. The result holds candidate objects for the
later checks against the trusted launch context.

`verify_live_process_pair` compares two ordered pidfds against an independently
retained pair. It reads the kernel's `Pid` row for each descriptor under a
single trusted procfs directory and requires the four process references to be
live before and after those reads. It checks that the expected processes are
distinct and that the offered pair matches their order. The check refuses
thread and autokill pidfds. The trusted launch path supplies the expected pair.

The combined exchange consumes the bootstrap on both sides and preserves the
original socket, peer and deadline. The supervisor derives the commitment from
its registered generation and copies the descriptor already holding the
reservation's flock along with the two registered pidfds. The keeper checks
the offer against its fresh challenge and bootstrap expectation, and retains
the received references and validated directory as a provisional set.

The provisional set keeps the directory locked while it is held. Adoption
revalidates and consumes that set, then occupies the keeper process's one
adoption slot. The adopted admission, directory and pidfds survive a failed
confirmation and dropping the protocol handle. The keeper keeps the set
until its dedicated process exits. That exit is laboratory teardown and
carries no operational release authority.

The keeper can attempt confirmation once after adoption. It sends the digest
of the exact offer on the original channel with zero descriptors. The
supervisor checks the frame, the expected live peer and its retained
registration within the same deadline. A failed check after sending leaves
delivery uncertain. The keeper retains the adopted set and the attempt stays
consumed. Trusted provider selection, process
coverage, complete closure and recovery remain integration requirements. No
current operation advances to secret entry.

After a successful confirmation send and observed exit of all three retained
processes, the disposable keeper can retire a restricted pre-unlock
generation once. It revalidates the retained admission and lock and draws a
distinct successor generation while keeping the same locked descriptor. A
borrowed successor check accepts the current generation and rejects the
previous value. The transition produces no signature, transfers no
reservation, and starts no session.

The disposable keeper can consume the successor once to start a restricted
OpenSSH pair under a duplicate of the original locked directory description.
It derives fresh launch material from its retained admission and binds the
new owner to the successor generation. The keeper retains the original lock
through preparation, startup, both exit receipts and cleanup, including an
attempt that fails. The fixture stops at the private loader prompt and
observes the pair's exit before returning. The attended unlock requires its
own complete closure and activation evidence.

The attended successor uses the same retained keeper and requires the keeper
to collect both original provider exits by pidfd with the expected kill status.
It consumes the successor before trying a fresh unlock. A sealed package, the
current provider and both registered new roles are checked before the
controlling terminal prompt and again before handoff to the private OpenSSH
loader. A different process verifies the signed generation. The fixture
observes native session closure while the keeper still holds the reservation.
The predecessor stops before receiving a secret in this experiment.
The guest first presents an incorrect synthetic secret and checks refusal,
terminal restoration and socket cleanup. It then verifies the positive case
with the same admitted credential.

## Disposable acceptance

The transport fixture uses disposable marker processes and metadata. A positive
exchange must retain references while its supervisor and markers remain alive.
Negative cases must exercise malformed descriptor counts, a non-pidfd,
challenge mismatch, another current sender using an inherited endpoint,
deadline expiry and repeated consumption. Resource limits, source and helper
identities, actual cleanup and each case's outcome belong to the test evidence.
The transport fixture establishes the exchange and retention boundaries.

The restricted OpenSSH fixture uses a synthetic credential. It refuses to
advance before confirmation, while the original processes are alive, and
after a prior advance. A marker pair must refuse even after exit. A successful
advance rejects the previous and an altered generation while a competing
reservation remains blocked. The successor fixture also checks contention
with the new pair registered and again after it has exited.

The keeper experiment must distinguish local adoption from delivery of a
confirmation. After adoption, terminate the disposable supervisor before the
keeper attempts to send. Observe both the failed send and continued exclusion
while the keeper retains its descriptors. Verify each marker's exit using the
keeper's own references. The fixture's final release is an explicit cleanup
step. Its marker set and release do not establish a production closure
capability.
