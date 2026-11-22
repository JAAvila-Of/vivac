# Linux key-protection options

The recommendation is to accredit an OpenSSH-based profile next: a new key
used only by vivac, an explicitly encrypted private-key file and an owned
private agent while unlocked. This is a candidate selection, not an available
backend. Linux custody and signing remain disabled until the complete profile
passes its acceptance cases.

There is no assumed credential facility shared by every Linux installation.
Package availability, installed components, kernel configuration, permissions
and a working service are separate facts. A product must check the required
capabilities instead of inferring them from a distribution name or SSH login.
Ordinary local capture and public proof verification need none of these stores.

## Mechanism comparison

| Mechanism | Coverage and extra setup | Persistent protection and key holder | SSH and limits | Disposition |
|---|---|---|---|---|
| Secret Service | Shared desktop interface; requires a running implementation and D-Bus. Not a baseline for a server without a desktop. | Interface does not require a master password or guarantee the provider's encrypted-at-rest format. Holder and recovery depend on that provider. | A desktop login session cannot be assumed over SSH; provider prompts may require GUI. | Optional only after a specific provider/profile is accredited. No generic fallback. |
| Kernel user/session/persistent keyrings | Requires enabled kernel key support and permitted syscalls; containers may restrict access. No desktop daemon is required. | Ordinary key retention caches data in kernel memory. A persistent keyring can outlive a login, not a reboot. It does not by itself supply encrypted disk storage. | Useful for transient availability, but does not solve the complete protected-store lifecycle. | Not a complete signing-store backend by itself. |
| Kernel trusted/encrypted keys | Additional configured key types; trusted keys need a supported trust source, such as TPM or TEE. | Encrypted blobs can persist, but an encrypted key still needs a master key; hardware-rooted and user-key-rooted protection have different guarantees. | Availability, permissions and userspace access/signing capabilities need separate proof. The kernel interface is not a generic Ed25519 signing agent. | Potential hardened profile, not a universal baseline. |
| systemd credentials | Encryption/decryption since systemd 250; normal user-scoped operation since 256 needs the system credential broker. Does not require a graphical desktop. | AES-256-GCM envelope, with host key, TPM2 or both. Host-only master material belongs to the root-controlled OS installation; it is not a personal passphrase. | Suitable for some managed hosts/services. User-scoped availability and policy must be checked; successful auto mode does not prove TPM use. No foreground personal unlock lifecycle is provided by the envelope alone. | Optional managed-host candidate; insufficient coverage as the only Linux backend. |
| OpenSSH client tools and private agent | Client package is distributed across the surveyed Ubuntu/Debian and Alpine families. Agent/keygen may be split into packages. Neither client tools nor agent availability is guaranteed on a minimal host. | New vivac-only private key, explicit authenticated encryption, reviewed bcrypt work factor and human passphrase. Agent holds the unlocked key locally. | CLI operation and an explicitly selected Unix socket support headless use without a desktop or systemd broker. Needs owned process lifecycle and actual protocol/format tests. | Preferred next accreditation candidate; do not reuse login keys or ambient agent. |
| Integrated encrypted store using a maintained library | No external credential service if compiled into vivac; adds a library graph and a supported format. | Human passphrase or another explicitly defined wrapping authority must protect an authenticated envelope with reviewed KDF and bounds. | Can support headless use, but vivac then owns unlock/session behavior, recovery and memory lifecycle. | Compare as a separate design. Current pillars require an OS protection backend; do not adopt this silently as a fallback. |
| Dedicated oo7 daemon | Additional implementation whose availability/version cannot be assumed. | Different new and legacy formats; existing experiments describe its limits, not production acceptance. | Requires its own bus/profile/PAM socket isolation and explicit lifecycle. | Research remains reproducible but is not a runtime requirement or default choice. |

## Distribution and environment coverage

This is a source/package survey dated 2026-11-11, not installation testing. Package versions
are observations of the official repositories at review time, not minimum
supported product versions or proof of default installation.

