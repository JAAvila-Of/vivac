# Own-credential revocation model

The isolated registered-authority model evaluates individual signing-credential
revocation in memory. It owns the [version-two authority](TEAM-ENROLLMENT-POLICY.md)
and an explicitly supplied fixture catalogue. Signed policy changes and signed
revocations share one revision order. The separate
[storage-format-4 experiment](TEAM-AUTHORITY-STORAGE.md#registered-history-in-format-4)
persists and replays that mixed fixture history. Neither model establishes a
trusted production registry, fresh human verification or a server endpoint.

## Registry boundary

Fixture bootstrap supplies the organization, initial principal, initial
enrollment-policy administrator and public signing bindings. A binding contains
the organization, principal, device, credential and Ed25519 verification key.
The catalogue accepts at most 128 bindings, including subsequently revoked
entries. Device identifiers, credential identifiers and verification keys must
each be unique across the catalogue. Every binding belongs to the same
organization, and the initial principal must have a binding.

Bindings for other fixture principals do not make those principals members.
Their credentials remain unusable until the principal is active in the owned
authority. Predeclaring a fixture binding is not an enrollment mechanism.
An existing authority history cannot be imported into this constructor: its
device validity at each historical transition would need separate verification.

Every registered entry point resolves the exact organization, principal,
device and credential against this catalogue, checks current principal and
credential validity, and selects its verification key internally. Requests do
not supply a trusted key. The wrapper exposes no mutable authority or registry
reference through which those checks can be bypassed.

## Authority to revoke

An active principal may sign a request to revoke one of its own exact device
and credential bindings. This removes a signing credential; it grants no role,
new key or replacement binding. Membership administration, enrollment-policy
administration and `OwnAdditionalDevice` do not confer authority to revoke
another principal's credential.

The signing credential may revoke itself. Revoking the last usable credential
does not create a substitute, reactivate another binding or start recovery.
Administrative revocation and separately authorized recovery require their own
reviewed contracts. They are not implicit privileges in this model.

A revoked binding remains a permanent tombstone. Its identifiers and key stay
reserved, and the model has no removal, replacement, reactivation or enrollment
operation. Other credentials of the same principal remain independently usable
while that principal is active. Revoking a principal still makes all of its
credentials unusable through the current-membership check.

## Canonical request

Revocation uses a separate signing domain. It does not add a tag or change the
meaning of an existing version-one or version-two policy request.

| Field | Exact representation |
|---|---|
| Domain | ASCII `vivac.team.credential-revocation.v1`, followed by one zero byte |
| Organization | 26 canonical ASCII identifier bytes |
| Actor | 26 canonical ASCII identifier bytes |
| Signing device | 26 canonical ASCII identifier bytes |
| Signing credential | 26 canonical ASCII identifier bytes |
| Operation | 26 canonical ASCII identifier bytes |
| Expected authority revision | Unsigned 64-bit big-endian integer |
| Target device | 26 canonical ASCII identifier bytes |
| Target credential | 26 canonical ASCII identifier bytes |

The request is exactly 226 bytes, with a separate 64-byte Ed25519 signature.
Parsing refuses every other length, noncanonical identifier, unknown domain,
truncation and trailing byte. Re-encoding must reproduce the exact bytes.
Signature verification is strict and uses the currently active signing binding
resolved from the registry. A signature cannot substitute for authorization.

## Shared ordering and retries

A new policy change or revocation must name the current expected revision.
Either transition advances that same revision once. Their combined operation
history is bounded to 1,024 entries; an operation identifier cannot be reused
between the two request types. Revocations cannot race in an independent order
that policy checks would overlook. All refusal checks precede mutation.

An exact retry can return the original revision only after checking the current
principal, signing credential and signature. A changed envelope or key under
the same operation identifier refuses. Replaying a revocation receipt does not
apply it again, undo another transition or renew permission. An exact retry may
still return its history after the target was revoked, provided the signing
credential remains valid. A self-revoked signer cannot retrieve that receipt
through the same credential. A surviving authorized credential must perform
any separately specified audit or recovery operation under its own identity.

The same current-credential check also applies to historical version-two policy
receipts. Revoking a signer therefore prevents that signer from reconciling an
older policy operation, even if the operation originally succeeded. Revocation
does not erase the retained historical evidence.

## Integration requirements

The fixture model holds public test bindings only. It has no credential loader,
wire transport, clock, passkey registration, human-confirmation mechanism,
content key or live account operation. Content keys remain with authorized
trusted clients or designated recovery custodians; registry state cannot grant
the coordinator those keys.

The existing storage formats 2 and 3 do not accept this mixed history. They
retain their closed schemas, signing contracts and fixture sources. Format 4
reconstructs credential validity from the ordered signed fixture history and
checks retained checkpoints. Production integration still needs accepted
bootstrap, current trusted checkpoints, atomic credential and approval-state
updates, and reconciliation after uncertain commits. Copying fixture bindings
or a backup into a database cannot establish that provenance.

The [device-approval boundary](DEVICE-ENROLLMENT-ASSERTIONS.md) still requires
accepted bootstrap, trusted client distribution, exact relying-party and origin
configuration, fresh verification and atomic consumption. Passing simulated
signing tests supplies none of those. No device enrollment or production
revocation is enabled by this model.
