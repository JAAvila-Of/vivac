# Experimental acceptance-signature verification

This helper-only profile verifies two signed experimental statements alongside
the existing passkey assertion and final signing-device proof. It does not
establish organizational mandate, independent expectation provenance, human
review, trusted-client distribution, transport authentication or durable
admission. Every acceptance-related public type and entry point is explicitly
fixture-only. No production acceptance key or bootstrap wire is selected.
The [initial-trust contract](TEAM-BOOTSTRAP.md) remains mandatory for production.

## Bind accepting identities before registration

The [version-1 proposal experiment](INITIAL-PROPOSAL-PROBE.md) remains unchanged:
its bytes, commitments, strict field rejection and possession-only result are
preserved. It cannot enter this fixture acceptance flow.

The new draft uses integer `version: 2` and
`profile: "initial-acceptance-probe-v1"`. It otherwise preserves the version-1
draft schema, validation and serialization order, inserting the required
`acceptance` object immediately after `grants`. Its fields, in order, are:

| Field | Required value |
|---|---|
| `organization_authority_id` | A canonical opaque 26-byte identifier using the existing identifier grammar. |
| `organization_authority_public_key` | A strong Ed25519 public key, 32 bytes as lowercase hexadecimal. |
| `principal_acceptance` | Exactly `candidate-signing-key`, an explicit choice for this experiment. |

These are declarations, not evidence that the organization designated the key.
Key validation uses the existing Ed25519 verifier: decode a valid verifying key,
reject `is_weak`, and use `verify_strict` for signatures. No new curve arithmetic
or signature implementation is introduced.
The principal signer is explicitly the candidate's principal and signing key.
No organizational key is inferred from that key or from TLS. Independently
supplied fixture expectations must match all these declarations and the exact
proposal commitment before signature requests are returned.

Version 1 rejects any `acceptance` field, including null. Version 2 refuses a
missing, null, unknown, duplicate, weak-key or incorrectly typed acceptance
object. All previous required fields, unsupported-policy rejections, 8 KiB
draft and 64 KiB proposal bounds remain in force. No role or grant is added.

The version-2 draft commitment is SHA-256 of ASCII
`vivac.bootstrap.acceptance-draft-probe.v1`, one zero byte, and the compact draft
bytes. Register with that exact D. Reserve the final context before constructing
F. The final proposal retains the original schema and complete registration,
with outer version/profile matching the version-2 draft. F is SHA-256 of ASCII
`vivac.bootstrap.acceptance-proposal-probe.v1`, one zero byte, and those compact
proposal bytes. The existing final-possession challenge already binds D, R and F.

Consequently, changing either accepting identity or its declared key requires a
new draft, registration and proposal. It invalidates both acceptance signatures,
the final passkey assertion and final signing-device proof. Adding a different
accepting key after sealing F is not supported.

## Separate fixture expectations

Fixture setup explicitly supplies the expected organization, operation, complete
F, organizational signer identifier/key and principal signer identifier/key.
Identifiers and keys are validated; the principal must equal the candidate
association, and the organizational signer must equal the declared acceptance
object. There is no default signer, fallback to the candidate key or
`approved`/`reviewed` input. The fixture expectation type is neither
deserializable nor a production trust anchor. Constructing it from public data
does not authenticate that data's provenance.

One configured key may serve both roles only when explicitly supplied for both
in fixture setup. Both distinct messages and signatures remain required. This
tests role separation; it does not prove that a real organization authorized
that arrangement or that two different natural people acted.

## Two distinct signed messages

Each request message contains the following bytes, in order. There is no JSON
statement importer or unsigned statement metadata selected by the caller.

| Field | Encoding |
|---|---|
| Role domain | Organization: ASCII `vivac.bootstrap.organization-acceptance-probe.v1`, then zero. Principal: ASCII `vivac.bootstrap.principal-acceptance-probe.v1`, then zero. |
| Proposal | The exact 32-byte F. |
| Organization and operation | The candidate's canonical identifiers, 26 ASCII bytes each. |
| Signer | The expected role's identifier (26 ASCII bytes) and Ed25519 public key (32 bytes). |
| Reserved context | Nonce (32 bytes), issuance and expiry (unsigned 64-bit big-endian seconds each). |

The context is the existing final reservation included in F; no new nonce,
timestamp or validity period is introduced after sealing. Request messages are
returned as unpadded base64url for the separate signing actions. A signature is
exactly 64 bytes and is verified strictly with the role's expected Ed25519 key.
Signature verification establishes only that experimental statement under that
configured key. It does not accredit the signer or their review ceremony.

Candidate possession signatures, final device signatures, passkey assertions,
the other role's acceptance signature and signatures for another packet cannot
substitute. Matching keys do not remove the domain separation.

## Owned composition and retained evidence

Beginning fixture acceptances consumes a live ready version-2 proposal, checks
its complete retained bytes, expected bindings and original time limits, and
returns the two messages with a private pending stage. Finishing consumes that
stage, verifies both signatures and checks scope and time again after crypto.
If either fails, no partially verified result is published.

The fixture-verified ready wrapper retains both messages, both signatures and
their exact expected signer bindings alongside the original ready proposal.
Starting the fixture proposal consumes it and delegates to the existing
proposal start operation, preserving F and the original reservation. Finishing
consumes the pending fixture proposal and delegates to the existing passkey and
signing-device confirmation. The final fixture result retains the complete
packet, the two acceptance-statement evidences and the two possession evidences.

Each live stage is private, non-cloneable and non-deserializable. Failure and
restart cannot recreate it from public evidence. All entry points recheck the
original scope and deadline; no acceptance renews the ceremony. No stage creates
an `Authority`, `TrustedPasskeyBinding`, admitted association, grant, checkpoint,
database record or production acceptance capability. The standalone version-1
proposal and possession APIs remain available with their original meaning.

## Required checks and laboratory boundary

Keep the original version-1 independent vector byte-identical. Add an independent
public vector for version-2 draft/D, complete registration/R, packet/F, both
acceptance messages/signatures and the final assertion/device signature. Use
OpenSSL in the owned container to sign and verify public RFC test keys.

Cover missing/invalid acceptance declarations, mismatched independent fixture
expectations, weak keys, non-version-2 proposals, all role/key substitutions,
swapped signatures including an explicitly shared key, invalid signature
lengths and bytes, changed packet/grants/scope/context, expired or rolled-back
time, original monotonic deadlines and failure without partial publication.
Exercise a complete browser flow with both fixture statements and the same
virtual authenticator for registration and final assertion, plus refusal before
the final assertion when an acceptance is invalid.

Compilation, auxiliary generation and all tests remain in owned resource-limited
containers, through the shared Rust coordinator where applicable. Pilot runtime
checks use Ubuntu 24.04. Real user review, actual organizational designation,
independent distribution, TLS trust, hardware and production admission remain
separate prerequisites, not results of these simulations.
