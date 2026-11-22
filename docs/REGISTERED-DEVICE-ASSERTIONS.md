# Registered additional-device assertion checks

The isolated format-4 coordinator can check an additional-device assertion
against its replayed registry and enrollment policy. This connects the assertion
verifier to current signed authority; it does not admit a new binding or make
the assertion state durable. The bootstrap, signing bindings and passkeys are
still explicit test trust.

## Issued candidate and current authority

Candidate issuance opens a checking transaction, replays the registered history
and requires the target principal's explicit `OwnAdditionalDevice` permission.
The device identifier, credential identifier and signing key must all be new
across the complete fixture catalogue, including revoked and inactive bindings.
The catalogue's 128-binding bound still applies. Membership administration or
enrollment-policy administration does not substitute for the owner's grant.

The coordinator supplies a fresh operation identifier and nonce from the
operating-system CSPRNG. The lifetime is between one and 300 seconds, and an
operation already present in the signed history refuses. Issuance returns the
public challenge and an opaque server-owned token bound to the selected physical
store identities, full checkpoint and exact 251 canonical candidate bytes.
An entropy, clock, transaction or file-validation failure refuses issuance.

Issuance checks the proposed owner and policy; it does not authenticate a caller
or a particular prior credential. It cannot grant enrollment authority. Beginning
the assertion must separately prove control of the mapped prior client. Knowing
an owner identifier and obtaining a challenge is insufficient.

The token is neither cloneable nor serializable, and a public challenge or proof
cannot reconstruct it. It is held in memory rather than the format-2 pending
cache. Restart loses it. This is not durable challenge issuance, replay
protection or nonce consumption; those remain admission requirements.

## Prior client and passkey association

An explicit fixture wrapper owns a trusted passkey binding and the opaque device
and credential identifiers of its prior signing client. The association is
private and immutable. Client input cannot select its own trusted passkey or
replace that association through this API. This constructor describes external
test trust, not authenticated organizational registration.

At both beginning and finishing, the registered authority must resolve that exact
prior client as currently valid for the passkey's organization and principal.
The prior-client endorsement must name the same device and credential, and its
verification key comes only from the replayed registry. The target principal
must be that same active owner. No administrator can approve another owner's
additional device through this path.

## Scope commitment

The applicable policy commitment is SHA-256 over ASCII
`vivac.device.registered-additional-policy.v1`, one zero byte, the organization's
26 canonical bytes, the owner's 26 canonical bytes, the checkpoint revision as
an unsigned 64-bit big-endian integer and its 32-byte digest. It describes the
checked registered context; it is not a caller-supplied grant catalogue.

The existing canonical prior-client endorsement binds that commitment, the full
checkpoint and the exact candidate. Its signature then participates in the
additional-assertion context digest. The WebAuthn challenge commits to that scope,
the expected passkey, relying-party identity, exact origin, fresh ceremony nonce
and time window. A generic assertion or a context from the legacy fixture-policy
flow cannot substitute for this registered context.

## Finishing the check

Beginning consumes the issued-candidate token and verifies the exact possession
proof, current time, physical store identities, checkpoint, grant, candidate
freshness, association and prior-client endorsement before starting WebAuthn.
The resulting pending token is also private, noncloneable and nonserializable.

Finishing consumes that token under a new `BEGIN IMMEDIATE` checking transaction.
It replays authority and repeats those checks before preparing the assertion
update. A changed checkpoint, revoked prior credential, lost grant, changed
candidate, different store, expired challenge or changed passkey instance or
generation refuses the old ceremony. Regranting a permission does not revive an
assertion made against an older checkpoint.

Verification exclusively borrows the passkey binding. Only confirmed commit and
final file validation permit publication of the prepared counter, backup state
and generation update in that object. Rollback and uncertain commit discard the
prepared update; an uncertain commit poisons the handle for reconciliation.
The result records the checkpoint of a successful check, not continuing
permission, human comprehension or an admitted device.

## Remaining admission boundary

The checking transactions append no authority entry, create no registered
binding and consume no durable nonce or approval. The counter update is in
memory only. Exclusive borrowing and generation checks protect one binding
instance; they do not establish globally coordinated counter state across
independently constructed fixture instances or restarts. Successful checking
does not survive as an enrollment receipt after restart.

Real admission must atomically retain the new binding, required passkey state
and single-use authorization under accepted bootstrap and current authority.
It must also specify durable reconciliation, trusted client distribution,
passkey association and the real human-verification mechanism. These checks
provide none of those trust sources. Format 4 retains its closed schema and
mixed policy/revocation history; formats 2 and 3 remain unchanged.
The retained-checkpoint limitations still apply: restoring the database and all
external expectations can defeat rollback comparison. Checking a local verified
view does not establish global freshness.

The separate [format-5 admission experiment](ATOMIC-DEVICE-ADMISSION.md) retains
verifiable witnesses and derives counter and consumption state in a new history.
It uses its own live tokens and accepted fixture trust; this checking result
cannot be converted into one of its new-admission capabilities.
