# Linux OpenSSH secret and KDF policy

This is the selected creation contract for the candidate Ubuntu 24.04 backend,
not backend admission. The disposable writer's `--generated-package-create` mode
exercises generation and attended confirmation. Its historical modes still
accept caller-selected passwords for retained experiments. Neither admits
product custody, which stays unavailable until the complete flow is implemented
and verified with the native provider and production Rust launcher.

## Threat and ownership

The object is a device signing credential, not a content-decryption key or a
membership grant. The person retains its unlock secret; the persistent package
holds ciphertext; the owned native agent holds unlocked key material. The
coordinator receives neither the unlock secret nor private key.

An attacker may obtain a complete encrypted package and attempt offline guesses
without rate limits. File permissions, online throttling, agent lifetime and
revocation do not prevent guesses against that copy. They still govern their
respective access and lifecycle boundaries.

The OS random generator, endpoint administrator, selected native provider,
terminal and same-account processes are trusted. This does not protect against
a compromised endpoint, recorded terminal, keylogger or extraction from an
unlocked agent. Over SSH, the remote host and terminal path are trusted;
laptop-agent forwarding is absent. The
[profile's provider and erasure limits](LINUX-OPENSSH-PROFILE.md#native-protection-and-trust)
remain unchanged.

## New credentials require generated secrets

Creation must draw **32 fresh bytes from the OS cryptographic random generator**,
independently of the Ed25519 seed and KDF salt. Random-source failure refuses the
operation. Encode them as exactly 64 lowercase hexadecimal ASCII characters,
without truncation, normalization or a user-selected suffix. This lossless
encoding preserves 256 bits of input uncertainty under a uniform-source
assumption; it does not claim 256-bit security for Ed25519 or the whole system.

Under that assumption, independent of an attacker's information, at most `q`
distinct guesses cover at most `q / 2^256` of the input space, capped at one.
This is a conditional counting argument, not a measured attack speed or recovery
time. Length and character checks cannot establish a supplied secret's origin.

The initial production creation path therefore accepts neither a caller-chosen
password nor an imported experimental package. A manifest, policy flag or
matching hexadecimal shape cannot upgrade unknown secret-generation history.
Activation of an already admitted credential checks encoding and protected
identity; it does not infer randomness from the input's appearance.

The experimental terminal reader returns an opaque in-memory
`ConfirmedGeneratedSecret` only after successful generated-secret confirmation,
terminal restoration and the final foreground check. Its fields are private; it
has no public constructor, `Clone`, `Debug` or serialization implementation.
The shared [ciphertext builder](../src/device_keys/linux_creation_ciphertext.rs)
validates the credential comment before prompting, borrows the confirmed bytes
for encryption, and retains the confirmation with the encrypted key. The writer
adapter consumes that in-memory value to prepare the package on disk and keeps
the confirmation until the attended native handoff consumes it.
It drops that confirmation before publication; reopening requires fresh attended
input and cannot reuse the creation value.
Historical password-input modes cannot construct that value from their bytes.

The [generated input reader](../src/device_keys/linux_attended_creation_input.rs)
and [terminal implementation](../src/device_keys/linux_attended_terminal.rs)
are used by Vivac's Linux creation command, its integration test target and
the disposable writer. The private reopening path uses the same terminal
implementation for existing credentials. Callers must use a single-threaded
foreground process for the signal mask and terminal ownership contract.
The disposable writer also exposes preparation and publication checkpoints
for the isolated tests.

The separate experimental existing-secret reader returns an `AttendedUnlockInput`
after one foreground `/dev/tty` entry, checked terminal restoration and a final
foreground check within a single 15-second deadline. It accepts exactly the
64 lowercase hexadecimal ASCII characters of the admitted creation policy,
without normalizing the input or inferring its origin or entropy. The input is
opaque and zeroizing; it is not a generated-secret confirmation, evidence that
the key unlocked, admission authority or proof of an earlier session's exit.
It shares the existing terminal experiment's single-threaded and blockable-signal
boundary; it does not promise restoration after `SIGKILL` or `SIGSTOP`.
The disposable input probe drops it before emitting constant metadata and
neither opens a key nor activates a session. A future activation must establish
current admission, reservation, provider and independent prior closure before
requesting this input. The disposable successor fixture uses this same reader
after observing the exit of a registered predecessor that stopped before
unlock. It drops the zeroizing value after the private loader handoff. An
operational activation must not request the input as an IPC or MCP side effect.

This preserves a procedure boundary inside the trusted program. It is not a
persistent receipt, proof that the terminal was unrecorded, fixed-installation
binding, native-process closure or credential admission. Borrowing bytes for
encryption does not promise that trusted code or its libraries never copy them;
the existing memory-erasure limits still apply. No confirmation value is written
into the package or exported through CLI, MCP or an observation report.

Password change and package import are outside this initial profile. Replacing
the secret requires a separately created credential and explicit revocation and
enrollment under the applicable authority. Replacing an encrypted file alone
does not revoke a copied old signing credential.

## Attended delivery before publication

The creation interaction teaches the person which unique secret they must retain
to unlock the credential and why losing it requires replacing the credential.
Before generating or displaying it, require explicit attended creation and a
usable foreground controlling terminal. Never reveal it through MCP, an
unattended agent response or redirected output.

The disposable mode first presents that warning and requires the exact phrase
`CREATE IN TRUSTED TERMINAL` within 60 seconds. Only then does it generate the
secret. This acknowledgement is an explicit choice, not detection or attestation
that recording software is absent.

Deliver the secret only to that controlling terminal, once per creation attempt.
Require exact re-entry with echo disabled before publishing the package. Preserve
bounded terminal input, signal handling, foreground checks and restoration.
Use a single 180-second deadline from disclosure through confirmation, without
resetting it after input; the old experiment's 15-second entry window is not a
usability measurement of this new interaction.
Confirmation proves matching input, not that a password manager saved it.

Never place it in argv, environment, stdout, stderr, provenance, reports,
temporary files or ordinary backups. Do not automatically copy it to a clipboard,
manager or service. The person selects a trusted private place to retain it
separately from the package. Scrollback, screen sharing and session recording
can retain displayed secrets; screen clearing cannot promise erasure. Explain
this exposure before disclosure and require a trusted, unrecorded creation
terminal. The program cannot attest that no external recorder exists.

Random-source, terminal, confirmation or cancellation failure before commit must
leave no published credential. Preserve the package's existing commit semantics:
failure after the successful exclusive rename reports indeterminate durability,
not success or permission to delete the committed package. Do not automatically
retry publication or redisplay the same secret. Zeroize caller-owned buffers on
normal exit, including refusals, within the profile's existing erasure limits. Create no
recovery copy. A lost secret requires a new credential and revocation of the
inaccessible one; there is no server escrow or secret-reset route.

## Exact KDF policy

Keep Ed25519, AES-256-GCM, a fresh 16-byte random salt and **exactly 64
`bcrypt_pbkdf` iterations**. This counter is not bcrypt's logarithmic password-hash
cost. Native OpenSSH decrypts and signs; Rust only encodes the encrypted package.
Reject other ciphers, KDFs, salt lengths and iteration counts before native
loading. A parameter change requires a reviewed profile and new measurements,
never an adaptive fallback.

The generated secret supplies guessing resistance under the source assumption.
The KDF supplies compatible derivation and additional work per guess. Sixty-four
iterations do not make an arbitrary human password safe. The public random salt
separates derivations and supplies no secret entropy. This rationale relies on
neither bcrypt password-hash cost recommendations nor an unmeasured GPU rate.

The retained [work measurements](PERFORMANCE.md#experimental-openssh-bcrypt-work-on-ubuntu-2404--2026-11-12)
cover the existing 64-iteration fixture. The maintainer accepted its creation cost
around 2.3 seconds while preserving the original target, tolerance and samples.
Activation and unlocked signing have separate observations. The
[integrated release runs](PERFORMANCE.md#integrated-linux-binary-with-hardware-acceleration--2026-11-21)
record creation, attended input and the complete verification process as
separate intervals. The 50 ms target applies to unlocked signing and
verification in its original fixture. The integrated verifier also starts a
process and reads the retained admission. Size, expansion, memory and work
bounds remain hard guards.

## Required implementation evidence

Demonstrate random-source failure refusal, lossless encoding and independent
draws; exclusive delivery to the foreground controlling terminal; and refusal
without disclosure when it is unavailable. Exercise mismatched confirmation,
EOF, timeout, signals and terminal replacement without a published package before
commit. Faults after commit must retain the package and report indeterminate
durability, consistent with the existing publication contract.

Use disposable fixtures to check that secrets never reach inherited arguments
or environment, captured stdout/stderr, reports or auxiliary files. Retain no
secret values or their hashes in evidence. Expected bytes may exist transiently
in an isolated test observer, never its report. Test native unlock and exact
protected-identity validation through the safe Rust launcher, including
concurrency, cancellation and observed child exit. Existing package, provider,
filesystem and agent lifecycle gates still apply.

## Sources and limits

OpenSSH's [versioned key format](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/PROTOCOL.key)
defines salt and iteration fields. Its
[versioned KDF implementation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/openbsd-compat/bcrypt_pbkdf.c)
defines the derivation loop. These establish compatibility, not vendor approval
of this product policy.

NIST's informative [Strength of Passwords appendix](https://pages.nist.gov/800-63-4/sp800-63b/passwords/)
distinguishes offline guessing from rate-limited authentication and explains the
difficulty of estimating uncertainty in human-selected passwords. It prescribes
neither this 32-byte secret nor OpenSSH bcrypt64. The generated-secret
requirement and counting argument are this profile's choice for its stated
offline-copy threat.
