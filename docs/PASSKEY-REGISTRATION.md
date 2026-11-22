# Isolated passkey registration profile

This coordinator-probe profile evaluates registration through Passki 0.5.0 and
Graviola 0.4.5. It returns an unconfirmed registration, outside the product's
runtime, authority history and credential stores. It creates no organization,
membership, signing binding or passkey association. The
[initial-trust contract](TEAM-BOOTSTRAP.md) still requires separate organizational
acceptance, signing-key possession and a final assertion over the complete
proposal before activation.

## Ceremony scope

The caller supplies a valid candidate possession proof and a 32-byte commitment
to the immutable registration draft. The [ceremony stages](TEAM-BOOTSTRAP.md#from-registration-draft-to-final-proposal)
distinguish this draft from the later final proposal, which includes registration
evidence not yet available at issuance. That commitment is a binding input, not
evidence that a person reviewed or authorized it. The helper does not construct
either proposal. The profile accepts an exact RP hostname
and a matching HTTPS origin without user information, a path, query or fragment.
It does not install certificates or choose transport trust on the user's behalf.

The wrapper owns the pending registration state. It is neither cloneable nor
serializable, and finishing consumes it even when verification refuses. Issuance
uses a fresh CSPRNG nonce and the candidate's existing expiry. Wall-clock checks
and a private monotonic deadline prevent beginning or finishing from extending
that window. CPU, clock, entropy or ceremony-factory failures refuse without
returning a registration.

The WebAuthn challenge is SHA-256 over the following ordered encoding:

| Field | Encoding |
|---|---|
| Domain | ASCII `vivac.passkey.registration-challenge.v1`, then one zero byte. |
| Candidate | Its complete 251 canonical challenge bytes. |
| Possession signature | The decoded 64-byte Ed25519 signature from the proof. |
| Reviewed proposal | The caller's 32-byte commitment. |
| RP identifier and origin | Each preceded by its unsigned 16-bit big-endian byte length. |
| Ceremony nonce | 32 fresh bytes. |
| Issuance and expiry | Unsigned 64-bit Unix seconds, big-endian, eight bytes each. |

Both the public options and private library state use this digest. Registration
requests advertise only EdDSA (`-8`) and ES256 (`-7`), require user verification,
request no attestation, discourage resident credentials and request no extensions.
Finishing checks the original proof, proposal, configuration and ceremony scope
again. A registration response is never an input capable of minting pending state.

## Accepted response

The strict wrapper runs before the maintained registration verifier. The profile
accepts one bounded JSON object with `id`, `type`, `rawId`, `response`, optional
empty `clientExtensionResults` and optional `authenticatorAttachment`. Its response
contains `clientDataJSON`, `attestationObject` and optional `transports`.
Unknown fields and duplicate fields refuse at every JSON level. This is an
explicit minimal response profile, not a generic importer of every browser's
`toJSON()` output. Extra derived fields such as `publicKey`, `publicKeyAlgorithm`
and `authenticatorData` are outside this profile and refuse when supplied.

Binary fields use canonical unpadded base64url. The outer type is `public-key`;
both outer credential identifiers must agree with the attested identifier.
Client data must contain `webauthn.create`, the exact challenge and origin, and
no cross-origin ceremony. A supplied `crossOrigin` value must be boolean false.
The optional `other_keys_can_be_added_here` field accepts only Chromium's
published compatibility-marker string. A null, duplicate, changed or non-string
marker refuses. The original client-data bytes, including this marker, remain
part of the retained response and its commitment. This explicitly supported
[Chromium marker](https://chromium.googlesource.com/chromium/src/+/140.0.7339.185/content/browser/webauth/client_data_json.cc)
is not an authority input. Other unknown client-data fields, including
`topOrigin`, refuse; the adapter never removes them to manufacture acceptance.

| Input | Maximum |
|---|---|
| Complete response | 16 KiB |
| Decoded client data | 8 KiB |
| Decoded attestation object | 8 KiB |
| Credential identifier | 1 KiB |
| Encoded COSE key | 1 KiB |
| CBOR recursion | 8 levels |
| Transport hints | Six unique values |

CBOR parsing uses the existing Ciborium library with an explicit recursion limit
and exact end-of-input checks. Reencoding must reproduce the input, excluding
indefinite lengths and nonminimal encodings. The attestation object has exactly
the three distinct text keys `fmt`, `authData` and `attStmt`. Only `fmt: none`
with an empty statement map is accepted. Duplicate or unknown map keys, trailing
data and unsupported shapes refuse.

Authenticator data must have the fixed header followed by its AAGUID, bounded
credential identifier and exactly one COSE key. User presence, user verification
and attested credential data flags are required. Extensions, reserved flags and
backup state without backup eligibility refuse. RP hash and all lengths are
checked before the library consumes the response.

The COSE key has exactly the required labels for Ed25519 or P-256 ES256, with no
additional or duplicate labels. Curve, key type, algorithm and coordinate lengths
must agree. Existing maintained key constructors validate the public material;
weak Ed25519 points refuse. The wrapper does not implement elliptic-curve
arithmetic, signatures or attestation algorithms.

## Explicit public metadata

The unconfirmed result retains the original response and the accepted credential
identifier, COSE key, algorithm, counter, backup flags, AAGUID, opaque user handle,
transport hints and attachment hint. Transport values are restricted to `usb`,
`nfc`, `ble`, `smart-card`, `hybrid` and `internal`. Unknown, duplicate or legacy
alias values refuse instead of being removed or normalized silently. Attachment
may be `platform` or `cross-platform`; it remains client-reported metadata.

The result commitment is SHA-256 over ASCII
`vivac.passkey.registration-record.v1`, one zero byte, the 32-byte ceremony
challenge, the original response's unsigned 32-bit big-endian length and those
exact response bytes. That commitment binds the original metadata and scope;
it does not make a client-reported hint an authenticated hardware property.

Attestation format `none` establishes no manufacturer trust or hardware provenance.
The registration's flags alone do not prove a real person's interaction. A final
assertion under the new credential remains required. Global credential uniqueness,
organizational acceptance and current authority must also be checked at admission;
this in-memory wrapper has no admitted registry against which to check them.

The result cannot be converted through an API into the
[format-5 fixture descriptor](ATOMIC-DEVICE-ADMISSION.md#bootstrap-and-state).
It does not clear metadata to fit that descriptor. Restart loses unfinished
ceremonies; this profile provides neither durable nonce consumption nor a new
authority-storage format.

## Automated laboratory evidence

Synthetic credentials exercise accepted and refused protocol inputs without
registering anything in the user's authenticator or browser. A separate public
[registration vector](../tests/data/passkey-registration-vector.json) binds
fixed public RFC 8032 material, metadata and original response bytes to independently
authored commitments. Software-generated verification flags remain simulation.

Compilation and execution use a minimal read-only source snapshot and explicit
synthetic fixtures. Build outputs, temporary data and configuration are isolated
from the installed product and earlier laboratory targets. No host Cargo command,
credential store, certificate trust store, browser profile or RDP setting is used
as a test fixture. Compiler and runtime phases have separate resource limits and
retain their actual exit status, OOM state and selected RDP observations.

The laboratory limits compilation to one CPU and 1 GiB and ordinary native test
execution to one CPU and 512 MiB, without extra swap. The suite's durable-store
cases require their own private ext4 data volume; the store deliberately refuses
tmpfs. That data volume is separate from source snapshots and build artifacts.
The laboratory monitors its size and retains failed runs rather than weakening
the store's filesystem checks.

Within a prepared isolated environment with the pinned dependencies cached, the
registration tests can be selected with:

```sh
cargo test --locked --offline --manifest-path tools/team-coordinator-probe/Cargo.toml registration::tests:: -- --test-threads=1
```

Browser/authenticator emulation and real platform integration are separate
verification layers. This profile alone does not establish that a production
client supplies the accepted RP/origin or that a real person approved the proposal.
It exposes no account, network or enrollment command.

### State for a later assertion

The next verifier must start from the exact public state validated by this
registration, while the association remains unconfirmed. The pinned Passki 0.5.0
API has no separate persisted user-verification initialization boolean.
Successful registration with required UV records what was checked during that
ceremony; it does not create an authenticated session or replace required UV
in a later assertion. Whether the final-proposal assertion is still pending is
a composition state, not a guessed library flag.

| State | Required source and treatment |
|---|---|
| Credential, canonical COSE key and algorithm | Exact accepted descriptor, checked against the maintained registration result. Never replace them with a supplied fixture or an independently submitted descriptor. |
| Initial counter and backup eligibility/state | Registration authenticator data, checked against the maintained result. Preserve the actual counter, including zero, and both backup flags. |
| Expected opaque user handle | The retained registration request's user ID, already bound to the candidate principal. It is not inferred from login or a user-supplied principal claim. |
| AAGUID, attestation type, transport and attachment hints | Preserve their profile-defined provenance and limits. Attestation remains `none`; hints are not authenticated hardware properties. |
| Resident-key and extension state | Preserve the accredited profile's absence of unsupported metadata. Do not treat an absent resident-key report as proof of a nonresident credential. |
| Verification policy | Require a new assertion with UP and UV. No initialization default, remembered registration gesture or automation token satisfies that requirement. |

For this confirmation profile, the request must allow exactly the registered
credential and require user verification. An empty list permitting discoverable
selection or a list containing another credential is outside this contract.
If an assertion supplies a user handle, it must equal the retained expected
handle; omission does not establish another principal and is accepted only
under this exact-credential request contract.

The maintained authentication verifier supplies the verified counter and checks
its policy: the new counter must exceed the old one unless both are zero. The
composition must also validate the assertion's backup flags. Eligibility must
remain unchanged; backup state cannot be set without eligibility. Flags read
from authenticator data become a verified successor only after signature
verification and all scope checks succeed. Do not assume every required flag
is independently returned by the maintained authentication result.

The successor counter and backup state are outputs of that assertion, not new
inputs to the proposal that generated its challenge. Publishing them requires
the complete admission transition. The
[final assertion contract](TEAM-BOOTSTRAP.md#state-and-purpose-of-the-final-assertion)
keeps this scoped possession evidence separate from organizational acceptance,
signing-device confirmation and authority. The helper's
[experimental possession profile](FINAL-PROPOSAL-PROBE.md) binds a fresh assertion
and signing-device proof to an opaque proposal commitment using live registration
state. It does not validate the complete business proposal or organizational
acceptance, and provides no constructor for an admitted association.

### Browser laboratory

The opt-in `browser-lab` helper feature exercises `navigator.credentials.create`
with a virtual CTAP2 authenticator. The cases cover a valid registration,
an altered origin, a changed proposal commitment and refusal without user
verification. They retain `PendingRegistration` in Rust while sending only the
public creation options to the [laboratory driver](../tools/team-coordinator-probe/lab/registration_browser_driver.py).
Driver input is limited to 32 KiB and output to 64 KiB. Missing laboratory markers,
unsupported CPUs and browser failures fail the tests rather than skip them.

The driver retains the browser's complete `toJSON()` result alongside an explicit
minimal DTO. Only the three derived response fields listed above are omitted
from that DTO. Other fields, including transport hints, attachment and extension
results, remain unchanged and are compared against the native result before
verification. No authenticator private key is exported.

A prepared Linux container supplies Chrome for Testing and ChromeDriver
157.0.8103.78 at `/assets/chrome-linux64/chrome` and
`/assets/chromedriver-linux64/chromedriver`. It also supplies Python 3, OpenSSL,
the required distribution libraries and fonts, and the driver at
`/browser-lab/registration_browser_driver.py`. The root filesystem, assets and
selected driver source are read-only. The container runs as UID 65534 with all
outer capabilities dropped, no-new-privileges, no external network or published
ports, private IPC and 128 MiB of shared memory. Its private ext4 data directory
is `/test-data/lab`, owned by UID 65534 with mode 0700.

The combined browser and test process budget is one CPU pinned to one logical CPU,
1 GiB of memory including swap, and 256 threads and processes. The browser must
report PID and network namespace sandboxes and Seccomp-BPF as active. Its
container seccomp profile permits namespace setup and the `chroot` syscall;
the kernel must still reject `chroot` outside the browser's own user namespace.
Disabling the browser sandbox or granting outer capabilities does not satisfy
this laboratory contract.

The HTTPS fixture binds only to loopback inside that container. Its disposable
certificate is accepted using an exact SPKI pin in the private browser, and
`vivac.test` is resolved through a browser-local rule. No host certificate store,
DNS, hosts file, browser profile or operating-system authenticator is configured.
Extracted Ubuntu fonts use a Fontconfig file and cache inside the private data
directory, without configuring the container's or host's system font paths.
The virtual authenticator, browser session, TLS fixture and temporary profile
are torn down after each case. Container cleanup remains the supervisor's duty.

Within that prepared isolated environment, set
`VIVAC_BROWSER_LAB=isolated-v1` and select the cases with:

```sh
cargo test --locked --offline --manifest-path tools/team-coordinator-probe/Cargo.toml --features browser-lab registration::browser_tests:: -- --test-threads=1
```

This remains a registration test. It grants no membership or signing authority,
performs no final assertion and proves no physical authenticator or human consent.
