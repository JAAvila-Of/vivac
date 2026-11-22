# Local custody bootstrap and admission boundary

This contract governs local admission for Linux device credentials.
`vivac device-key create-linux` creates a credential in the provisioned
account store. The private [endpoint proof](LINUX-ENDPOINT-PROOF.md) uses
that admission. Public Linux reopening and enrollment signing are unavailable.

The [connected-peer component](LINUX-CUSTODY-CONNECTED-PEER.md) identifies a live
kernel peer. The [reservation](LINUX-CUSTODY-RESERVATION.md) excludes cooperative
contenders for a physical directory. Neither establishes that the peer may act
as custodian of a selected credential. This contract defines where the missing
credential expectation must come from before that peer can be authenticated.

## The client needs an independent expectation

The experimental package manifest describes a public key, device and credential
identifiers, ciphertext digest and size. Those fields do not authenticate their
own origin. Replacing the key and manifest together can preserve structural
validity. PackageLease retains and revalidates anchored objects and bytes; it
does not parse an admitted credential identity or certify creation history.
The [OpenSSH profile](LINUX-OPENSSH-PROFILE.md) keeps package checking, native
identity binding and operational admission separate.

A peer returning a key and a valid signature under that key has shown
self-declared possession. It has not supplied an independent reason to trust
the key. The existing [enrollment proof](DEVICE-ENROLLMENT-PROOFS.md) deliberately
has that narrower meaning and must not become an endpoint handshake.

The client trust source is local installation admission state produced by the
trusted attended creation flow. It is selected and
validated independently of the endpoint and user-selected package. A package,
socket, remote response, command argument or imported record cannot designate
itself as that source. The creation command and the private endpoint verifier
select the fixed namespace internally.

## Producer-only creation admission

An admission record may be committed only after the trusted creation procedure
has completed all of the following:

1. The person requested attended creation and completed the generated-secret
   delivery and confirmation required by the
   [secret policy](LINUX-OPENSSH-SECRET-POLICY.md).
2. The exact package passed the selected creation, filesystem and provider
   profile, including the policy's generated-secret origin requirements.
3. The exact native key's protected identity, public key and bounded possession
   check agreed with the identity the creator intended to publish.
4. The producer stopped admitting temporary native work, observed the exits of
   its owned temporary agent and loader, and completed the required drain and
   resource release. Creation leaves the credential unavailable for signing;
   uncertain native exit does not earn a creation/admission acknowledgement.
5. The package was published successfully and the creation procedure committed
   its corresponding local admission state before acknowledging admission.

The initial native binding is checked locally by the trusted foreground
procedure against its owned native process, not through a discovered endpoint.
It cannot depend on the endpoint authority it is intended to establish. The
[initial native binding](LINUX-CREATION-BINDING.md) specifies its independent
purpose and complete-record challenge. The Linux creation command runs that
check on the generated package and observes native closure before publication.
The check has a purpose distinct from enrollment.

This is provenance of a trusted procedure, not a caller-provided assertion
that those steps occurred. There is no generic `admit_existing` route, imported
producer flag or self-signed receipt that upgrades an experimental package.
The admission writer must receive its evidence from the creation procedure;
it must not expose those facts as independently assertable CLI/MCP booleans.
Uncommitted producer bookkeeping, if retained, is not admission and cannot be
used by a separate client to authorize activation.

The record belongs to private application state of the local installation,
outside the selected package, work tree, provenance log, artifact custody and
central synchronization service. It contains only the public credential
expectation and the creation/profile/policy provenance needed to validate it.
It contains no private key, unlock secret, decrypted payload or content key.
OS ownership, safe path handling and access policy protect this state within
the profile's trusted-OS-user boundary. It is not tamper-proof storage against
the endpoint administrator or a malicious process acting as that user.

The [installation-root contract](LINUX-CUSTODY-ROOT.md) selects explicit
administrator provisioning, a fixed system-state prefix, an independent binding
and private resident package/admission slots. The Linux
[`device-key provision-linux` command](LINUX-CUSTODY-PROVISIONING.md) prepares
the fixed account slot.
[Candidate fixed-namespace selection](../tools/native-key-store-probe/review/RUST-FIXED-ROOT.md)
opens the namespace root independently of environment and caller paths.
The supplied-root candidate writer tests the
separate [installation publication transaction](LINUX-CUSTODY-PROVISIONING.md).
The [resident creation-admission transaction](LINUX-CREATION-ADMISSION.md)
specifies the format, publication order and retained reader used in the Linux
binary. A version marker,
installation identifier or digest alone cannot establish that provenance.
Persistent records must not treat current kernel device numbers or connection
PIDs as stable installation identities. Physical objects are freshly anchored
and revalidated during activation.

