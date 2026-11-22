# Device enrollment possession proofs

A possession proof connects a [protected local key](DEVICE-KEYS.md) to one
explicit enrollment challenge. It proves that the candidate signed those
fields. It does not establish a person's identity, enrollment approval,
membership, current permissions or that the challenge is still unused.

## Commands and inputs

On Windows and macOS, `vivac device-key prove <absolute-key-directory> --challenge
<absolute-challenge-file> [--json]` loads the protected key and signs a valid,
matching challenge. It does not change the key package or register a device.

On all supported platforms, `vivac device-key verify-proof
<absolute-proof-file> [--json]` verifies the public proof's signature and its
local time window. It does not open a credential store. Its key is the
candidate's self-declared public key, not a trusted authority binding.

Challenge and proof files are bounded at 8 KiB before JSON decoding. Inputs
must be regular files without followed links or multiple hard links. Unknown
fields, duplicate fields, unsupported versions and trailing JSON data refuse.
Errors do not echo input values. Neither command makes a shared-admission
request, locates a project tree or updates the machine's vivac registry.

The challenge is a JSON object with exactly these fields:

| Field | Required value |
|---|---|
| `version` | Integer 1. |
| `organization_id`, `principal_id` | Intended organization and principal, as canonical opaque identifiers. They are claims, not authenticated identities. |
| `device_id`, `credential_id` | Canonical identifiers matching the protected key package. |
| `public_key` | Matching Ed25519 public key, 64 lowercase hexadecimal characters. |
| `operation_id` | Canonical identifier of this enrollment operation. |
| `base_revision` | Expected authority revision, unsigned 64-bit integer. It is not checked against shared state here. |
| `nonce` | 32 challenge bytes, encoded as 64 lowercase hexadecimal characters. |
| `issued_at`, `expires_at` | Unsigned Unix seconds defining the challenge window. |

Canonical identifiers use the 26-character lowercase Crockford format already
used by vivac. The issuer must generate unpredictable challenges and operation
identities from cryptographic randomness. A verifier cannot infer that quality
from the format alone.

The local check requires `issued_at <= now < expires_at` and a positive window
of at most 300 seconds. It uses the local operating-system clock without a
fallback if time is unavailable. This is a security expiry, not a performance
deadline or a global freshness claim. The coordinator must enforce its own
current clock and stored challenge lifecycle during admission.

The proof file has exactly five fields: `challenge`, containing that complete
object; `signature`, containing a 64-byte signature encoded as 128 lowercase
hexadecimal characters; and the fixed labels `status: "possession_only"`,
`authority: "not_checked"` and `challenge_use: "not_checked"`. The labels are
format constraints, not trusted assertions from the file. Verification checks
them and independently verifies the signature and local time window.
Both commands return this complete document with `--json`, so the output of
`prove` can be saved directly and passed to `verify-proof`.

## Local candidate inspection

`vivac device-key inspect-proof <absolute-proof-file>
[--expected-candidate-sha256 <64-lowercase-hex-digits>] [--json]` verifies the
proof and local time window using the same bounded, link-refusing public-file
reader. It then shows the complete candidate: organization, intended principal,
device, credential, public key, operation, base revision, nonce and times.
It neither opens a key store nor writes a project, registry or trust record.

The candidate reference is SHA-256 over ASCII
`vivac.device.candidate-reference.v1`, one zero byte, and the complete 251-byte
canonical challenge defined below. The full 64 lowercase hexadecimal digits
must be retained. JSON ordering, whitespace and equivalent string escapes do
not change this reference. A change to any candidate field changes the input;
the proof signature and output labels are outside the reference. Signature,
labels and expiry are still verified before reporting it.

An expected reference is optional. When supplied, exactly one canonical value
is required and a mismatch refuses without public output. A reference obtained
from the same untrusted server as the proof does not establish independent
intent. A person must obtain and compare it through a separately trusted path
from the installed client controlling the new device. This command does not
authenticate that path or establish trusted client distribution.

The version-one JSON inspection receipt has `version: 1`, `candidate`,
`candidate_sha256`, `expected_reference` (`matched` or `not_supplied`),
`status: "possession_only"`, and `authority`, `approval` and `challenge_use`,
each `"not_checked"`. It is a review receipt, not an interchangeable proof file.
The existing `prove` and `verify-proof` JSON documents remain unchanged.
Matching a reference establishes candidate equality only: it grants no
membership, enrollment permission or human approval, and checks neither current
shared authority nor durable challenge consumption.

## Exact signed bytes

Version 1 signs this concatenation directly with Ed25519:

1. ASCII `vivac.device.enrollment-proof.v1` and one zero byte.
2. Organization, principal, device and credential identifiers, 26 ASCII bytes each.
3. The public key decoded into 32 bytes.
4. The operation identifier, 26 ASCII bytes.
5. Base revision as eight big-endian bytes.
6. Nonce decoded into 32 bytes.
7. Issuance and expiry as eight big-endian bytes each.

All fields have fixed widths. JSON order, whitespace and equivalent JSON
string escapes do not change these canonical signing bytes. Identifier and
hexadecimal representations must still be canonical. Unsupported semantics
require a new version/domain; extra fields cannot be silently ignored.
Verification uses `ed25519-dalek`'s strict verification and refuses malformed
or weak public keys. The separate domain prevents this proof from being used
as a signature over a shared-authority change or an arbitrary caller message.

## Admission remains separate

The separate [approval requirements](DEVICE-ENROLLMENT-ASSERTIONS.md) remain
integration gates. Virtual user verification is not human approval and does not
enable admission.

A valid proof may be generated or verified more than once during its window.
These commands deliberately keep no challenge-consumption database and cannot
authorize admission. The coordinator must compare the exact challenge it
issued, bind it to its intended operation and current authority, and consume
it atomically with durable enrollment. Changed requests, consumed challenges
and revoked credentials must not gain access through a prior proof.

An existing authorized principal must separately approve adding the candidate,
with the required fresh human verification. First-administrator bootstrap
needs its own reviewed trust-establishment procedure. Possession of a candidate
key cannot provide either approval. Automation may produce a candidate proof;
that does not turn it into the person's privileged confirmation.

Private signing material stays inside the installed client's supported key
backend. Only public challenges and proofs cross this boundary. Development
tests use temporary application paths and generated test credentials; they do
not install vivac, enroll real users or change Windows Hello credentials.
