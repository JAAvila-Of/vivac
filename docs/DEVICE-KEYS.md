# Local device signing keys

Windows uses current-user DPAPI. macOS uses a new private, passphrase-protected
file-based keychain. Both create an Ed25519 credential for future device
enrollment. Neither creates
a person, organization, membership, permission or content decryption key.

## Commands

`vivac device-key create <absolute-new-directory> [--json]` creates a complete
private key package. Its parent must already exist; the destination must not.
`vivac device-key show <absolute-directory> [--json]` reads an existing package
without writing it and returns its public identity. Both commands operate
without a project tree. There is no default store, global registration or
harness setup. Destinations inside Git working trees or with a `.git` or
`.vivac` path component are refused, including a new final component. Existing
destinations are never replaced.

On macOS, creation requires an existing private parent directory and a
controlling terminal. It asks for a new passphrase and confirmation, with
terminal echo disabled by the native password reader. Passphrases must be
valid UTF-8, contain 16 to 512 bytes and contain no NUL. They are not accepted
through arguments, environment variables, stdin or files. They are not an
organizational content key or proof of human approval.

`vivac device-key lock <absolute-directory> [--json]` locks a macOS store.
`vivac device-key unlock <absolute-directory> [--json]` unlocks it using a
terminal passphrase if it is locked. An already unlocked store is validated
without prompting; this is availability, not fresh authentication. These
commands return `status` (`locked` or `unlocked`) and `protection` in JSON.
Their reported state is not a lease: Keychain Services may change it later.
`show` and `prove` never prompt, unlock or repair a store.

### Windows paths

This backend accepts conventional drive-letter paths on identified local
fixed, removable or RAM disks with persistent ACL support. Remote, unknown,
UNC, device-namespace and unverified drive aliases are refused before opening
credential files. Windows volume classification is part of this validation.

The JSON receipt contains only `device_id`, `credential_id`, `public_key`,
`protection` and `status`. The public key is 32 bytes encoded as 64 lowercase
hexadecimal characters. Protection is `windows_current_user_dpapi` or
`macos_classic_keychain_passphrase`; status is
`local_key_only`: these commands do not check enrollment with any authority.
It is not a global claim that the key has never been registered. Device and
credential identifiers are independent opaque
128-bit random values encoded in the existing canonical 26-character
Crockford alphabet. They are not a person's identity or a lane.

Creating another package creates another credential; it does not enroll a
second device or create another user. Enrollment must later bind an exact
public key and these identifiers to a principal under current authority.

The [enrollment-proof commands](DEVICE-ENROLLMENT-PROOFS.md) sign a bounded
challenge with a protected key on Windows or macOS and verify the public
result on Linux, macOS or Windows. Proof verification is common; it does not
supply the missing Linux custody/signing backend. That backend remains a
prerequisite for a complete multiplatform team flow.

## Protection and bounds

The signing seed and identifiers use the operating system's cryptographic
random source. Failure refuses the operation; the local work-ID generator's
deterministic fallback is not used. Plaintext secret buffers are zeroized on
normal scope exit, including errors. Application file writes contain only
encrypted credentials; secret bytes are not printed. This does not promise
protection against memory dumps, debuggers, a compromised endpoint or all
compiler/runtime copies.

On Windows, the stored file is `device-key.bin`, inside a newly prepared private directory.
It contains a versioned wrapper and a DPAPI-protected payload. The wrapper is
ASCII `vivac.dpapi-key.v1` followed by a zero byte, then a four-byte big-endian
protected-blob length and exactly that many bytes. The complete file is at most
16 KiB. The plaintext payload is exactly ASCII `vivac.device-key.v1` followed
by a zero byte, a 16-byte device identifier, a 16-byte credential identifier
and a 32-byte Ed25519 seed. Its total length is 84 bytes.
The wrapper magic is also supplied as DPAPI optional entropy to separate this
use from other protected data. Parsing requires the expected wrapper,
declared length, payload size and payload version. DPAPI owns verification of
its opaque blob; the format does not promise a canonical encoding for that
provider-owned structure.