## Admission does not activate a session

A separate client must obtain its expected credential identity and applicable
profile revision from verified local admission state. It retains the admitted
expectation while it anchors and validates the selected package. The endpoint's
own assertion of those fields cannot replace that lookup. Changed admission
state or package identity invalidates the in-flight attempt; there is no
silent reread that turns the attempt into admission of another credential.

| Evidence | What it may establish |
|---|---|
| Valid local creation admission | Which public credential and creation/profile provenance the installation recognizes |
| Current anchored package and provider admission | Which exact local objects and native provider this activation selected |
| Cooperative reservation | Exclusive cooperative use of that physical directory while the reservation is held |
| Explicit foreground unlock | This activation supplied fresh attended unlock input under the secret policy |
| Live peer identity and current message credentials | Which process is communicating on this connection |
| Private endpoint proof against the independently expected key | Possession bound to the selected live connection and session context |
| Current request authorization | Which specific operation the requester may perform; it is not implied by the other rows |

Readiness is published only after all activation conditions have actually
succeeded. A creation record, successful connection or retained public key does
not load a key, imply fresh human presence or authorize another session. The
[isolation contract](LINUX-CUSTODY-ISOLATION.md) still requires independent
native-exit evidence and fresh unlock for replacement. Missing prior exit
evidence refuses replacement even if local creation admission is valid.
Historical completion of temporary creation work does not establish exit of
any later native session; a creation record cannot serve as its exit receipt.

## Endpoint proof is a separate protocol gate

The separate [Linux endpoint-proof profile](LINUX-ENDPOINT-PROOF.md) selects the
experimental domain and fixed encoding. This bootstrap contract adds no signer
entry point or authenticated result. Implementation must require:

- An independently admitted expected credential, never a verification key
  selected by the responding endpoint.
- A fresh verifier challenge and a new session generation, with single-use
  attempt state, fixed canonical encoding and bounded input/output/work.
- Binding to the selected credential, currently anchored store, peer roles
  and the live kernel peers. The signer must derive its own process/session
  context rather than sign caller-supplied claims about that context.
- A distinct reviewed purpose that cannot produce an enrollment proof or
  arbitrary-message signature. The existing signing gate must order it against
  lock, cancellation, lease invalidation and publication.
- Explicit refusal of wrong keys, stale attempts, substituted stores,
  forwarded/relayed context and lost liveness, with no prompt, unlock or retry
  initiated by a non-terminal requester.

Even a correctly bound endpoint proof would authenticate only this local
custody relationship. It would not establish membership, permission to enroll,
fresh human approval of an organizational change or globally current authority.
Those remain governed by the [shared authority contract](TEAM-AUTHORITY.md).
The central self-hosted service receives no unlock secret or client private key.

## Failure, copies and restoration

Publication and local admission have distinct failure states. An encrypted
package left without a committed admission record remains unavailable for
activation; neither its valid structure nor a retry upgrades it automatically.
A committed record whose selected package is missing or different also refuses.
The [resident transaction](LINUX-CREATION-ADMISSION.md#visibility-durability-and-failures)
specifies crash behavior and separates logical admission commit, durability and
acknowledgement, with no automatic orphan adoption. A filesystem rename alone
is not a cross-store transaction.

Copying, relocating or restoring a package or installation state must not
silently create a new local admission. Such operations need their own explicit
rules and evidence. The root contract defines the selected profile's location
rules and the limits of detecting a restore from local bytes. Ordinary tree
backups and remote synchronization cannot
bootstrap this trust. Existing records are not proof that a restored policy is
current, that an old native process exited or that a credential remains
organizationally authorized. This contract does not promise hardware-backed
rollback detection against a trusted administrator restoring an entire endpoint.

## Acceptance of the admission boundary

The [candidate metadata-storage component](LINUX-ADMISSION-STORAGE.md) tests
exclusive publication and anchored reads of public fixtures. It supplies neither
the trusted root resolver nor producer evidence required by this contract.
The separate [resident snapshot fixture](../tools/native-key-store-probe/review/RUST-RESIDENT-READER.md)
composes root, version 2 record and package checks without claiming that a
trusted attended producer wrote its synthetic records.

The local producer/reader and persistence contract governs the use of a public
key as an endpoint trust anchor. Tests must
distinguish real producer evidence from imported metadata, detect substitution
and partial publication, refuse unknown revisions and unavailable admission
state, and keep ordinary local capture independent of these failures.

The endpoint-proof contract then needs its own codec, native signing,
transport and lifecycle evidence. Passing connected-peer or reservation tests
does not satisfy these requirements. An independent verifier refuses when its
admission or the private proof exchange is unavailable.
