# Verifiable additional-device admission experiment

The isolated coordinator's format-5 experiment admits an additional signing
binding, updates its approving passkey state and consumes its authorization in
one signed-history transaction. Replay verifies the retained cryptographic
evidence and derives those effects again after restart. It uses explicit public
test trust; it does not provide organizational bootstrap, a production account
service or evidence that a real person used a trusted authenticator.

The separate [initial-trust contract](TEAM-BOOTSTRAP.md) defines acceptance and
rejection requirements for replacing fixture trust. It does not change this
profile's bootstrap or authorize importing a registration as fixture state.

## Bootstrap and state

Fixture trust supplies the registered-authority bootstrap, exact relying-party
identity and HTTPS origin, and at most 128 passkey associations. Each association
names one existing signing device and credential, with the same organization and
principal as its public passkey state. Association identifiers and WebAuthn
credential identifiers are unique, and the initial principal has an association.
There is no association imported from an enrollment request.

The public state contains the credential identifier, COSE public key, algorithm,
counter, backup-eligibility and backup-state flags, and logical generation. It
contains no authenticator secret. The fixture constructor accepts algorithms
`-8` and `-7` with bounded credential and key fields. It refuses nondefault
authenticator model, attestation, transports, resident-key and extension metadata
rather than retaining fields outside the commitment. This bounded descriptor is
not a registration or attestation verifier; malformed public key material still
refuses when the maintained assertion verifier attempts to use it.

The state commitment is SHA-256 over ASCII `vivac.passkey.public-state.v1`, one
zero byte, organization and principal identifiers (26 canonical bytes each),
credential and key bytes each preceded by their unsigned 16-bit big-endian
length, algorithm as a signed 64-bit big-endian integer, counter as an unsigned
32-bit big-endian integer, BE and BS as one byte each (`0` or `1`), and generation
as an unsigned 64-bit big-endian integer.

The verifier configuration commitment uses ASCII
`vivac.passkey.configuration.v1`, one zero byte, and the canonical relying-party
and origin bytes, each preceded by their unsigned 16-bit big-endian length.
The format-5 bootstrap digest uses ASCII
`vivac.team.authority.enrollment-fixture.v1`, one zero byte, the 32-byte registered
fixture bootstrap digest, the 32-byte configuration commitment, the association
count as an unsigned 16-bit big-endian integer, and sorted associations. Each
association contributes device and signing-credential identifiers (26 bytes
each) and its 32-byte public-state commitment.

The database cannot replace these trust inputs with its own keys. A retained
checkpoint must match the reconstructed history. Restoring the database and all
external expectations can still defeat rollback comparison; no global freshness
or malicious-coordinator consensus is claimed.

## Live admission and historical verification

Issuance and beginning retain private process-local tokens tied to the physical
store, full checkpoint and exact candidate. Beginning resolves the prior signer
and its passkey association from the replayed state and starts the existing
additional-device assertion scope. Client-supplied keys or association claims
cannot establish that trust. Restart loses unfinished tokens.

Finishing takes a current `BEGIN IMMEDIATE` view. It checks the token, checkpoint,
active owner, explicit `OwnAdditionalDevice` grant, prior signing credential,
candidate possession, exact endorsement and passkey state again. The maintained
verifier checks the additional assertion, required user verification, origin,
relying-party identity, signature, counter and backup flags. Time is checked in
the live flow, including before commit. The accepted time is selected by that
flow, not by an imported record.
Issuance also fixes a private monotonic deadline. Beginning or finishing cannot
renew it, and both phases and the pre-commit check refuse after it expires even
if the wall clock moved backwards. That deadline is never serialized or restored
as a live capability.

The admission request wrapper has a private live capability bound to its exact
canonical bytes and checkpoint. Decoding a historical record never creates that
capability. Even an otherwise valid signed witness cannot create a new live
admission by being passed directly to the generic append API. An exact existing
receipt may be reconciled only while its original principal and prior signing
credential remain valid; it does not consume evidence again or renew permission.

