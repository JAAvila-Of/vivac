# Experimental unaccepted initial proposal

This helper-only profile constructs an unaccepted proposal from a strict draft
and live validated registration. It replaces a caller-supplied opaque final
commitment in this experiment. It establishes neither organizational mandate nor
principal acceptance, transport authentication, trusted-client distribution or
durable admission. The [initial-trust contract](TEAM-BOOTSTRAP.md) still governs
those requirements. No production bootstrap wire or acceptance key is selected.

## Bounded draft

Draft input is UTF-8 JSON of at most 8 KiB. Every field below is required;
unknown, duplicate, missing and incorrectly typed fields refuse at every level.
No acceptance, review or verification flags are supported. Field order and JSON
whitespace in input are immaterial; the validated output has the fixed order
below. No field is omitted from that output.

| Object | Fields in serialization order |
|---|---|
| Draft | `version`, `profile`, `purpose`, `candidate`, `coordinator`, `webauthn`, `grants`, `project_grants`, `governance_grants`, `recovery`, `content_key_custody` |
| Coordinator | `endpoint`, `spki_sha256` |
| WebAuthn | `rp_id`, `origin`, `registration_profile`, `user_verification` |
| Grants | `capabilities`, `delegation`, `enrollment_policy_administrator`, `own_additional_device` |

`version` is integer 1, `profile` is `initial-proposal-probe-v1`, and `purpose`
is `initial-organization`. The candidate is the existing complete strict public
possession-proof DTO, in its declared field order. Verify its signature, labels,
identifiers and time window with the existing verifier. Its `base_revision` must
be zero for this new-organization experiment. Its organization, principal,
device, signing credential, public key and operation identify the proposed
initial association and operation; they establish no mandate or uniqueness.

The endpoint equals the canonical HTTPS origin, without a trailing slash. This
profile supports the registration verifier's exact RP/host relationship, with no
credentials, query, fragment or application path. The SPKI expectation is exactly
32 bytes encoded as 64 lowercase hexadecimal characters: SHA-256 of the DER
SubjectPublicKeyInfo, not the certificate DER or a raw key. This records an
expected TLS peer public-key fingerprint; the helper does not authenticate a TLS
connection or prove the independent provenance of that expectation. The real
transport and client profile remains unaccredited.

`registration_profile` is `passki-0.5-registration-probe-v1` and
`user_verification` is `required`. Capability names are the closed strings
`grant-project-access`, `manage-members`, `manage-organization-capabilities`,
and `manage-projects`. The array must be strictly ascending in ASCII order,
with at most four entries; an explicitly empty array is valid. Duplicates,
unsupported entries and unsorted arrays refuse rather than being repaired.
The two enrollment-policy values are required booleans. Their scope is the
candidate's exact principal and organization, with no implied grants.

`delegation` is exactly `organization-model-v2`: it declares the existing model's
organization-scoped capability and enrollment-policy administration semantics,
not a narrower delegation limit. This profile cannot encode restricted
delegation; any other value refuses. No authority object is constructed from it.
Both resource-grant arrays must be explicitly empty. `recovery` and
`content_key_custody` must each be `not-established`. Establishing project or
governance grants, recovery or content-key custody needs a separate reviewed
profile; unsupported policies refuse instead of being dropped.

Canonical draft bytes are the compact UTF-8 serialization of these fixed-schema
structs, in the declared field order, with no trailing newline. All strings in
this profile are ASCII; numbers are integers and booleans are JSON booleans.
The existing proof DTO order is `challenge`, `signature`, `status`, `authority`,
`challenge_use`; its challenge follows the existing DTO declaration. This is a
schema-specific encoding, not a general JSON canonicalizer or RFC 8785 claim.
The draft commitment D is SHA-256 of ASCII
`vivac.bootstrap.draft-probe.v1`, one zero byte, and the canonical draft bytes.
The immutable parsed draft exposes those bytes and D, without an acceptance token.
It can be parsed again from public input; doing so creates data, not a live
registration or confirmation capability.

## Live registration to final proposal

Register using the draft's exact candidate proof and D with the existing
[registration profile](PASSKEY-REGISTRATION.md). Proposal preparation consumes
that live unconfirmed registration and the parsed draft. It reuses the existing
confirmation reservation checks for the exact proof, D, verifier RP/origin,
signature, retained state and original wall-clock and monotonic limits. It cannot
accept an archive or a descriptor supplied separately by the caller.

Reserve final context before constructing F. The helper then constructs the
complete proposal itself; the caller supplies neither F nor a replacement
registration descriptor. The compact JSON has this fixed field order:

| Object | Fields in serialization order |
|---|---|
| Proposal | `version`, `profile`, `draft`, `draft_commitment`, `registration`, `context` |
| Registration | `commitment`, `context`, `descriptor`, `response` |
| Either context | `nonce`, `issued_at`, `expires_at` |
| Descriptor | `aaguid`, `counter`, `backup_eligible`, `backup_state`, `algorithm`, `public_key`, `credential`, `transports`, `attachment`, `opaque_user_handle` |

Proposal version and profile match the draft. `draft` is the complete validated
draft, not a shortened projection. Commitments, context nonces and AAGUID use
lowercase hexadecimal. Descriptor key, credential, handle and the exact original
registration response use unpadded base64url. Integers and flags retain their
validated values. Transport order and optional attachment are retained; absent
attachment serializes as explicit null. This describes the initial registration
state, never the later assertion counter or backup-state output.

Final proposal bytes are limited to 64 KiB. F is SHA-256 of ASCII
`vivac.bootstrap.proposal-probe.v1`, one zero byte, and those compact bytes, with
no trailing newline. The proposal contains neither F itself nor final evidence.
Its public comparison data does not establish an independent expectation.

The ready proposal owns its private reservation. Starting it consumes that stage
and calls the existing final-confirmation verifier with the internally computed
F, exact R and reserved context. Finishing consumes the pending stage and uses
the original proof and D. It verifies the new signing proof and passkey assertion
under [the existing confirmation profile](FINAL-PROPOSAL-PROBE.md). Both start
and finish check that retained bytes still hash to the retained commitment.
Success returns the unchanged complete proposal alongside scoped possession
evidence. It creates no `TrustedPasskeyBinding`, authority object, association,
grant, checkpoint or durable record.

Draft, ready, pending and possession-confirmed proposal wrappers expose no
mutable packet or token importer. Live stages are neither cloneable nor
deserializable. A public packet cannot recreate them. Raw registration bytes
and all accepted metadata remain retained, without conversion through fixture 5.
No network, account command, trust-store modification or live key custody is
added. Existing fixture formats and the standalone confirmation experiment remain
unchanged.

## Required checks

Cover deterministic compact bytes and D/F with an independent public vector.
Exercise the complete owned flow, and reject malformed, oversized, missing,
duplicate or unknown draft fields; unsupported policies; invalid candidate
proofs, nonzero base revision and expired windows; replacement proof, D, RP,
origin, registration or context; modified packet bytes; and missing or invalid
final possession evidence. Every changed bound draft field must change D or
refuse. The complete proposal must retain the exact R response and descriptor.

Use the isolated browser's same virtual authenticator for create and get through
this wrapper. Synthetic verification remains simulation. All compilation,
fixture generation and tests run in owned resource-limited containers, through
the shared Rust coordinator where applicable; pilot runtime checks use Ubuntu
24.04. These checks do not replace either independent acceptance, a physical
authenticator check or the requirements still open for production.
