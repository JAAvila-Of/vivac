# Enrollment approval requirements

An [enrollment possession proof](DEVICE-ENROLLMENT-PROOFS.md) does not approve a
device. An existing authorized principal must separately approve the exact
candidate with fresh user verification. The first administrator requires its
own reviewed bootstrap. The coordinator probe does not implement this approval
boundary or expose an admission command.

## Isolated candidate profile

The coordinator probe evaluates Passki 0.5.0 with default features disabled and
only its Graviola backend enabled, pinned to Graviola 0.4.5. The active dependency
graph must exclude the rejected `rsa` crate and pass the existing audit policy
without exceptions. This is an experimental assertion boundary, not adoption of
a production approval system. The product's dependency policy remains separate.

The profile requires EdDSA or ES256 credentials supplied as external public
fixtures. The application must associate the exact credential and opaque user
handle with its trusted binding. The outer credential type and identifier must
be consistent; a user handle, when present, must match that binding. Both outer
JSON and decoded client data need strict field and duplicate handling while
preserving the original signed bytes for the library verifier.

Authentication data is limited to its fixed 37-byte header. Attestation data,
extensions and reserved flags are outside this profile. User presence and user
verification are mandatory. Backup eligibility must remain consistent with the
binding; backup state and the authenticator counter may change only after
successful signature verification. This fixed-header policy does not implement
signature, CBOR or base64 algorithms; maintained libraries provide those.

Ceremony state must be private, owned and neither cloneable nor serializable by
the wrapper. It binds the candidate, trusted binding instance and generation,
relying-party configuration and time window. Finishing consumes that in-memory
object even when refused. Success updates the in-memory credential and
invalidates ceremonies issued against its previous generation, including for
zero-counter authenticators. This is not durable nonce consumption or rollback
protection; the pending SQLite cache and admitted authority remain separate.

