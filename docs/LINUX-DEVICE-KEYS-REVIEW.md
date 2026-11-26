# Linux signing-store provider review

This review rejects the initial GNOME Keyring legacy-format selection for new
vivac signing credentials. It does not establish an exploit, a compromised
credential or a vulnerability in every use of GNOME Keyring. Linux custody and
signing remain unavailable. The [target contract](LINUX-DEVICE-KEYS-DESIGN.md)
retains explicit foreground unlocking and private per-store isolation without
approving a replacement provider.

## GNOME Keyring legacy format

The review fixes upstream 46.1 at commit
`4e173494bf15795a1ebab6e2bbd9377fac456240`. The relevant functions are
`encrypt_buffer`, `verify_decrypted_buffer`, `gkm_secret_binary_write` and
`egg_symkey_generate_simple` in the two primary sources below.

| Property | Observed implementation |
|---|---|
| Encryption | AES-128-CBC; key and IV derived from the passphrase. |
| Derivation | Repeated SHA-256, rather than PBKDF2 or a memory-hard KDF. |
| Work factor | Newly written files choose 1,000 through 4,095 iterations. |
| Salt | Eight bytes. |
| Internal check | An encrypted MD5 checksum of the plaintext block, rather than a MAC or AEAD tag. |
| Outer structure | Some header and metadata remain outside the encrypted block. |

The reviewed functions also matched upstream main at the time of this review.
The official Ubuntu noble-updates offer was 46.1-2ubuntu0.2; inspection of that
package's source patch set found no changes to these two files. This identifies
an offered package, not the binary installed in any past CI run. Future native
evidence must record its actual package version and executable provenance.

These observations do not mean AES-128 is broken. They do mean that functional
round trips and a nonempty human passphrase are insufficient grounds to admit
this new signing-store profile: the derivation permits inexpensive offline
password trials, while the internal checksum is not authenticated protection
of the complete package. Passphrase length alone does not establish entropy.
A public SHA-256 manifest can detect accidental changes but cannot supply an
independent authenticated authority when both data and manifest can be changed.

The existing isolated probe remains useful evidence for native persistence,
selection and locking behavior. Its encrypted-header check and DH transport
cannot answer the missing at-rest protection questions. No runtime round trip
can change the reviewed format's derivation or authentication design. The
provider is therefore rejected for this new profile rather than admitted with
an implicit cryptographic exception. No provider-specific SSH implementation
is accredited on the strength of the previous fixture.

Primary sources:

