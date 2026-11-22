# Candidate coordinator authority storage

This specifies the isolated persistence experiment for the
[signed authority model](TEAM-IDENTITY-PROTOCOL.md). It is not an account system,
an organization bootstrap procedure or a deployable coordinator. The experiment
uses public signing fixtures and does not receive participant signing seeds or
content decryption keys.

## Trust and history

The caller supplies the organization, initial principal and known public device
bindings independently of the database. Database contents cannot appoint their
own verification key. These inputs are fixtures, not evidence of enrollment or
human approval.

Stored entries contain the exact canonical request for the selected profile and
its signature. Opening a history must decode every entry, verify its signature
and replay the transitions from the fixture bootstrap. The legacy profiles use
external signing bindings; the registered profile resolves their current
validity from the registry at each step in the verified history.
The operation limit applies to the complete history; reaching it cannot discard
earlier evidence. An exact retry returns historical evidence only while its
requester remains authorized. Revocation is checked before receipt lookup.

A checkpoint binds a revision to a digest of the bootstrap trust and admitted
history. The caller supplies its previously trusted checkpoint when reopening.
The stored history must contain that exact prefix. Each live handle retains its
highest verified checkpoint and checks it again before using newer history.

This detects a database restored behind an independently retained checkpoint or
a conflicting history at that checkpoint. Restoring both the database and every
external expectation can defeat that comparison. A new installation still needs
authenticated bootstrap and a current trusted checkpoint. This experiment does
not establish global freshness or resistance to a malicious coordinator.

## Fixed storage profiles

The isolated engine has sealed, statically selected profiles. The existing
`AuthorityStore` alias retains storage format 2, version-one signed requests and
the pending-evidence table. `AuthorityStoreV2` uses a new private root with storage
format 3, version-two signed authority history and no pending table or enrollment
ceremony methods. The caller selects a typed profile; database metadata does not
select a decoder or authorize a fallback. `AuthorityStoreRegistered` uses a
separate new private root with storage format 4 for version-two policy changes
and individual own-credential revocations.

Format 3 contains exactly `meta`, `entries` and the unique-operation index.
It retains the metadata columns and entry bounds, but its exact metadata DDL
requires version 3. Format 2 retains its existing DDL and indexes. Each profile
refuses the other's database, and creation refuses an existing destination.
There is no conversion, schema repair, imported history or automatic migration.
The existing engine's SQLite rollback-journal recovery still applies when opening
a database; it is distinct from converting a signed history or storage profile.

All profiles use the same filesystem validation, procfd VFS, locks, connection
configuration and transaction implementation. Database, journal, SQLite heap
and operation limits remain unchanged. Opening and reading replay the complete
signed history and compare its prefix with the independently retained checkpoint.
Read-only policy queries do not establish global freshness.
The root identity is the retained directory descriptor's capability, not its
original pathname. Renaming that directory while retaining the same descriptor
does not substitute the selected root.

The version-two fixture trust wraps the existing organization, initial principal
and known signing bindings, and names its initial enrollment-policy administrator.
That administrator must equal the initial principal. The bootstrap digest is
SHA-256 over ASCII `vivac.team.authority.fixture.v2`, one zero byte, the 32-byte
version-one fixture bootstrap digest and the administrator's 26 canonical bytes.
The base fixture's size, organization and binding checks still apply. This binds
the explicit role into the external test trust; it does not authenticate a real
organization's bootstrap.

Each history step retains the existing SHA-256 encoding: previous 32-byte digest,
complete canonical request and 64-byte signature. Bootstrap and message domains
separate the profiles. Format 3 replays the
[version-two policy model](TEAM-ENROLLMENT-POLICY.md), including its single order,
explicit grants, delegation limits and revocation. Its permission queries read
that verified snapshot, rather than an external grant catalogue.

Successful commit, rollback, uncertain commit and poisoned-handle behavior are
shared. A policy-model receipt remains distinct from human approval or device
admission. Format 3 does not register or individually revoke device credentials,
consume enrollment evidence or expose a production service. Its known signing
bindings are still external fixtures. Format 2's existing pending and assertion
checks are not moved to the new profile.

### Registered history in format 4