Creation writes only encrypted bytes under owner-only permissions, flushes the
file and prepared directory, and publishes the complete directory exclusively.
Loading refuses links, unexpected package members, oversized input, invalid
protection or nonprivate permissions. Directory and file identities are checked
through native handles. Windows publication closes child-file handles while
retaining the prepared directory handle; reopening must recover the original
file identity and exact encrypted bytes before success is acknowledged. Reads
exclude concurrent writers while the key bytes are inspected.
A failed preparation is removed only while it still
has the identity created by the operation. No existing package is repaired,
migrated or regenerated by `show`.

DPAPI uses current-user protection with UI forbidden, never machine-wide
protection. A process running as the same Windows user may decrypt the
credential. Endpoint administrators and organizational profile-recovery
authorities may be inside that trust boundary. This is neither a hardware
nonexportability guarantee nor fresh human verification. Profile roaming and
recovery mean the package is not proof of a physically unique machine.
See Microsoft's [DPAPI protection contract](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata).

Copying the package alone is not a supported cross-PC enrollment or recovery
procedure. Credential revocation must be admitted by the future shared
authority; deleting a file does not revoke an already enrolled key. Windows
credential custody remains separate from organizational content-key recovery.

## macOS keychain profile

The macOS backend creates its own keychain; it does not adopt an existing
keychain, search the login/default keychain, or change their search list.
Path components beginning with `login.keychain` are refused because native
creation can treat that name specially when updating the search list.
The private parent and store must belong to the current user, have no
group/other permissions or ACL entries, and use local APFS or HFS with ownership
and extended security enabled. Every path component is opened without
following links. Links, multiple hardlinks, unexpected members and changes
to retained file/directory identities are refused.

The directory contains `device.keychain` (or its native `-db` form, never both)
and `device-key.json`. The native database is limited to 1 MiB. The JSON
manifest is limited to 2 KiB and contains its format version, device and
credential identifiers, public key and SHA256 of the committed encrypted
database. It contains no seed or passphrase. The protected native item contains
the same 84-byte versioned payload described above, without the DPAPI wrapper.
Lookup fixes the keychain, service and account, requires one matching item,
and checks native item identity before and after retrieving its data.

Creation prepares the final private directory at a stable path. The manifest
is published atomically and exclusively only after the encrypted native store
has been created, flushed and verified. Until that commit, other commands
refuse the incomplete directory before accessing Keychain Services. Failed
creation cleans only an uncommitted directory that still has the identity
created by that operation; a crash may leave such a private preparation.
Errors after the commit preserve the completed store rather than deleting it.
No existing destination is overwritten or adopted.

The backend checks native lock state and disables framework dialogs. An
unavailable, locked, ambiguous, modified or corrupt store refuses instead of
falling back to another keychain. The encrypted database digest is checked
before and after native operations so a cached secret cannot hide a changed
persisted revision. The manifest is not an authorization token or a signed
statement from an enrollment authority.

This profile uses the classic file-based keychain, including public APIs Apple
has deprecated. It does not use Data Protection Keychain, iCloud synchronization
or Secure Enclave, and does not promise hardware nonexportability or fresh
human verification. Native per-application access control is preserved; vivac
does not broaden it to trust every application. A different executable may be
refused. External password/ACL changes, native file migration and changed
encrypted revisions are not silently accepted or repaired by these commands.
Keep the creating executable and passphrase available until a supported
credential rotation/recovery procedure is in place.

Linux refuses custody and signing before credential filesystem access.
Its ordinary local vivac functionality remains available. Future client
backends must establish their own protected-custody contract before activation.
The Linux coordinator does not need a participant's signing seed.

## Development isolation

Tests use the built development executable, generated test credentials and
explicit temporary destinations. They do not update an installed executable,
enroll real users, migrate real trees or alter harness configuration. Ordinary
capture and model-facing startup instructions do not load this credential store.

Redirecting `HOME` and `USERPROFILE` isolates application paths, not the Windows
security principal. Native DPAPI integration tests use the current Windows
token and provider. They do not create Credential Manager entries or change
login settings; they do not claim that the provider's internal profile or cache
handling is sandboxed.

Native macOS integration runs only through the explicitly enabled hosted-runner
test script, with its own temporary profiles and pseudo-terminal. Ordinary unit
tests do not create Keychain credentials. Redirecting `HOME` does not isolate
Keychain Services or the operating-system account. Native framework and terminal
behavior are tested on disposable runners, never against a maintainer's real
keychains. No credential files are uploaded as CI artifacts.
