# Resident creation-admission transaction

This is the selected contract for composing the
[installation root](LINUX-CUSTODY-ROOT.md), resident encrypted package and
[trusted attended producer](LINUX-CUSTODY-BOOTSTRAP.md). The Linux
`device-key create-linux` command runs the composed creation transaction.
Enrollment signing and credential recovery remain separate operations.

## Authority and inputs

The foreground producer obtains the fixed installation internally. A request
cannot supply its root descriptor, installation ID, package path or an existing
record as authority. Device and credential identifiers are opaque canonical
26-character identifiers. Creation gets a separate fresh 128-bit OS-random
creation identifier. No identifier is derived from a path or kernel identity.

The producer must complete the selected generated-secret delivery and
confirmation, package/profile checks, protected native identity comparison and
bounded local possession validation against its own temporary native process.
The [initial native binding](LINUX-CREATION-BINDING.md) defines its distinct
signing purpose, complete-record binding, fresh challenge and lifetime.
The Linux creation command and the disposable composed writer exercise this
check with attended preparation and the internally selected Ubuntu provider.
The storage transaction does not reuse enrollment or endpoint proof semantics
or satisfy that check by itself.

It then stops admitting temporary native work, drains it, observes the required
exits of its owned agent and loader under the closure contract and releases
their resources.
Uncertain exit supplies no evidence. Creation leaves no unlocked signing session.

Only the producer's internal state machine may construct the resulting evidence
value. Its fields and constructor are private; it cannot be deserialized,
imported, supplied by CLI/MCP booleans or constructed from public metadata.
It retains the validated installation and exact intended public credential,
package bytes/digests and completed procedure state. It is consumed once by
the publication transaction. A copied success flag, log entry, process ID,
uncommitted journal or public-key signature is not a substitute.

This is an implementation authority boundary within the trusted program, not
cryptographic protection against a malicious same-UID process or administrator.
Those actors remain trusted by the profile and can forge local public bytes.
No extra local signing secret or organizational permission follows from this
transaction. The central server receives no client private key or unlock secret.

The [private creation producer](../src/device_keys/linux_creation_producer.rs)
is shared by Vivac's Linux creation command, integration tests and the
disposable writer.
The writer owns the observer protocol. The attended factory selects the fixed root
and Ubuntu provider internally before secret delivery. It retains the generated
confirmation, its own draft and the provider's executable
and metadata objects. The producer acquires the store reservation, starts the
verified agent and loader and constructs the native session on the process
owner's thread. The experimental writer observes the ready PTY and the stopped
loader through two typed startup events. Handoff consumes the confirmation, and
only successful handoff permits construction of the native session. A private
weak reference binds the creation attempt to that session's exact gate without
retaining another package lease. The producer issues a challenge from its retained
record, verifies the signature and drains that gate while servicing the native
session on its owning thread. It checks released state, the work snapshot and
observed exits before recovering the exclusive draft. Only verified possession
and closure can construct `ClosedCreationDraft`.
Publication consumes that value and keeps package, binding, record, provider and
final joint validation inside the core. The provider remains retained through
the store reservation if native closure is uncertain. JSON, case selection and
checkpoint controls remain in the experimental adapter; none constructs producer
evidence.

The private foreground operation runs this sequence in one process. It checks
the account's opening collection before secret delivery, creates and owns the
temporary agent directory, and closes the session before publishing
the initial opening permit and record, and returns the validated publication
result. If process closure is uncertain, it retains the runtime directory.

The private publication operation returns a non-cloneable `PublicationResult`.
A successful result retains the original published package, verified creation
binding and provider, together with an independently opened resident snapshot
whose record must equal this attempt's original record. Access to the result's
record and durable outcome revalidates those objects together. Failure after the
commit preserves the published objects and reports unknown record durability;
it cannot return a durable ACK or silently adopt a refreshed snapshot.

Only the closed producer constructs that successful result. There is no
constructor from a record, JSON, descriptor, outcome flag or reader candidate.
This in-process result does not make a fresh candidate reader into producer
evidence. A later reader still cannot infer historical ACK delivery or rebuild
the private state that authorized publication.