Format 4 also contains exactly `meta`, `entries` and the unique-operation index,
with metadata version 4. Its exact entry DDL accepts policy requests of 186 to
213 bytes or revocation requests of exactly 226 bytes. The sealed profile fixes
both the DDL and the maximum replay input size. The decoder dispatches those
disjoint lengths to their strict canonical codecs; it never retries another
codec after a refusal. Formats 2 and 3 retain their original 186-to-213-byte
entry bounds. All three profiles refuse each other's databases without migration
or reinterpretation.

The registered fixture trust names the organization, initial principal, initial
policy administrator and complete public fixture catalogue. It validates the
[registered-authority bootstrap](TEAM-CREDENTIAL-REVOCATION.md#registry-boundary),
including the initial binding and uniqueness of device identifiers, credential
identifiers and keys, before accepting that trust. Its bootstrap digest is SHA-256
over ASCII `vivac.team.authority.registered-fixture.v1`, one zero byte, the 32-byte
version-one fixture bootstrap digest and the administrator's 26 canonical bytes.
This separates the registered history from the standalone policy profiles even
when their external fixture inputs happen to match.

Replay rebuilds the complete registered authority in order. An old policy row
signed before its credential was revoked remains verifiable at that historical
step. A new request or retry is checked against the final replayed registry:
revoked credentials cannot retrieve old policy or revocation receipts through
the admission entry point. The same shared revision and 1,024-operation limit
cover both request types. Revoked identifiers and keys remain reserved; there
is no editable revocation table outside the signed checkpoint.

Credential-validity and policy queries use a fresh verified snapshot. The
stateless fixture-key lookup inherited by the profile trait always refuses for
this profile: fixture trust alone cannot establish current credential validity.
The registered transition path resolves the key inside the replayed authority.
Rollback, commit uncertainty, poisoned handles and retained-checkpoint checks use
the same engine as the legacy profiles. Restoring every external checkpoint as
well as the database still defeats rollback comparison.

Format 4 persists fixture history only. It has no pending-enrollment table,
passkey-counter state, new-binding operation, human-verification mechanism or
operational endpoint. It does not migrate the format-2 ceremony or turn fixture
bootstrap into organizational trust. Enrollment still requires accepted trust
sources and atomic binding, approval-state and nonce-consumption updates.

The separate [registered assertion checks](REGISTERED-DEVICE-ASSERTIONS.md) use
that verified registry and policy in checking transactions. Their issuance and
assertion tokens stay in memory, and their confirmed counter update is not a
durable state transition. They do not add rows or tables to format 4.

### Additional-device admission in format 5

The separately gated [atomic admission experiment](ATOMIC-DEVICE-ADMISSION.md)
uses `AuthorityStoreEnrollment` and a fresh format-5 root. It retains complete
possession, endorsement and assertion evidence, derives the approving passkey's
state and consumed nonces from that history, and admits the new signing binding
in the same transaction. It does not alter the format-4 checking API or import
its ephemeral results. Its bootstrap includes public passkey associations and
verifier configuration as external fixture trust. It is not a production account
system or an authenticated organization bootstrap.

## Pending enrollment evidence in format 2

The candidate store's format two adds a bounded pending-evidence table in the
same database. Format one is refused; this experiment provides no migration.
The signed authority message remains version one.

A caller supplies an opaque request identifier and proposed principal, device,
credential, public key and lifetime. The coordinator fixes the organization and
current authority revision, samples an independent operation identifier and
32-byte nonce from the operating-system CSPRNG, and records the complete
[enrollment challenge](DEVICE-ENROLLMENT-PROOFS.md). Entropy or clock failure
refuses issuance. No participant signing seed is stored. The lifetime is at most
300 seconds; the operating-system clock is a trusted input.

Reusing a request identifier requires the same proposal and lifetime, an unexpired
challenge and the same current authority revision. It returns the original
challenge without renewing it. The store retains at most 128 pending records,
including expired records; reaching the limit refuses new requests. There is no
implicit eviction or renewal.

Proof observation requires the exact stored challenge, a current revision and a
strict signature within the challenge's time window. The product's proof
verifier is reused without modification, including its possession-only labels
and distinct signing domain. JSON input is bounded before parsing. A repeated
proof must still satisfy the current revision and time window; it returns its
original observation time. Reopening verifies canonical stored
records and signatures at their recorded observation times; it does not make an
expired challenge current again.

Issuance and observation use the same writer transaction, filesystem validation
and uncertain-commit rules as authority admission. They do not change authority,
consume a nonce or establish human approval, enrollment eligibility or a trusted
device binding. Looking up evidence is a historical observation only.

The authority checkpoint does **not** cover this pending table. Its validation
cannot detect every replacement or restoration of otherwise valid pending
evidence. This is not a single-use enrollment or pending-cache anti-rollback
guarantee. Future enrollment must revalidate the issued challenge, current time,
authority, binding and accredited approval, and consume the nonce atomically
with the admitted transition. Merely recording possession must not satisfy that
gate.

## Coordinated assertion checks

The isolated probe composes pending evidence and the
[assertion verifier](DEVICE-ENROLLMENT-ASSERTIONS.md) under a current authority
view. Both beginning and finishing use `BEGIN IMMEDIATE`, validate the opened
store and replay its signed history before checking the approver's principal.
The principal must be active in the same organization and currently hold
`ManageMembers`. This is a necessary condition of that member-administration
check, not sufficient
enrollment authorization or permission to delegate privileged capabilities.

The opaque in-memory token binds the physical store identities, full authority
checkpoint, pending operation and exact canonical possession proof. Finishing
requires all of them to remain unchanged. A different revision, revocation,
capability removal, replaced proof or different store refuses the old ceremony;
an earlier capability cannot authorize a historical retry.

Signature verification prepares a credential update while exclusively borrowing
the in-memory binding. It does not publish the counter, backup state, generation
or assertion evidence yet. Only a confirmed commit of the checking transaction
and final file validation allow publication. Refusal or an uncertain commit
discards the prepared update. A successful result records the checkpoint at
which the checks were made, not continuing permission after that checkpoint.

These transactions write no authority entry or pending record. They do not
consume an enrollment nonce, persist a passkey update, admit a device or prove
human approval. The fixture passkey still has no individually revocable trusted
credential registry. Future admission must perform those durable operations
atomically and recheck the full enrollment policy and fresh human verification.

## Additional-device policy checks

A separate experimental path checks the
[owner-consent policy](TEAM-AUTHORITY.md#additional-devices-and-owner-consent).
It requires an active principal, matching owner and approver, an existing known
signing binding for that owner, and explicit `OwnAdditionalDevice` permission.
It does not require `ManageMembers` from an ordinary owner, and an administrator
cannot use that capability to act for another owner.

Both steps also require a live endorsement from an owner's previous signing
binding. The caller supplies only that device identifier, credential identifier
and a detached 64-byte signature. The verification key is selected from
`FixtureTrust.known` for the exact organization, owner, device and credential.
The candidate's self-declared public key cannot substitute for it.

The permission catalogue is deliberately named `FixtureAdditionalDevicePolicy`.
It is immutable, contains at most 128 distinct opaque principal identifiers in
one organization, and has a canonical fingerprint. It is not deserialized from
a request and is not an admitted policy record. Its entries are external trust
fixtures, like the probe's signing bindings. Production must obtain both from
authenticated, current authority rather than infrastructure configuration or
an editable client field.

The additional-device token and result have distinct types from the generic
member-administration check. Beginning captures the fixture-policy fingerprint;
finishing requires it unchanged and repeats all current owner, permission,
known-client and candidate checks. A policy change invalidates the ceremony
even if the owner remains listed. The physical store, full checkpoint and exact
candidate proof stay bound by the existing transaction boundary.

The additional token retains the exact endorsement. Verification uses strict
Ed25519 over this fixed concatenation, without changing the other signing
domains:

| Field | Encoding |
|---|---|
| Domain | ASCII `vivac.device.additional-endorsement.v1`, then one zero byte. |
| Organization and owner | 26 canonical ASCII bytes each. |
| Previous device and credential | 26 canonical ASCII bytes each. |
| Authority checkpoint | Revision as unsigned 64-bit big-endian, then its 32-byte digest. |
| Fixture-policy fingerprint | 32 bytes. |
| Candidate | Complete 251-byte canonical enrollment challenge. |

The checkpoint revision must equal the candidate's base revision. Beginning and
finishing verify the same signature against the current context. A signature
for the possession-proof or authority-change domain is not an endorsement.
The probe exposes message construction for a future constrained client, not
arbitrary signing or a command that uses real keys.

The additional WebAuthn challenge also commits to the complete endorsement
message and signature through its own context and assertion domains, as specified
in [the assertion contract](DEVICE-ENROLLMENT-ASSERTIONS.md#additional-device-challenge).
The coordinator recomputes that context at both ends. Generic assertions keep
their existing encoding and cannot be substituted for this additional scope.

Reusing any known device identifier, credential identifier or signing public key
refuses, including a binding belonging to another principal. Without a known
owner client this path refuses; it does not infer first-device bootstrap or
recovery from a login or candidate signature. Publication still happens only
after the checking commit and final validation. No new binding, role, permission
record or nonce consumption is written by this experiment.

Endorsement verifies control of a registered fixture key, not physical custody,
human presence or an individually revocable production credential. Those trust
sources still require their own accredited implementations.

## Atomic admission

SQLite orders writers with `BEGIN IMMEDIATE`. Admission replays the current
history while holding the transaction, checks the expected predecessor and
stores the signed entry in the same transaction as the resulting revision.
The journal mode is `DELETE` and synchronous mode is `FULL`. A successful
acknowledgement requires a successful commit and final validation. A pre-commit
failure must not publish a transition. An uncertain commit must invalidate the
handle and require reopening and reconciliation, rather than return success or
blindly resubmit the request.

Hot rollback journals are recovery inputs, not trusted authority. Only bounded,
private regular journals are eligible for SQLite's recovery. Recovery is followed
by the same schema, signature, replay and checkpoint checks. Corruption does not
authorize recreation, repair or automatic restoration from backup. Tests that
terminate a process establish process-crash behavior; they do not simulate physical
power loss or prove a storage device honors synchronization requests.

## Linux filesystem boundary

The experiment accepts an already opened private directory descriptor on its
declared local Linux filesystem profile. It validates ownership, permissions,
file types, link counts and access-control metadata. The database name is fixed
as `authority.sqlite3`. Creation is exclusive; opening an existing store must
not create a missing database. Unexpected auxiliary files refuse.

Before the first SQL statement on an existing database, the physical header
must declare rollback-journal read and write versions. This prevents SQLite
from creating even temporary WAL or shared-memory files while inspecting an
unsupported journal profile. The fixed-size header read uses SQLite's own file
object through `SQLITE_FCNTL_FILE_POINTER` and its maintained `xRead` callback;
it does not open another database descriptor.

SQLite's maintained Unix VFS supplies file I/O, locking and journal recovery.
A narrow copy of that VFS replaces only `xFullPathname`: it preserves the exact
internally generated `/proc/self/fd/<descriptor>/authority.sqlite3` path instead
of resolving the descriptor anchor into a pathname. Caller-supplied SQL paths
are not accepted. `SQLITE_OPEN_NOFOLLOW` remains enabled for the leaf.

The root descriptor outlives the SQLite connection and its statements. Named
file checks use descriptor-relative metadata and SQLite's `HAS_MOVED` control.
They must not open and close a second descriptor for the database inode while
SQLite connections exist: on POSIX systems that can release another connection's
process-associated locks. The profile refuses changed database identity and
does not claim protection against a malicious process with the same Unix identity.

## Isolation and integration gates

The [coordinator probe](../tools/team-coordinator-probe/README.md) is a separate
crate and process. The product does not compile its durable module or use its
SQLite initialization policy. The product's artifact SQLite engine has its own
initialization and memory limits; these cannot be silently relaxed to integrate
authority persistence.

Before team deployment, the real client and server paths still need reviewed
bootstrap, enrollment, credential lifecycle, authenticated transport, durable
invitation consumption, administrative authorization and recovery. Privileged
ratification needs its separate fresh, non-delegable human-verification flow.
Content sharing additionally needs encryption and an explicit answer to who
holds its keys. A valid device signature or a durable fixture history does not
establish any of those properties.