- [Fixed binary format implementation](https://github.com/GNOME/gnome-keyring/blob/4e173494bf15795a1ebab6e2bbd9377fac456240/pkcs11/secret-store/gkm-secret-binary.c)
- [Fixed derivation implementation](https://github.com/GNOME/gnome-keyring/blob/4e173494bf15795a1ebab6e2bbd9377fac456240/egg/egg-symkey.c)
- [Official Ubuntu package offer](https://packages.ubuntu.com/noble-updates/gnome-keyring)
- [Official distribution source patch archive](https://archive.ubuntu.com/ubuntu/pool/main/g/gnome-keyring/gnome-keyring_46.1-2ubuntu0.2.debian.tar.xz)

## Replacement investigation: oo7 0.7.1

oo7 is a candidate for investigation, not an approved dependency or backend.
This review fixes tag 0.7.1 at commit
`4251a43abce2a0633fa35cdf3cd360af1fa88bc0` and distinguishes its new format from
its legacy GNOME reader. The presence of legacy code does not identify which
format a newly created credential actually uses.

Its new file API declares a 32-byte salt and 100,000 default/minimum iterations.
The native cryptography implementation derives keys with PBKDF2-HMAC-SHA256;
encrypted items use AES-128-CBC with an HMAC-SHA256 checked over ciphertext and
IV before decryption. This is evidence of a different design, not a completed
security review: assess password policy, work-factor suitability, key reuse,
complete metadata authentication and malformed-input bounds before approval.
Do not infer complete file authenticity from an authenticated item.

The workspace declares Rust 1.95, above vivac's Rust 1.89 minimum. Linking the
current client library into vivac is therefore not compatible with the current
toolchain contract. A separately built native daemon may use its own toolchain;
that does not by itself require raising vivac's MSRV or approve its client,
distribution method or runtime dependencies.

The daemon's `--login` path accepts a secret on its stdin and otherwise supports
ambient PAM/systemd credential discovery. A possible private fixture must use
only its owned pipe and explicit profile/bus, forbid ambient credential
discovery, replacement of another daemon and automatic migration of existing
collections, and prove missing-store refusal during unlock. The startup code
alone does not prove those properties. A passing fixture must identify the
new format and reject unencrypted or legacy fallback explicitly.

Before selecting this candidate, review the exact dependency graph, notices,
advisories and provider build separately. Then establish its file schema and
aggregate bounds, exact item/collection binding, persistence without unintended
revision changes, prompt refusal and owned process cleanup. Only after those
gates pass should the isolated SSH foreground/disconnect cases be run against
that provider. No build, credential operation or SSH lifecycle pass is claimed
by this source review.

Primary sources, fixed to the reviewed commit:

- [Workspace version and compiler requirement](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/Cargo.toml)
- [New file format and derivation parameters](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/client/src/file/api/mod.rs)
- [Native cryptography implementation](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/client/src/crypto/native.rs)
- [Encrypted item verification](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/client/src/file/api/encrypted_item.rs)
- [Daemon startup and credential sources](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/server/src/main.rs)

## Prompt and bus adapter review

The exact `secret-service` 6.0.0-rc.1 source executes provider Prompt from its
high-level lock/unlock helper and some create/delete operations. Its
`Error::Prompt` can describe dismissal after the dialog has already run; catching
that error does not prevent a graphical interaction. The existing read/create-item
probe paths are not evidence that its lock wrapper is prompt-free.

The crate keeps its generated proxy module private. It does expose
`connect_with_existing`, allowing a client to supply an explicit connection;
`zbus` 6.2.1 can construct that connection from a literal Unix bus address.
An adapter can instead inspect direct D-Bus replies and refuse any Prompt path
without invoking Prompt. The private bus must also forbid service activation:
an explicit address alone does not prove that the service was already present.
These are source-level capabilities, not a compiled or runtime accreditation.

Exact source artifacts:

- [secret-service 6.0.0-rc.1](https://crates.io/api/v1/crates/secret-service/6.0.0-rc.1/download)
- [zbus 6.2.1](https://crates.io/api/v1/crates/zbus/6.2.1/download)

## Reproduced format boundaries and startup requirements

The [memory-only review runner](../tools/native-key-store-probe/review/README.md)
reproduces five specific format observations against the fixed oo7 source.
It builds the upstream library and test module independently of vivac, using
Rust 1.95 on Linux. Its fixtures contain no device signing keys. Passing those
observations does not approve the provider or accredit native daemon behavior.

| Observation | Consequence for vivac |
|---|---|
| An empty container validates unrelated passwords. | Require an existing committed package and the exact protected item before acknowledging readiness. |
| Deleting all items preserves parse/password success while losing the expected item. | Check credential presence and payload binding, not just native unlock success. |
| Deleting external attribute hashes preserves decryption of the authenticated item. | Verify the complete expected attribute set; do not infer authenticated outer selection metadata. |
| Encrypted blobs shorter than the MAC cause a caught panic rather than an error result. | Bound malformed input and isolate provider failures; a crashed child is not successful unlock or lock. |
| The maximum u32 iteration count passes the minimum-strength check without deriving. | Establish a maximum work factor before any derivation; a lower-bound check is not a work budget. |

These cases do not show an ability to decrypt or forge the expected signing
key. Container edits can instead remove availability or alter selection. The
adapter's exact single-item check and protected identity binding are part of
the required defense. Reordering alone is not relevant to a store whose only
permitted credential is that one exact item. General rollback protection is
not established by item authentication or a mutable manifest digest.

Static startup review also found that an isolated bus/profile and a clean
environment do not fully isolate the daemon. `Service::run` always starts its
PAM listener; without `OO7_PAM_SOCKET`, the listener uses
`/run/user/{uid}/oo7-pam.sock` and removes an existing socket before binding.
Any future fixture must explicitly set this path inside its private runtime
directory. Use an absolute private `XDG_DATA_HOME`, explicit private D-Bus
address and `--login` with an owned pipe to avoid ambient PAM login-helper or
systemd-credential discovery. Do not use `--replace`.

A private profile also needs validation before startup: a missing store can
produce an empty in-memory Login collection, and a legacy file can enter the
daemon's migration flow. Neither is acceptable during vivac unlock. Native
shutdown must verify owned-process exit and clean only owned runtime objects;
source-level exit on bus closure is not an SSH disconnect test.

Additional fixed primary sources:

- [Service initialization and migration](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/server/src/service/mod.rs)
- [PAM listener path and startup](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/server/src/pam_listener/mod.rs)
- [File load and missing-store behavior](https://github.com/linux-credentials/oo7/blob/4251a43abce2a0633fa35cdf3cd360af1fa88bc0/client/src/file/locked_keyring.rs)