The [private provider selector](../tools/native-key-store-probe/review/PROVIDER.md#internal-producer-selection)
accepts only the explicitly observed Ubuntu build; caller-supplied agent and
loader descriptors are refused. The generic
candidate metadata store remains a mechanism, not a producer-authority boundary.

## Operational record

The operational record is `state/admissions/<credential_id>/record.json`.
It is strict canonical JSON, at most 2048 bytes, with exactly these fields in
the listed serialization order:

| Field | Required value |
|---|---|
| `version` | 2 |
| `installation_id` | Current independently selected administrator binding ID |
| `creation_id` | Fresh producer-generated canonical identifier |
| `device_id` | Intended canonical device identifier |
| `credential_id` | Canonical identifier equal to the resident slot name |
| `public_key` | 32-byte non-weak Ed25519 public key, 64 lowercase hex characters |
| `ciphertext_sha256` | SHA-256 of exact `key.openssh` bytes, 64 lowercase hex characters |
| `manifest_sha256` | SHA-256 of exact `manifest.json` bytes, 64 lowercase hex characters |
| `ciphertext_len` | Integer, 1 through 65536 bytes |
| `manifest_len` | Integer, 1 through 2048 bytes |
| `creation_profile_revision` | 1 |
| `secret_policy_revision` | 1 |

The two package files total at most 67584 bytes. These bounds follow the selected
[OpenSSH package profile](LINUX-OPENSSH-PROFILE.md), rather than the experimental
storage envelope's 8 KiB per-file limit. Unknown or duplicate fields, whitespace
or encodings outside the canonical serialization, unsupported revisions,
invalid IDs/keys/digests and out-of-range lengths refuse. The record contains
no secret, personal identity, file content, PID, device/inode, approval boolean,
unlock status, organizational authority or claim that an ACK was received.

Version 1 remains experimental metadata and is never interpreted as operational
admission. There is no automatic migration, import or `admit_existing` operation.
Changing a version field cannot establish producer history. Version separation
prevents accidental reuse; only the trusted producer can establish the procedure
within this profile's trusted-user boundary.

### Candidate codec boundary

The Linux [record codec](../src/device_keys/linux_creation_record.rs)
implements this version 2 envelope independently of experimental metadata.
It bounds input before JSON parsing, validates the fields and requires exact
canonical bytes. Its tests cover valid package-size boundaries, rejection across
the two versions, malformed/noncanonical data and unsupported values.

The codec processes public data only. It has no filesystem access, publication,
producer evidence, admitted-credential constructor or CLI/MCP operation. Parsing
a record establishes neither its location nor its origin. The composed reader
and producer must establish those independently before using any public key as
a trust anchor.

### Candidate resident reader boundary

The [resident snapshot fixture](../tools/native-key-store-probe/review/RUST-RESIDENT-READER.md)
uses a read-only candidate that composes the fixed root, version 2 record and
resident encrypted package. It retains all three, checks their agreement and
revalidates current named objects against retained descriptors and bytes.
Its shared OpenSSH byte validator is also used by the experimental writer.
The Linux binary compiles the same reader and codec. The creation command
reopens its publication, and the private endpoint verifier selects its own
retained admission through the boundary below.

The returned record is candidate public data, not an admitted-credential value.
Synthetic records in these tests establish neither trusted attended creation
nor its durable transaction. Producer evidence, exact initial native validation,
complete failure handling and operational adoption remain prerequisites.

### Retained current-expectation boundary

The private
[`CurrentAdmission`](../src/device_keys/linux_current_admission.rs) component
wraps that resident snapshot in a non-cloneable read value. Its only constructor
selects the fixed namespace from a canonical credential ID. It accepts no record,
descriptor, package path or preconstructed candidate as authority. Access to the
canonical public record, expected public key and record comparison validates the
same retained snapshot before and after the operation; it exposes no unchecked
record reference.

A failed validation permanently invalidates this attempt. The first native
error is preserved, and later accesses refuse without reopening the namespace
or rerunning validation. Comparing a valid snapshot with different expected
public data returns a mismatch; that comparison cannot construct admission or
change the selected identity.

The producer's retained result and the independent reader both use this
boundary. The reader repeats validation and checked access on the same value
before releasing it. An invalidated snapshot remains rejected, while an unchanged
snapshot still yields the original expectation. This read value establishes
neither the private procedure that authorized publication nor historical ACK
delivery. Installation authority, activation and endpoint authentication
remain separate requirements.

## Publication order

### Candidate package preparation boundary

The shared [package storage](../src/device_keys/linux_package_storage.rs)
separates private preparation from publication. The Linux command and disposable
writer compile that implementation. The integration test uses the same sources
and the synthetic resident fixture distributed under `tests/data`.
Its non-cloneable prepared value
retains the staging directory, both file handles, snapshots and exact encrypted
bytes. Preparation does not rename the package into its final slot. Publication
consumes the value and revalidates the retained objects, current names, private
permissions, exact membership and original bytes before its non-replacing rename.
Changing both files into a different internally consistent package still refuses.

Ordinary preparation accepts the location, comment and encrypted key. The
experimental adapter can observe two typed checkpoints after synchronization
of the key file and manifest file. It maps those checkpoints to its existing
fault selections. Both entries use the same preparation checks and return the
same retained value.

The disposable writer's first-opening extension coordinates the lease and
reservation through a storage view borrowed from the published draft. The
view uses the retained package descriptors and the verified openings
directory. Permit framing and session reservation live in the extension.

After the rename, synchronization and structural checks are followed by a final
named-object and byte revalidation. Failure there is a visible package with
indeterminate durability, never an unpublished operation or a successful ACK.
Cleanup preserves changed preparations and foreign objects; it never rolls back
a published package. The existing experimental `create` entry point composes
preparation and publication immediately and retains its existing CLI behavior.

Publication observers receive a checkpoint before renaming, one after the
rename commits the package name, and one after synchronizing the parent.
The disposable adapter owns the fault selector and translates it into those
checkpoints. The publisher classifies errors by the commit boundary and uses
the same sequence in the ordinary and observed paths.

This value supplies a filesystem mechanism only. It accepts encrypted input and
an experimental location; it establishes neither the trusted installation root,
attended secret generation, native binding nor observed native closure. The
trusted producer must supply those prerequisites before calling publication.
No admission record or operational custody follows from preparing a package.

### Fixed-root creation draft

The [rooted preparation](../src/device_keys/linux_package_storage/rooted.rs)
selects the fixed namespace internally and obtains `packages` from the retained
installation root. It accepts a canonical credential selector, not a root path,
installation identifier or caller-supplied directory descriptor. It retains the
root and revalidates its binding, named chain, exact package parent and absence
of an admission for that credential. Relative descendant opens require
`openat2` with `RESOLVE_NO_XDEV`, symlink and magic-link refusal; retained and
fresh objects must also match the selected device and mount ID.

Consuming that prepared package constructs a private in-memory draft. The
installation identifier comes from the retained binding, credential and device
identifiers and public key from the exact manifest, and digests and lengths from
the original retained package bytes. A fresh 16-byte OS-random draw supplies all
128 bits of the canonical creation identifier. No record, key, digest or creation
identifier is imported to construct the draft. Its record is exposed by immutable
reference, and validation recomputes its agreement with the retained objects.
The draft has no deserialization operation and confers no admission authority.
Its internal package-publication operation consumes the draft and retains the
original objects and record in a published-package result. The experimental
composition invokes that mechanism through a private wrapper constructed only
after binding verification and acknowledged native closure. It cannot publish
an operational record merely by serializing a draft.

The disposable `--fixed-root-record-draft` writer experiment composes attended
confirmation with this draft, allows independent observation, then revalidates
and drops the preparation. It writes no operational record or final package.
Its public observation remains data: it is neither producer evidence nor an
admitted credential. Native binding, observed native closure and ordered durable
publication remain separate requirements of the complete producer.

The separate `--fixed-root-native-bind` experiment checks those native steps
before its publication cases proceed. It distinguishes a durable experimental
publication from a package-only result and visible publication with unknown
durability. These observations do not enable product custody or establish
producer authority. The storage mechanism uses separate sealed codecs for
experimental version 1 metadata and version 2 records; neither reader accepts
the other format. Both use mandatory constrained `openat2` resolution and
retained mount identities, without a weaker fallback.

### Composed producer ordering

The producer retains one validated root and private preparation objects for the
entire attempt. Both destinations are exact credential IDs beneath that root:
`state/packages/<credential_id>/` and `state/admissions/<credential_id>/`.
It does not reconstruct either destination from a caller path.

1. Complete attended generated-secret delivery and exact confirmation under the
   [secret policy](LINUX-OPENSSH-SECRET-POLICY.md), including its foreground
   terminal, consent, deadlines and cancellation checks. Complete bounded
   in-memory encoding before creating any package file, as the package profile
   requires. Prepare a new private encrypted package under the retained packages directory.
   Apply the selected package ownership, mode, ACL, filesystem, exact-membership,
   manifest and cryptographic-header checks. Neither final slot is adopted.
   Form and retain the complete canonical version 2 draft from those objects
   before issuing the initial native binding challenge.
2. Complete the remaining creation/profile checks, native identity and possession
   validation and observed native closure required above. Retain the exact
   prepared package and internal evidence.
3. Revalidate the installation and prepared package. Synchronize the two files
   and complete package staging directory, then publish the whole package with
   `renameat2(RENAME_NOREPLACE)`. Revalidate retained and final named objects,
   synchronize the packages parent and validate again. No package-durability
   uncertainty permits admission publication.
4. Compare the published resident package with the same internal evidence and
   revalidate the retained canonical version 2 draft. Use that same record for
   the operational file; do not generate a new creation identifier or replace
   any bound field after native verification. No identity or digest comes from
   an endpoint response. Create a new
   private admission stage containing exactly `record.json`, mode 0600 and one
   hard link, inside a mode 0700 directory owned by the selected UID/GID.
5. Synchronize the complete record and admission stage. Revalidate installation,
   evidence, package and staging objects before publishing the admission directory
   with `renameat2(RENAME_NOREPLACE)` beneath the retained admissions directory.
   This rename is the logical admission commit and visibility boundary.
6. Revalidate root, package and published admission against retained objects,
   synchronize the admissions parent, then validate the joint snapshot again.
   Only success through this step permits a durable creation-admission ACK.

No step has an overwrite, ordinary-rename, copy/delete or weaker-resolution
fallback. A second producer that loses either final slot refuses; it cannot
adopt the winner, replace its bytes or report the winner's work as its own.
There is no cross-directory atomic rename joining package and admission.
Ordering ensures every record the producer exposes refers to a package whose
publication was already checked and synchronized after native closure.

## Visibility, durability and failures

| Cut or error | State and required response |
|---|---|
| Before package rename | No final package or admission from this attempt; private staging may remain |
| Native validation, delivery or exit uncertain | No admission commit or ACK; do not bypass, adopt or retry native work implicitly |
| Package destination occupied | Refuse without changing the existing entry; no admission from this attempt |
| After package rename, before its durability checks finish | Package may be visible with unknown durability; no admission publication or ACK |
| Package durable, before admission rename | Resident package is an orphan for activation; private admission staging may remain, with no final admission and no ACK |
| Admission destination occupied | Refuse, preserve both existing entry and published package; do not claim ownership of the existing commit |
| After admission rename, before final checks/sync finish | Logical commit may be visible; durability is unknown and no durable ACK is returned |
| All final checks and synchronization succeed | Durable ACK may be returned for this attempt |
| Reply is lost or producer dies after admission rename | A complete commit may remain; a reader cannot infer that an ACK was sent or received |

Publication errors distinguish unpublished admission from visible admission with
indeterminate durability. A package-only result is never admission. Failures
preserve already published objects: no destructive rollback after rename.
Uncommitted staging is not discovered, repaired, resumed or adopted by a reader.
Retrying the same occupied credential refuses; a new attended creation does not
silently reclaim or erase an old slot. Cleanup and administrative resolution
need a separately specified explicit procedure.

A fresh reader may accept a complete visible operational commit after an
unacknowledged publication, provided its entire joint snapshot passes. Every
producer prerequisite precedes the commit rename. The reader establishes
current local admission, not historical durability or ACK delivery. A later
power failure may lose an unacknowledged commit; that is not proof it was never
visible. No marker can turn ACK receipt into a fact deducible from these bytes.
This does not promote the experimental storage component's visible metadata
into admission or authorize automatic repair of an uncertain transaction.

## Joint non-writing reader

The input is only a canonical credential identifier. The reader internally
selects the fixed root, retains it and reads the version 2 record from that
root's admissions slot. It checks the independent installation binding before
opening the package resident under the same credential ID. A package path,
record, endpoint or environment variable cannot select another installation.

The reader must retain and repeatedly validate all three snapshots together:

- The fixed root and its current named ancestors, binding bytes and child objects.
- The admission directory and exact record, with ownership/mode/ACL/link/shape
  checks and supported canonical format/revisions.
- The named resident package directory and both files, with the complete selected
  package profile, manifest identity, encrypted-header public key, exact lengths
  and SHA-256 digests matching the independently read record.

NO_XDEV, symlink/magic-link refusal and the selected filesystem profile extend
from the custody anchor through credential slots and file leaves. A same-device
bind mount is still a mount crossing. A descriptor handed out by the root reader
does not alone establish this policy for its descendants. Returning to an
unrelated package path or comparing only device/inode is insufficient.

Binding installation ID, record installation ID, slot and record credential ID,
manifest device/credential/public key and package/header public key must agree.
Bounded parsing and revalidation occur before exposing a usable expectation;
the root, record and package remain held throughout any activation attempt.
Relevant in-place mutation, replacement, permission/ACL change, mount change or
missing object invalidates the attempt without silently refreshing its identity.
Publishing an unrelated credential must not invalidate an unchanged snapshot.

Reads do not create, chmod, delete, synchronize a failed publication, repair,
scan for candidates or write an admission receipt. Missing, experimental,
unknown, partial, misplaced or mismatched state is unavailable for custody.
Ordinary local capture remains independent of this result.

Current local admission supplies an expected public credential only. Fresh
foreground unlock, reservation, endpoint proof, native-session exit/replacement
and organizational authorization remain separate gates. Restoration and copying
retain the [root contract's limits](LINUX-CUSTODY-ROOT.md#copies-resets-and-restoration);
the record does not provide anti-rollback or organizational revocation.

The disposable producer experiment reopens its result with this same candidate
reader in a new process after observing the producer's exit. Only the credential
identifier selects the resident objects; the observer compares the independently
read canonical record with the producer's original record. Barriers separate
opening, retained-snapshot revalidation and release, allowing controlled
in-place changes and identical-byte replacements between reads. Filesystem
comparisons check contents and selected persistent metadata around reader
execution, excluding access-time updates caused by reads; they are not a
system-call trace. Neither the reader result nor those comparisons infer a
historical durable ACK or establish trusted producer authority.

Multi-producer experiments retain explicitly selected prior entries and verify
their named objects, held descriptors, metadata and bytes without refreshing
their identities. The empty-namespace experiment remains strict by default;
preservation does not permit arbitrary extra entries. One variant completes a
second attended producer and a fresh read of its result while the first reader
keeps its original snapshot open. The first snapshot must remain valid.

A controlled same-credential collision instead pauses one producer after native
closure, lets a second producer publish, then resumes the first. The losing
attempt must report its own unpublished result without adopting the winning
record or ACK. The winner remains independently readable and unchanged; the
loser's private preparation may remain for inspection. This ordering tests
overlapping attempts and destination occupancy, not simultaneous rename calls.

Owned-death variants pause at package rename/synchronization checkpoints, record
file/directory/rename checkpoints, and after joint validation but before the
durable reply. The observer verifies the original objects and acknowledged native
child exits before sending `SIGKILL` through the retained producer pidfd. It
requires the exact signalled exit, channel EOF and no normal final reply; it does
not turn an I/O error or timeout into a death receipt. A new reader then checks
the resulting namespace without repair. Private stages and package-only states
remain unavailable; a complete visible record can be read without inferring an
ACK. These cuts demonstrate owned process death, not a physical power failure or
which destructors ran before the selected checkpoint.

Live-mount variants cover the retained preparation directory or either leaf
before publication, or the package directory, either leaf or record while a new
reader holds its snapshot. An isolated administrative auxiliary binds the same
object over itself, makes only that new mount private and verifies its mount ID
and propagation state. The native actor retains no capabilities. The observer
independently requires a different named mount ID with the original device,
inode and content unchanged, while its retained descriptor keeps the original
mount ID. The preparation must remain unpublished; an already opened reader
must reject revalidation with a cross-device error. Only after observing the
native result and process exit does the auxiliary unmount the selected object,
and the observer verifies restoration. Persistent-state comparisons alone do
not establish a mount change. These cases neither infer a new durable ACK nor
confer creation authority or enable Linux custody.

## Preparing retained ciphertext for a later activation

An experimental preparation may consume a `CurrentAdmission` to derive native
material internally. It accepts no caller-selected descriptor, path, record or
ciphertext. The preparation duplicates the retained package directory and copies
the retained encrypted key into a bounded anonymous file sealed against writes,
growth, shrinkage and further seal changes. Its package lease must match the
admission's retained physical objects. Admission, named objects and descriptors
remain subject to validation before and after preparation and subsequent use;
the first validation failure permanently invalidates that preparation.

The opaque preparation may acquire an independent, nonblocking store reservation.
The reserved value retains the same admission and validates both that admission
and the reservation against its lease. It exposes no native descriptors, secret
input, process launch or session activation. Dropping it releases the reservation.

A free reservation does not prove that an earlier key-capable process exited.
Reopening still requires independently established prior closure, a fresh
attended TTY unlock and fresh native session evidence. Preparing encrypted
material grants none of those authorities and does not enable Linux custody.

Post-preparation experiments must retain an unreserved preparation and a reserved
preparation from independent admissions before the observer changes any object.
Each must detect that change through its own validation, before validating the
original reader. After the observer restores the original named object, a new
admission may validate the current namespace; it must not refresh either failed
preparation. Evidence must distinguish the original, replacement, retained and
restored objects and their encrypted contents. Renaming an original object back
does not establish that all of its metadata returned to its previous values.

## Acceptance before implementation is offered

Implementation and native accreditation must cover the composed producer/reader,
not substitute three individually passing fixtures. Required cases include:

- Real attended producer evidence and observed exits; refusal of imported flags,
  metadata version 1, weak keys, unknown revisions and caller-selected roots/paths.
- Resident roundtrip and cross-installation/cross-credential, manifest/key/digest/
  length mismatch, copied package, missing record and missing package refusal.
- Actual ACLs, symlinks, extra members, multiple hard links and additional mounts
  at record/package directories and leaves, including same-device bind mounts.
- Retained joint-snapshot invalidation for root/binding, record, package directory
  and file mutations or identical-byte replacements; unrelated publication passes.
- Injected failures at each file/directory sync, both renames and final validation;
  distinguish injected I/O faults from owned process death and physical power loss.
- Death before/after each rename, concurrent producers, occupied destinations,
  orphan package, visible commit with unknown durability and lost replies, with
  no false durable ACK, replacement, automatic adoption or repair.

Tests retain source/artifact identity and independent process/exit observations,
use disposable installations and preserve the existing resource and RDP guards.
The local creation command publishes admission. Session activation and
the [private endpoint proof](LINUX-ENDPOINT-PROOF.md) have their own acceptance
criteria. Shared operations remain governed by the
[shared authority contract](TEAM-AUTHORITY.md).