| Surveyed environment | OpenSSH client family | systemd user credential route | Consequence |
|---|---|---|---|
| Ubuntu 24.04 | Official client package is based on OpenSSH 9.6p1. | Official systemd family is 255.4: it has encrypt/decrypt but predates the user-scoped broker route. | Client-tool capabilities can be checked without requiring a systemd upgrade. |
| Ubuntu 26.04 | Official client package is available; its installed capabilities still need checking. | Official systemd family is 259: supports the route if the broker and policy are usable. | Both candidates need runtime checks; OS version alone is insufficient. |
| Debian 13 | Official client package is based on OpenSSH 10.0p1 and includes agent, add and keygen utilities. | Official systemd family is 257: supports the route subject to broker/policy. | OpenSSH does not require starting the privileged credential broker. |
| Fedora 45 | Check the installed OpenSSH components; package names and policy are distribution-specific. | Official systemd family is 262: supports the route subject to broker/policy. | A managed systemd profile is possible, not an assumed personal store. |
| Alpine or another non-systemd environment | Alpine publishes OpenSSH client/common/keygen packages separately. Check the complete required set. | The normal no-root broker route is not supplied merely by another init system. | A client-tool profile avoids making systemd a product prerequisite. |
| Minimal container | No agent, desktop service, broker, allowed keyctl syscall or persistent storage is assumed. | A binary alone does not supply its service, host key or trust policy. | Either provision a supported profile explicitly or report custody unavailable; never create plaintext fallback. |

The systemd own-user route in the reviewed versions runs through a privileged
system broker. It can work without a GUI, PAM or a user service manager, but it
still trusts that broker and the endpoint administrator. Host-only protection
can be decrypted with the host secret and sufficient privileges. TPM binding
changes protection of an offline copy; it does not promise privacy from the
administrator of a running authorized endpoint. A synchronization coordinator
must still never receive a participant's signing seed or content keys.

## Why OpenSSH is the next candidate

OpenSSH reduces dependence on a particular desktop or secrets-service provider
and fits the command-line/SSH environment of the pilot. This is a portability
judgement from available packages and interfaces, not evidence that the backend
already works or that every Linux installation has those tools.

The required profile is stricter than an arbitrary existing SSH key:

- Generate a new Ed25519 key only for vivac, outside login key directories and
  outside Git, the provenance tree, artifact custody and ordinary backups.
  Do not adopt, convert or copy a person's existing SSH authentication key.
- Select authenticated private-file encryption explicitly. OpenSSH 9.6p1's
  default is AES-256-CTR, not an AEAD mode. Its source supports authentication
  tags in the private-file path and AES-GCM ciphers; the actual installed
  keygen/add build must prove the selected combination end to end.
- Use a reviewed bounded bcrypt work factor and passphrase policy. Never pass
  the person's passphrase through command arguments, environment or caller
  files. Native process/TTY handling must prove nonempty creation without
  transient plaintext-key publication or hidden graphical prompts.
- Start an owned foreground agent with a new private socket. Ignore ambient
  `SSH_AUTH_SOCK` and forwarded agents. Load only the exact committed vivac key;
  reject unexpected identities, comment/binding changes and stale instances.
- Bind device and credential identities to the protected key material and the
  expected public key. A filename or mutable public comment alone is not an
  enrollment identity. Prove the binding before publishing readiness.
- Use the agent's Ed25519 signing operation over the exact existing vivac
  challenge bytes. SSH signature framing must be decoded and checked; the
  general `ssh-keygen -Y sign` SSHSIG format is a different signed message and
  cannot be substituted without changing vivac's protocol.
- Verify the resulting raw signature with the existing verifier. Refuse wrong
  algorithms, extra bytes, unsupported key types, expired challenges and
  responses not bound to the expected key. Vivac exposes no arbitrary-message
  signing interface.
- Close signing admission and confirm owned-agent shutdown on lock, hangup or
  failure. Test real SSH disconnects separately from PTY behavior. Do not kill
  another agent, alter login services or install software automatically.

The operating-system account and endpoint administrator remain inside the
trust boundary. Agent availability is not fresh human verification, device
enrollment or a grant of organizational permissions. The private key is still
software-protected; no hardware nonexportability claim is made.

