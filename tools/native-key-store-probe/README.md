# Native device key-store probe

This standalone development package exercises candidate native stores before
they are connected to `vivac device-key`. It has its own dependency graph and
lockfile, is not linked into the product and is excluded from its published
package. A successful probe does not enable Linux/macOS device enrollment,
protected signing commands, shared authority or human approval.

## What it checks

The probe generates a synthetic signing seed from operating-system randomness,
stores a versioned payload through the native provider and retains only its
public key in the temporary application files. A separate process retrieves
the payload, signs a fixed probe message and verifies against that public
receipt. A changed message, changed key or malformed payload must refuse.
No arbitrary-message signing interface is exposed.

Linux uses the exact `secret-service` 6.0.0-rc.1 candidate with Rust cryptography and default-runtime
features, explicitly negotiating DH. `zbus` is used directly only to identify
the expected missing-service error, rather than crediting any connection
failure. The runner starts a private bus with no service activation, a
separate profile and GNOME Keyring with a nonempty fixture password supplied
on stdin. It verifies retrieval after restarting its daemon, an encrypted
GNOME file header, a wrong account, a locked collection, malformed data and
ambiguous matching items. Each native operation has an external deadline;
a timeout is a failure, never an expected refusal.

macOS uses `security-framework` 3.7.0 and explicitly selected temporary
file-based keychains. Reads never search the default/login keychain. It
checks process-to-process persistence, duplicate-add refusal, account and
keychain isolation, native inactivity locking and malformed data. Interactive
Keychain UI is disabled for the probe process. The runner and executable
refuse outside a disposable GitHub-hosted macOS runner; changing `HOME` does
not isolate Keychain Services. The CI guard prevents accidental local use;
environment variables are not a security boundary against a malicious user.

The fixture password is public test material. None of these credentials are
suitable for production use. Native framework, provider and allocation
internals may retain copies; this probe does not establish complete memory
erasure or non-exportable hardware keys.

## Isolated execution

The dedicated `native-device-keys` workflow builds this package with its
lockfile, checks formatting and Clippy, runs common tests, then launches its
native runner on Linux and macOS. A separate matrix checks Rust 1.89.
It runs for probe/workflow changes or explicit workflow dispatch, rather than
adding provider startup to normal capture, hooks or every unrelated CI change.

From this package directory on an appropriate disposable runner:

```text
cargo build --locked --release
python3 run_linux.py target/release/native-key-store-probe
python3 run_macos.py --allow-hosted-keychain-probe target/release/native-key-store-probe
```

Select the runner for that operating system. Linux requires `dbus-daemon` and
`gnome-keyring-daemon`; macOS requires Python 3.12 or later. Do not point the
probe at an existing profile, bus, keychain or device-key directory. The runners
create and clean their own temporary roots and terminate only their own child
processes. On this shared Windows account, all Cargo and Rust runtime steps
must use the workspace's RustWork coordinator with their original arguments.

## What remains to establish

The Linux library is a prerelease candidate confined to this probe. Its
predecessor's dependency graph contains unmaintained `zvariant_derive`
(RUSTSEC-2026-0372); the candidate migrates to maintained zbus 6 instead of
suppressing that advisory. Passing these checks does not approve a prerelease
dependency for the production device-key backend.

Secret Service specifies an interface, not encrypted-at-rest storage or
persistence for every provider. GNOME Keyring can also operate with an
unencrypted keyring. The controlled nonempty-password fixture does not prove
that an arbitrary user's existing collection meets the product requirement.
Attributes are metadata, not secret storage. SSH does not imply that a usable
login-session provider exists. A product backend still needs an explicit
provider/protection contract and refusal when that contract cannot be verified.

The library may run a provider prompt internally. The isolated Linux fixture
has no GUI or service activation and credits no timeout as success; this does
not establish a prompt-free product path. The macOS file-based keychain is a
legacy implementation and does not accredit the Data Protection Keychain,
entitlements, Secure Enclave, Apple CryptoKit or a production signing policy.
Native tests must actually pass on their operating systems before their
behavior can be claimed. Application-package safety, credential revocation,
durable admission and fresh human verification remain separate requirements.

## Primary references

- [Secret Service specification](https://specifications.freedesktop.org/secret-service/latest-single/)
- [secret-service 6.0.0-rc.1](https://docs.rs/secret-service/6.0.0-rc.1/secret_service/)
- [Dependency migration advisory](https://rustsec.org/advisories/RUSTSEC-2026-0372.html)
- [GNOME encrypted file format](https://github.com/GNOME/gnome-keyring/blob/main/pkcs11/secret-store/gkm-secret-binary.c)
- [security-framework 3.7.0](https://docs.rs/security-framework/3.7.0/security_framework/)
- [Apple keychain implementations](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains)
- [Product device-key contract](../../docs/DEVICE-KEYS.md)