Replay reconstructs assertion state through Passki's maintained API and checks
the same scoped commitments at the retained accepted time. It does not import
`PendingAssertion`, trust a saved "approved" flag or require an old assertion to
remain unexpired at the time of a later restart. Each passkey's counter, backup
state and generation come from the verified prefix before the next assertion.
The original public-state object is never updated by witness verification.
The retained accepted time is the coordinator's admission-time claim, checked
against the signed windows. It is not a cryptographic attestation of the physical
commit time or a trustworthy clock. The online coordinator remains trusted to
enforce issuance, deadlines and ordering; replay alone does not establish those
facts against a malicious coordinator.

Only after all checks succeed does the transition add the new Ed25519 binding,
advance the shared revision, record the updated approving passkey state and
retain consumption of both candidate and ceremony nonces. These effects derive
from one admitted record; no editable counter or consumption table sits outside
the checkpoint. The new binding belongs to the same principal and grants no new
roles. It does not automatically register a passkey on the new device.

## Exact record encoding

| Field | Representation |
|---|---|
| Domain | ASCII `vivac.device.additional-admission.v1`, then one zero byte |
| Candidate | Exact 251 canonical possession-challenge bytes |
| Possession signature | 64 bytes |
| Prior signing device | 26 canonical identifier bytes |
| Prior signing credential | 26 canonical identifier bytes |
| Prior-client endorsement | 64 bytes |
| Assertion context nonce | 32 bytes |
| Assertion issued time | Unsigned 64-bit big-endian integer |
| Assertion expiry | Unsigned 64-bit big-endian integer |
| Accepted time | Unsigned 64-bit big-endian integer |
| Assertion length | Unsigned 16-bit big-endian integer |
| Assertion | Complete raw response, from 1 to 16,384 bytes |

The fixed header is 526 bytes, so a record is between 527 and 16,910 bytes.
Unknown domains, malformed identifiers, bad lengths, trailing bytes and
noncanonical re-encoding refuse. The `entries.signature` column repeats the
endorsement and must match the record. Existing policy and revocation requests
retain their signing bytes and signatures. Each history step remains SHA-256 of
the previous digest, complete canonical request and 64-byte signature column.

## Durability and bounds

Policy, revocation and enrollment share one revision and one namespace of at
most 1,024 operations. The profile permits at most 64 complete enrollment
records and 128 total signing bindings, including revoked bindings. Neither
reaching a limit nor revocation evicts evidence, releases identifiers or resets
a consumed nonce. Counter-zero authenticators remain supported: durable nonce
and operation consumption, rather than a fabricated counter requirement, prevent
a second admission from the same authorization.

The profile creates a fresh private root with metadata version 5. It retains the
existing filesystem, locking, SQLite heap, 2 MiB database and 4 MiB journal limits.
Before-commit failures leave all four effects absent. An uncertain commit
poisons the handle; reopening and replaying retained history reconciles the
result before another action. Historical reconciliation does not run a second
approval or update the counter a second time.
The reconciliation query accepts the original possession proof and prior-client
endorsement, which the caller could retain before finishing. It matches them
exactly to an admitted receipt and rechecks the original principal and signing
credential's current validity. It returns only the original historical outcome;
the caller needs neither SQL access, the full assertion archive nor the accepted
timestamp chosen inside the coordinator. An unknown, changed or revoked request
refuses. This is the existing authorization's idempotent retry, not a new session
or an authentication transport.

Formats 2, 3 and 4 retain their closed schemas and signing contracts. There is no
automatic migration, decoder fallback or conversion of their pending caches or
in-memory checking receipts into admission evidence.

## Build and production boundary

The `coordinator-probe` feature is enabled by default only in the isolated
`team-coordinator-probe` crate. It loads the admission backend there; the probe
still exposes no account or network commands. Disabling its default features
keeps the earlier profiles available. The product recognizes that feature name
only for checking the shared source's configuration; it neither declares nor
activates the feature, and gains no Passki dependency through this experiment.

Production still needs accepted organizational bootstrap and passkey
associations, trusted client distribution and candidate transfer, the real human
verification mechanism, and operational review. Passing public signing fixtures
does not establish those facts. Content keys remain with authorized clients or
designated recovery custodians, not the coordinator. Artifact synchronization
and encrypted server retention remain separate work.
