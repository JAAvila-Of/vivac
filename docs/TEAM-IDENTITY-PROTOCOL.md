# Device signatures for authority changes

This is the first cryptographic boundary for the [shared authority
contract](TEAM-AUTHORITY.md). Its executable implementation remains compiled
only for tests. It verifies possession of a known device signing key before
applying an authority change to the in-memory policy model. It does not
implement account creation, device enrollment, invitations, a login, durable
admission, human confirmation or a network service.

## Trust input

A known device binds one organization, principal, device and credential to an
Ed25519 public key. A request supplies the four identifiers, never the key
used to trust it. The verifier compares all four identifiers with that
binding and verifies the signature using the bound key. Malformed and weak
public keys are refused when constructing the binding.

In the executable model, bindings are explicit trusted test fixtures. Their
construction is not an enrollment mechanism. Production integration must
obtain bindings from authenticated, currently admitted authority, not a
request body, an editable actor field, a restored backup or an administrator's
unverified key replacement. Revocation of individual devices and credentials
needs its own admitted state before this boundary can be enabled.

A binding models a principal's device, not a delegated automation credential
or a grant of authority. A principal may eventually have several devices and
credentials. Changing a credential must not change the principal's provenance
identity. A device
signature proves possession of that credential; it does not prove a person's
presence, approval or understanding, and it grants no permissions by itself.

## Version 1 signed bytes

The message is a single byte sequence, signed directly with Ed25519. It uses
`ed25519-dalek` 3.0.0 and `VerifyingKey::verify_strict`; batch, prehashed,
legacy-compatibility and hazardous signing interfaces are not used.
The library is a development dependency while this model is test-only.

There is no JSON serialization, display formatting or implicit enum ordinal
in the signed message. Concatenate the following fields in this order:

| Field | Encoding |
|---|---|
| Domain and version | ASCII `vivac.team.change.v1` followed by one zero byte, 21 bytes in total. |
| Organization | 26 ASCII bytes of the canonical opaque identifier. |
| Principal | 26 ASCII bytes of the change's actor identifier. |
| Device | 26 ASCII bytes of the device identifier. |
| Credential | 26 ASCII bytes of the credential identifier. |
| Operation | 26 ASCII bytes of the operation identifier. |
| Expected authority revision | Unsigned 64-bit integer, big-endian. |
| Action | One tag byte followed by the payload below. |

Each identifier uses the existing canonical lowercase Crockford representation:
26 characters, first character `0` through `7`, alphabet
`0123456789abcdefghjkmnpqrstvwxyz`. There are no separators or lengths between
fixed-width fields.

| Tag | Action | Payload |
|---|---|---|
| 1 | Add principal | Principal identifier, 26 bytes. |
| 2 | Set organization capabilities | Principal identifier, then one capability mask byte. |
| 3 | Create project | Project identifier, 26 bytes. |
| 4 | Set project permissions | Principal identifier, project identifier, then one permission mask byte. |
| 5 | Revoke principal | Principal identifier, 26 bytes. |

The capability mask assigns bits 0 through 3 to managing members, managing
projects, granting project access and managing organization capabilities,
respectively. The permission mask assigns bits 0 through 4 to reading content,
writing work, proposing norms, reading reports and reading audit, respectively.
All other bits are zero. An empty set has mask zero. A changed action or mask
changes the signed bytes. The largest message is 213 bytes; its detached
signature is exactly 64 bytes.

These are canonical signing bytes for typed requests, not a complete transport
format. This increment has no untrusted wire decoder. A future decoder must
bound input before allocation and refuse unknown versions, tags, mask bits,
noncanonical identifiers, truncated fields and trailing data. It must not
reinterpret unknown input as a different supported request.

## Verification and policy remain separate

The signed admission entry point verifies the device binding and signature
before calling the existing policy transition. A valid signature cannot bypass
project isolation, missing capabilities, a revoked principal, a revision
conflict or operation reuse with a different change. Refusal leaves the
in-memory authority unchanged.

An exact authorized retry may return the existing historical receipt. The
receipt does not extend credential validity or create permission for another
operation. The signed model keeps the complete device request and its bound
public key with each admitted operation. Reusing the operation with a different
device, credential or key refuses even when the underlying policy change is
unchanged. The policy model's change-only receipt is insufficient for this
comparison. The signed model starts from a new bootstrap and exposes no raw
mutation path that could admit changes without their signed evidence. Its
evidence count shares the policy model's 1,024-operation bound.

The current model has no individual device-revocation registry;
signature verification alone must never be used to claim current credential
validity or globally fresh authority.

## Remaining enrollment boundary

Before account and invitation commands are exposed, the implementation must
establish the following through the real client and admission paths:

- Reviewed initial organization trust and the first administrator's binding.
- Proof of possession by a new device, approval by an existing enrollment
  authority, a current checkpoint and an exact binding to the intended
  organization and principal. An invitation alone grants no content access.
- Single-use, expiring invitations and atomic, durable consumption with the
  corresponding authority transition; retries reconcile the original result.
- Protected local signing-key custody, credential revocation and replacement,
  and a separate fresh human-verification mechanism for privileged approvals.
- Organizational account recovery distinct from content-key recovery by
  designated custodians. The coordinator receives neither content keys nor
  authority to manufacture participant approval.

Signing-key fixtures do not establish these properties. No private key or
credential is written to the provenance log, and the signature model generates
or stores no real credentials. The separate [Windows device-key
store](DEVICE-KEYS.md) provides explicit local credential custody, not enrollment
or shared authorization. Ordinary capture and model-facing instructions are
unchanged.

The verification choice follows the library's [strict-verification
documentation](https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/struct.VerifyingKey.html#method.verify_strict).
Cryptographic interoperability tests use the public Ed25519 test vectors in
[RFC 8032, section 7.1](https://www.rfc-editor.org/rfc/rfc8032.html#section-7.1).