Verification can also return an opaque prepared update holding an exclusive
borrow of the binding. Dropping it leaves that binding unchanged; publishing it
applies the verified counter, backup state and next generation. The
[coordinated store check](TEAM-AUTHORITY-STORAGE.md#coordinated-assertion-checks)
uses this boundary to confirm its authority view before publishing a result.
That result still establishes neither sufficient enrollment permission nor a
durable admission.

The additional-device path also applies the separate
[owner-consent policy](TEAM-AUTHORITY.md#additional-devices-and-owner-consent).
An ordinary owner needs explicit permission for that scope; a member
administrator's assertion does not replace the owner's. Its fixture policy and
distinct in-memory token are not a production enrollment grant or an admitted
credential registry.

The WebAuthn challenge must itself commit to the candidate scope. Associating
an unrelated random challenge with a database row is insufficient. The
version-one challenge is SHA-256 over this ordered encoding:

| Field | Encoding |
|---|---|
| Domain | ASCII `vivac.device.enrollment-assertion.v1`, followed by one zero byte. |
| Candidate | The complete 251 canonical enrollment-challenge bytes. |
| Approver organization and principal | 26 canonical ASCII bytes each. |
| Approver credential | Unsigned 16-bit big-endian length, then raw credential bytes; 1–1024 bytes. |
| Relying-party identifier | Unsigned 16-bit big-endian length, then canonical hostname bytes; 1–253 bytes. |
| Origin | Unsigned 16-bit big-endian length, then canonical HTTPS origin bytes; at most 2048 bytes. |
| Ceremony nonce | 32 fresh CSPRNG bytes. |
| Ceremony issuance and expiry | Unsigned 64-bit Unix seconds, big-endian, eight bytes each. |

The request and private authentication state use that same digest as their
challenge. The public request also carries the nonce and ceremony times. Expiry
must equal the candidate's expiry; beginning a ceremony cannot renew the
candidate. Finishing must recompute the digest before invoking the library.

The future trusted client must independently recompute this commitment from
the candidate and approver configuration it expects before requesting user
verification. Trusting only a server-supplied digest or remote preview would
not establish which operation the person intended. The probe provides the
encoding boundary, not that human client or a reviewed approval ceremony.

### Additional-device challenge

The additional-device path uses a separate challenge domain. Its context digest
is SHA-256 over ASCII `vivac.device.additional-context.v1`, one zero byte, the
complete 466-byte [prior-client endorsement message](TEAM-AUTHORITY-STORAGE.md#additional-device-policy-checks),
and the exact 64-byte endorsement signature. The authentication challenge is
SHA-256 over ASCII `vivac.device.additional-assertion.v1`, one zero byte, the
32-byte version-one commitment above and that 32-byte context digest, in order.
Thus the assertion commits to the additional-device purpose, prior credential,
complete authority checkpoint, policy and endorsement as well as the candidate.
The generic version-one encoding remains unchanged.

The server verifies the endorsement before creating this context and recomputes
it during the current-authority check at completion. Private ceremony state
distinguishes generic and additional-device requests. Neither a generic assertion
nor an assertion for a different context can complete that additional ceremony.

The experimental client boundary compares the complete candidate and signed
possession proof against independently supplied fixture expectations. It verifies
the prior endorsement against an expected public key, recomputes the challenge
using the expected principal, WebAuthn credential, RP and canonical HTTPS origin,
and requires exactly that credential with user verification required. Extensions,
transport hints and authenticator hints are outside this client profile. Checked
parameters are owned and immutable; taking the authentication options consumes
the check, refuses expiry or clock rollback, and caps the timeout at the remaining
candidate lifetime. These checks neither request authentication nor attest that
a person reviewed or approved the operation.

Those expectations have no production trust loader. Copying an expected candidate,
policy or key from the same untrusted remote response does not make it independent.
The boundary accepts typed experimental inputs; it provides neither a wire parser
nor an enrollment command.
A trusted candidate-transfer channel, accepted authority and credential registry,
trusted client distribution and actual authenticator interaction remain required
before this boundary can support a real enrollment flow.

The separate [local candidate inspection command](DEVICE-ENROLLMENT-PROOFS.md#local-candidate-inspection)
can verify a public possession proof and compare its complete candidate reference
with a separately obtained value. Its result does not populate those fixture
expectations, authenticate the transfer path or admit a trust record. An expected
checkpoint, policy, prior credential and WebAuthn configuration still need their
own accepted provenance.

The fixture client also requires an owned candidate selection before checking an
additional-device request. Creating that selection verifies a public proof and
compares its complete candidate reference with a mandatory canonical expected
reference. It retains the exact 251 canonical candidate bytes and the supplied
selection time in private memory. The check consumes the selection and refuses
clock rollback, expiry or a different candidate in either the proof or the local
configuration, even when a replacement has a valid proof, endorsement and request.
This preserves the selected candidate across that local invocation; it does not
authenticate where the expected reference came from.
The fixture API takes timestamps from its caller; it compares those supplied
values and does not establish a trusted clock source.

The selection has no cloning, serialization, persistence or receipt-import API.
The command's JSON inspection receipt cannot be turned into such a selection by
deserialization. A new selection can still be created from the same proof and
reference while valid: consuming an in-memory object is not durable replay or
nonce-consumption protection. Selection neither grants permission nor records
human approval, and it does not replace the current-authority checks.

The evaluated server profile is Linux on x86_64 with AES, SSSE3, AVX, AVX2, ADX,
BMI2 and PCLMULQDQ available. The wrapper must check these capabilities before
using Graviola and refuse other profiles. Ubuntu 24.04 alone does not establish
CPU compatibility. Graviola documents its early maturity and platform
requirements; production support and operational suitability remain deployment
review gates.

Signing fixtures are public test keys held only by tests, including the P-256
example in [RFC 6979, Appendix A.2.5](https://www.rfc-editor.org/rfc/rfc6979.html#appendix-A.2.5).
They can produce cryptographically valid assertions without a human. No passing
fixture test establishes Windows Hello, a trusted client, a real authenticator
or human approval.

## Required verification boundary

A maintained assertion verifier must check the signature, credential,
unpredictable challenge, relying-party identity, exact trusted origin, user
presence and required user verification. A device signature, client boolean,
PIN sent to Vivac or delegated automation credential cannot substitute for it.

Ceremony state must remain on the server and bind the exact canonical candidate
challenge, approver, credential, operation, current authority revision and time
window. It must not be supplied by the client. Responses need explicit input
bounds before parsing. A changed candidate, expired challenge, revoked
credential or obsolete authority must refuse.

The implementation must keep authenticator counter and credential state current.
Concurrent ceremonies must not validate against a snapshot made obsolete by
another successful verification. If the maintained API hides its counter,
exposing dangerous credential internals or interpreting an unchanged zero
counter as universal failure is not an acceptable workaround.

## Atomic admission and trust

The separate [registered assertion checks](REGISTERED-DEVICE-ASSERTIONS.md)
revalidate the format-4 registry, explicit own-device permission and prior-client
endorsement at both ends of a ceremony. They still retain tokens and passkey
updates only in memory; they do not satisfy the atomic admission requirements
below. The separate [format-5 experiment](ATOMIC-DEVICE-ADMISSION.md) evaluates
durable witness verification and admission with public fixtures. Accepted
organizational trust and real human verification remain production requirements.

Approval verification alone cannot grant membership. Admission must compare the
issued candidate, recheck the approver's current enrollment authority and
credential, and atomically record the new binding, credential update and
consumption of approval and enrollment. An uncertain commit requires durable
reconciliation, not another approval or silent resubmission.

The pending-evidence table is not a trusted credential registry. Its contents
are outside the authority checkpoint. Neither that cache, a database login nor
an infrastructure administrator can establish the approval trust root.

Trusted client code, relying-party identity, HTTPS trust, bootstrap and
credential custody must be reviewed before registering real passkeys. Software
authenticators can produce signed simulated user-verification flags for tests;
those tests do not establish human interaction or comprehension. A real flow
must establish how its trusted authenticator supplies fresh verification.

## Rejected dependency chain

The evaluated WebAuthn 0.6.2 graph selected `crypto-glue` 0.2.2 and
`rsa` 0.10.0-rc.19. The latter fails
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), whose
patched range starts at 0.10.0. Updating that dependency alone fails because
`crypto-glue` 0.2.2 requests the `sha2` feature removed from RSA 0.10.0.

This chain is not accepted. A vulnerability exception, local cryptographic fork
or unsupported downgrade cannot turn it into an approved verifier. The
[upstream security policy](https://github.com/kanidm/webauthn-rs/blob/master/SECURITY.md)
normally covers only the latest release and current development branch; using
an older release requires evidence of maintained support.

Re-evaluation requires a compatible maintained dependency chain that passes
the project's audit and MSRV gates, followed by functional and adversarial
ceremony tests. The rejected WebAuthn 0.6.2 chain and its proposed license-policy
exceptions are not retained in the probe. The separate Passki profile above
must pass its own gates; a different backend does not inherit acceptance from
the rejected experiment.