Before implementation, accredit capability detection and executable provenance,
complete package schema and integrity, key/identity binding, secret input,
resource and KDF limits, exact signature interoperability and process/socket
isolation using disposable credentials. Failure leaves custody unavailable
while ordinary local work remains usable. No dependency or MSRV change is
approved merely by this recommendation.

The [disposable OpenSSH probe](../tools/native-key-store-probe/review/OPENSSH.md)
has since demonstrated signature interoperability with the shared verifier,
but also reproduced plaintext creation after EOF/no TTY or empty passphrases,
despite an explicitly requested cipher. The direct keygen creation route is
therefore not accredited. The profile remains a candidate until creation fails
safely, identity binding and package guards are complete, and lifecycle cases
including actual SSH disconnects pass. Existing login keys must not be used as
a workaround.

The subsequent isolated encoder and terminal experiments demonstrate an
always-encrypted encoding path and bounded PTY/SSH input for synthetic keys.
They do not approve the complete backend. The [OpenSSH profile contract](LINUX-OPENSSH-PROFILE.md)
fixes the required native-provider boundary, provider-specific identity layout
and package publication/verification gates.

## Portable encrypted-file alternative

A maintained file-encryption implementation could avoid a separate runtime
provider. For example, the age format specifies authenticated encryption and a
passphrase recipient using scrypt, and a Rust implementation exposes that API.
This does not mean the implementation, graph, work bounds or version is approved.
The consulted Rust implementation also labels its pre-1.0 versions beta.

This option would require an explicit doctrine review before implementation:
the current [pillars](PILLARS.md) require a supported OS protection backend for
device credentials. Removing an external service is not authority to waive that
rule, hand-roll cryptography, keep a wrapping password on disk or add plaintext
fallback. The comparison retains this alternative rather than assuming either
that a native daemon is always necessary or that a library is automatically safer.

## Primary sources

- [Secret Service contract and excluded guarantees](https://specifications.freedesktop.org/secret-service/latest-single/)
- [Kernel key retention](https://docs.kernel.org/security/keys/core.html)
- [Persistent keyrings](https://docs.kernel.org/security/keys/persistent-keyring.html)
- [Kernel trusted/encrypted keys and master-key requirements](https://docs.kernel.org/security/keys/trusted-encrypted.html)
- [systemd-creds 257 manual](https://www.freedesktop.org/software/systemd/man/257/systemd-creds.html)
- [systemd credential model](https://systemd.io/CREDENTIALS/)
- [systemd 257 user credential service implementation](https://github.com/systemd/systemd/blob/v257/src/creds/creds.c)
- [Ubuntu 24.04 systemd packages](https://packages.ubuntu.com/noble-updates/systemd)
- [Ubuntu 26.04 systemd packages](https://packages.ubuntu.com/resolute/systemd)
- [Debian 13 systemd packages](https://packages.debian.org/trixie/systemd)
- [Fedora systemd packages](https://packages.fedoraproject.org/pkgs/systemd/systemd/fedora-45.html)
- [Ubuntu 24.04 OpenSSH client package](https://packages.ubuntu.com/noble/openssh-client)
- [Ubuntu 26.04 OpenSSH client package](https://packages.ubuntu.com/resolute/openssh-client)
- [Debian 13 OpenSSH client package and included utilities](https://packages.debian.org/trixie/openssh-client)
- [Alpine OpenSSH client package dependencies](https://pkgs.alpinelinux.org/package/v3.22/main/x86_64/openssh-client-default)
- [ssh-keygen manual](https://man.openbsd.org/ssh-keygen)
- [ssh-agent manual and endpoint trust boundary](https://man.openbsd.org/ssh-agent)
- [OpenSSH 9.6p1 private-file implementation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/sshkey.c)
- [OpenSSH 9.6p1 ciphers](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/cipher.c)
- [OpenSSH Ed25519 signing implementation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-ed25519.c)
- [age file format](https://age-encryption.org/v1)
- [Rust age implementation](https://docs.rs/age/0.12.1/age/)
