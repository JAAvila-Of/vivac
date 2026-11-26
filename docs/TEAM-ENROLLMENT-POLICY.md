# Signed enrollment policy model

The version-two authority experiment models explicit permission to add one's own
additional device. Its grant and revocation participate in the same revision and
operation history as membership and project policy. It does not enable device
enrollment, a credential registry, human approval or a server.

The [version-one signed protocol](TEAM-IDENTITY-PROTOCOL.md) remains unchanged.
Membership, a login, `ManageMembers` and the classic governance capabilities do
not imply permission to enroll a device or administer enrollment policy.

## Separate powers

The model distinguishes three things:

- An enrollment-policy administrator can enable or disable
  `OwnAdditionalDevice` for an active principal in the same organization.
- An active principal with that explicit permission may proceed to the separate
  own-device approval checks. The permission grants no role, other identity,
  content key or approval by itself.
- Changing the enrollment-policy administrators requires both that policy role
  and `ManageOrganizationCapabilities` in the actor's current authority. Neither
  alone suffices. The recipient must be active in the same organization.

These are organization-wide powers in this model; it does not implement project
subscopes or delegated automation credentials. A policy administrator without
`ManageOrganizationCapabilities` can manage own-device grants, but cannot appoint
or remove policy administrators. Holding an own-device grant conveys neither
administrative power. The classic capability masks retain their four existing
bits; the new policy powers are represented by explicit actions rather than
unassigned mask bits.

The fixture bootstrap names its initial policy administrator explicitly. That
identifier must equal the sole initial principal. No principal, including that
administrator, receives `OwnAdditionalDevice` implicitly. The constructor is
external test trust, not a reviewed organizational bootstrap. Revoking a
principal also clears its enrollment-policy role and own-device grant. Removing
the last policy administrator is permitted and leaves those administration
operations unavailable; no role is recreated automatically.

## Version-two signed bytes

Requests use ASCII `vivac.team.change.v2` followed by one zero byte, then the same
organization, actor, device, credential and operation identifiers as version one
(26 canonical ASCII bytes each), an unsigned 64-bit big-endian expected authority
revision, and an action tag and payload.

| Tag | Action | Payload |
|---|---|---|
| 1–5 | Classic authority actions | Exact version-one payload and closed capability/permission masks. |
| 6 | Set own additional-device permission | Target principal, 26 bytes, then exactly `0` or `1`. |
| 7 | Set enrollment-policy administrator | Target principal, 26 bytes, then exactly `0` or `1`. |

For tags 6 and 7, `0` disables and `1` enables. Those messages are 187 bytes;
the maximum request remains 213 bytes. The detached signature is 64 bytes and
uses strict Ed25519 verification against the exact externally trusted device
binding. The request never supplies its own verification key.

Unknown tags, bits, noncanonical identifiers, other boolean bytes, truncation
and trailing data refuse. Re-encoding must reproduce the exact request. A
version-one request or signature cannot be reinterpreted as version two, and
the version-one decoder refuses the new domain. This codec is not a transport
parser or a claim that a trusted credential registry exists.

## One order and exact retries

Classic and enrollment-policy actions share one revision counter and one bound
of 1,024 signed operation receipts. A new operation must use the current base
revision. A policy change cannot advance a separate enrollment revision while
leaving membership behind. All checks precede mutation; a refused request
leaves the model unchanged.

An exact signed request under the same device key can return its original
historical receipt while the actor remains active. A different action, device,
credential or key under that operation identifier refuses. A historical receipt
does not apply the action again, renew a grant or authorize another operation.
Current device validity still needs an individually revocable admitted registry;
the experiment's externally supplied signing bindings do not provide one.

The separate [own-credential revocation model](TEAM-CREDENTIAL-REVOCATION.md)
evaluates a registry wrapper with a separate storage-format-4 experiment. It
resolves active fixture bindings internally and shares one revision order between
version-two policy changes and own-credential revocations. It does not replace
the fixture bindings in this standalone model or in storage format 3.

## Integration boundary

A real policy change is privileged and still needs the required fresh human
verification, current authority and atomic durable admission. A fixture device
signature does not supply those. The same owner's prior-client endorsement,
fresh verification and exact additional-device scope remain required at
[device approval](DEVICE-ENROLLMENT-ASSERTIONS.md).

The [SQLite experiment](TEAM-AUTHORITY-STORAGE.md#fixed-storage-profiles) has a
separate storage-format-3 profile for this signed version-two history. It creates
a fresh private database, replays policy grants and revocations after restart,
and serves policy queries from that verified snapshot. The format-2 profile,
version-one decoder, pending cache and old fixture permission catalogue retain
their existing contracts. Storage numbers are independent of signing domains.

There is no migration, silent upgrade, decoder fallback or operational
version-two endpoint. Durable fixture-policy history does not supply fresh human
verification, a currently revocable credential registry or authenticated client
bootstrap. Wiring those trust sources and the enrollment ceremony to the new
profile remains a separate integration requirement.
