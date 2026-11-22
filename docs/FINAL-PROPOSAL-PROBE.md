# Experimental final-possession confirmation

This helper-only profile tests possession after an unconfirmed passkey
registration. It implements no organizational acceptance, trusted-client
distribution, authority bootstrap, durable admission or remote protocol. The
[initial-trust contract](TEAM-BOOTSTRAP.md) still governs those requirements.
The caller supplies an opaque 32-byte final-proposal commitment. Binding that
input does not validate its business contents or prove that anyone reviewed it.

## Ownership and stages

The registration result retains its validated public Passki state, candidate
proof, draft commitment, RP/origin and original private monotonic deadline.
These are private process-local provenance, not an archive importer. The
existing registration request, response and record commitment remain unchanged.
All accepted metadata remains in the original registration result; no fixture-5
or `TrustedPasskeyBinding` conversion is involved.

1. Reserve confirmation context by consuming the unconfirmed registration and
   checking its exact candidate proof, draft, verifier configuration and validity.
   Generate a fresh 32-byte nonce and expose its issuance and original expiry.
   The private reservation remains single-use and inherits the original deadline.
2. Fix the external proposal using that context and registration commitment.
   Start the final assertion by consuming the reservation and supplying the exact
   context and registration commitment along with the opaque proposal commitment.
   No final evidence or additional context value becomes an input after sealing.
3. Finish by consuming the pending assertion. Recheck the entire scope and
   validity, verify a fresh assertion for the exact registered key, and verify a
   new signature by the candidate signing key over the final-confirmation
   signing message. Recheck validity after cryptographic verification.

Reservation, pending confirmation and their ownership tokens are neither
cloneable nor serializable. Restart loses unfinished confirmation. Returning a
public context, a descriptor or a successful registration does not mint another
live token. A different verifier RP/origin, candidate, draft, context,
registration or proposal refuses rather than selecting replacement state.

## Experimental challenge and signing message

The version-1 challenge is SHA-256 over the ordered fields below. This is a
reviewed experiment-local encoding, not a selected production bootstrap wire
format. It uses the already accredited SHA-256, Ed25519 and Passki primitives;
it implements no signature or curve arithmetic.

| Field | Encoding |
|---|---|
| Domain | ASCII `vivac.passkey.final-confirmation.v1`, then one zero byte. |
| Registration | Its 32-byte existing record commitment. |
| Proposal | The caller's 32-byte opaque final-proposal commitment. |
| Candidate | Its complete 251 canonical challenge bytes and decoded 64-byte possession signature. |
| Draft | The 32-byte commitment originally supplied to registration. |
| RP and origin | Each preceded by its unsigned 16-bit big-endian byte length. |
| Reserved context | Nonce (32 bytes), issuance and expiry (unsigned 64-bit big-endian seconds each). |

The public WebAuthn challenge and retained Passki authentication state both use
this digest. The candidate's final signing message is ASCII
`vivac.device.final-confirmation.v1`, one zero byte, followed by that digest.
The previous candidate signature cannot satisfy this different message.

Passki may generate internal randomness while constructing authentication
options. That factory challenge is checked and replaced in both public and
private state; it is not a new proposal field or an extension of the reservation.
The request allows exactly the registered credential, requires user verification,
uses the original RP and requests no extensions. Factory failure or disagreement
refuses; it never falls back to preferred verification or discoverable selection.

## Verification and result

Assertion JSON is limited to 16 KiB, with canonical base64url components bounded
by the existing 8 KiB component and 1 KiB key limits. Unknown or duplicate JSON
fields refuse. Client data must contain `webauthn.get`, the exact challenge and
origin, and no cross-origin or top-origin ceremony. The registration profile's
exact optional Chromium marker is retained unchanged; other unknown fields refuse.
Extensions must be absent or an empty object. Attachment remains an optional
supported hint. A supplied user handle must match the retained expected handle;
absence or null is accepted only with the exact-credential request.

Authenticator data is exactly the fixed 37-byte assertion header. Require UP and
UV, the original RP hash, unchanged backup eligibility and BS only with BE.
Attested credential data, extension data and reserved flags refuse. The maintained
verifier checks the signature and counter policy. Its credential, counter and
optional user handle must agree with the strict parsing result. Unsupported
extension outputs refuse. Backup state becomes verified only after that complete
check, not merely because it appeared in the unsigned wrapper.

Successful output retains the complete original registration, proposal commitment,
reserved context, raw assertion, final device signature, verified counter and
backup state. The initial registration descriptor is not mutated. The output is
scoped key-possession evidence only: it neither constructs an admitted association
nor updates an authority database, checkpoint, generation or grant.

## Rejection coverage and boundary

The laboratory must cover a valid complete flow and reject substituted proposal,
registration, candidate, draft, RP/origin or context; reused or expired tokens;
wall-clock rollback past validity; a wrong final signing key or message; a
different assertion purpose; missing UP/UV; wrong credential, key or user handle;
counter rollback or an unchanged nonzero counter; inconsistent backup flags;
malformed/oversized JSON and factory disagreement. Tests must show that failure
consumes the owned stage and that no
descriptor, counter or authority state is published as a side effect.

Independent public vectors bind the challenge and final signing message. Real
browser emulation must register and then assert with the same virtual
authenticator while Rust retains the live tokens. Synthetic UV remains simulation.
Compilation and every test, including vector generation and auxiliary checks,
run inside owned, resource-limited containers through the shared Rust coordinator
where applicable. The pilot runtime checks use Ubuntu 24.04. The installed
product, normal browsers, trust stores
and real signing/authenticator keys are not test fixtures.
