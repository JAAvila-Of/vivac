# OpenSSH interoperability and creation observations

This development-only probe uses synthetic Ed25519 credentials in a disposable
Linux environment. It never enrolls a device, changes a login key, touches an
ambient/forwarded agent or enables vivac custody. A successful run can reproduce
an unsafe creation outcome; its exit code is not provider acceptance.

## Observed result

The Ubuntu 24.04 container run used OpenSSH 9.6p1-3ubuntu13.20 and Python 3.12.
It selected AES-256-GCM private-file encryption and bcrypt with 64 experimental
rounds. The positive path supplied a public fixture passphrase through an owned
PTY only after checking terminal echo was off. No passphrase entered arguments,
environment or a password file. The report contains executable hashes, public
protection labels, outcome flags and individual timings, not key material,
device identities, native output or captured terminal contents.

| Case | Result and scope |
|---|---|
| Encrypted file creation and load | Explicit GCM/bcrypt profile was generated, parsed and loaded into a new private foreground agent. This accredits interoperability of that happy path only. |
| Credential selection | The agent returned one expected public key and the exact opaque comment loaded with it. This is not organizational identity or an independent authenticated manifest. |
| Exact proof | The agent signed the canonical enrollment challenge. Its SSH signature envelope was checked and the 64 Ed25519 bytes passed the actual shared vivac proof verifier. |
| Altered/expired proofs | An altered challenge and a correctly signed expired challenge were rejected by the shared verifier. |
| Wrong password, damaged tag, no TTY and missing file | Loading was refused and the private agent stayed empty. No timeout was credited as refusal. |
| Owned-agent termination | The owned process exited and its socket disappeared after explicit termination. This does not establish supervisor-crash or SSH-disconnect behavior. |
| Creation with no TTY/EOF | Even with GCM requested, the command completed and wrote a key with cipher/KDF `none`. This is a creation safety gap. |
| Creation with two empty passphrases | The command also wrote a key with cipher/KDF `none`. This is a creation safety gap. |
| Forced askpass helper exits 1 | Keygen exited 0 and wrote a key with cipher/KDF `none`. Helper failure did not refuse creation. |
| Forced askpass helper is absent | Keygen also exited 0 and wrote a key with cipher/KDF `none`. Missing helper did not refuse creation. |

The report distinguishes `interoperability_passed: true`,
`creation_fail_closed: false` and `custody_accredited: false`. It also declares
that supervisor-crash and actual SSH-disconnect cases were not tested. The
plaintext negative fixtures are deliberate synthetic observations inside
disposable storage; this is never an allowed product provisioning path.

The single create/load timing samples are not p99 measurements, an accepted
KDF policy or evidence for every OpenSSH build. Sixty-four rounds is an
experimental input bounded by this probe, not a recommended production value.

The separate encoder run used the same Ubuntu image and native tools. Its
library-created GCM/bcrypt file loaded in the private agent, retained the exact
opaque comment and public key, and produced a signature accepted by the shared
verifier. Altered/expired proofs, a wrong passphrase and a damaged GCM tag were
refused. Invalid fixture inputs also refused before destination creation. This
establishes native-format interoperability and the tested input-validation
boundary; it does not establish the terminal, memory, package or OS-backend
guarantees below. The local writer tests passed on Rust 1.95/Linux and Rust
1.89/Windows; the latter compiles the encoder and tests validation but does not
exercise Linux file publication or native OpenSSH. Subsequent
[package and terminal runs](PACKAGE.md) rebuilt and exercised the Linux helpers
on Rust 1.89; remote CI remains a separate check.

## Why the creation gap matters

In the reviewed native code, missing/error input may become an empty string.
Two empty strings pass keygen's confirmation. The private-file writer selects
cipher/KDF `none` when the passphrase is empty, overriding a requested cipher.
Disabling askpass and inspecting the file after creation cannot prove that no
plaintext was ever written. Deleting the file afterward cannot meet that
requirement either.

The successful agent signature is still useful: it confirms a common native
signing interface can produce the existing vivac proof format. It does not
approve this key-creation sequence, the complete package schema, protected
device/credential binding, rollback behavior or a production session lifecycle.
Do not pass the passphrase with keygen's `-N` or `-P` to work around the gap.

Forcing an askpass helper is not a fail-closed replacement for these input
paths. Both cases above were reproduced without a TTY in the same disposable
Ubuntu image, using `SSH_ASKPASS_REQUIRE=force`. The helper fixture contains
only an exit statement; it never reads or stores a passphrase. In the reviewed
OpenSSH 9.6p1 source, helper failure returns `NULL`, but
the caller converts it to an empty string unless `RP_ALLOW_EOF` is set.
Keygen's confirmation uses `RP_ALLOW_STDIN` without `RP_ALLOW_EOF`; two helper
failures can therefore compare equal and reach the private-file writer.
See [readpass.c](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/readpass.c#L92-L179)
and [keygen confirmation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-keygen.c#L2904-L2938).

## Native-format writer boundary

A maintained OpenSSH-format encoder is a separate creation candidate. The
reviewed [`ssh-key` 0.6.7 source](https://github.com/RustCrypto/SSH/blob/6ae2c0850c0febb485fa53678cd622aa0e5f3db5/ssh-key/src/private.rs)
exposes `encrypt_with` with an explicit cipher, KDF and check integer. Its
[`Kdf::Bcrypt`](https://github.com/RustCrypto/SSH/blob/6ae2c0850c0febb485fa53678cd622aa0e5f3db5/ssh-key/src/kdf.rs)
accepts explicit salt and rounds. This would allow complete encrypted encoding
in memory before opening a destination, with native OpenSSH still responsible
for loading, holding the unlocked key and signing. The default encryption
method does not select this probe's GCM profile; defaults must not substitute
for explicit protection parameters.

This is source evidence, not approval of that dependency or of a Linux
backend. The implementation serializes plaintext into an ordinary temporary
`Vec` before encrypting it; errors after serialization do not guarantee that
buffer is wiped. `Zeroizing` on the returned encoded string cannot establish
that all intermediaries were erased. The dependency graph, exact native
interoperability, input handling and publication guarantees require separate
checks.

The minimal locked graph uses `ssh-key` without default features, with only
`std` and `encryption`, plus the existing shared Ed25519 implementation and
explicit entropy/zeroization dependencies. Its Linux dependency-policy check
passed the repository's advisory, license, source and ban policy; permitted
duplicate-version warnings remain. Declared MSRVs fit Rust 1.89, but metadata
alone cannot establish a successful build on that toolchain. Recheck the exact
lock with current advisories rather than treating this observation as permanent
approval:

```text
cargo deny --manifest-path tools/native-key-store-probe/review/openssh-writer/Cargo.toml --config deny.toml --target x86_64-unknown-linux-gnu --locked check
```

The [pillars](../../../docs/PILLARS.md#encryption) also require a supported
operating-system protection backend. A native agent supplies volatile custody
and signing, while a passphrase-encrypted file supplies persistent protection.
Calling the file format native does not settle whether that division meets the
requirement. An isolated encoder experiment neither changes that doctrine nor
enables custody in the product.
The [candidate profile contract](../../../docs/LINUX-OPENSSH-PROFILE.md) now
specifies the complete native-provider boundary: native OpenSSH must decrypt,
load, hold and sign; the library remains only its creation encoder. It also
specifies package publication and protected-identity checks that have to pass
independently before the profile can be enabled.

`ssh-agent -D` is not itself a supervisor-death guarantee and the reviewed
foreground code skips its ordinary core-limit setup. The probe sets
`RLIMIT_CORE=0` before each child exec and also runs the container with core
dumps disabled. Product lifecycle and memory guarantees still require their
own review and negative tests.

## Components and reproduction

`openssh_probe.py` is a bounded Python harness. It checks an explicit
`VIVAC_OPENSSH_DISPOSABLE=1` declaration to discourage accidental host use;
that environment variable is not a security boundary. Use it only in a
disposable container or runner, never with real credentials or mounted login
directories. It creates its own private temporary root, uses a reconstructed
child environment and an explicit private agent socket, and stops only its
retained child processes. Native calls have finite deadlines and output/IPC
bounds. Unexpected errors or timeouts fail the run.

`openssh-verifier` is a separate, unpublished Rust workspace. It includes the
actual product `src/device_keys/proof.rs` at compile time, with a minimal error
adapter. It adds no dependency to the product. Its input reader is limited to
8 KiB, and errors emit a generic refusal. This tests the shared proof format
and cryptographic checks, not the product CLI's filesystem or admission guards.

From the repository root in a disposable build environment:

```text
cargo build --locked --manifest-path tools/native-key-store-probe/review/openssh-verifier/Cargo.toml --target-dir /build/openssh-proof
```

Mount the resulting binary read-only into a disposable Linux runtime containing
OpenSSH client tools and Python 3.12. With the disposable declaration set:

```text
python3 openssh_probe.py --verifier /build/openssh-proof/debug/openssh-proof-verifier-probe --report /reports/new-observations.json
```

The separate `openssh-writer` workspace experiments with the reviewed encoder.
It pins its own dependency graph and adds nothing to the product graph. Build
and test it separately:

```text
cargo test --locked --manifest-path tools/native-key-store-probe/review/openssh-writer/Cargo.toml --target-dir /build/openssh-writer
cargo build --locked --manifest-path tools/native-key-store-probe/review/openssh-writer/Cargo.toml --target-dir /build/openssh-writer
python3 openssh_probe.py --verifier /build/openssh-proof/debug/openssh-proof-verifier-probe --writer /build/openssh-writer/debug/ssh-writer-graph-audit --report /reports/new-writer-observations.json
```

The optional writer receives public fixture input through a private child pipe.
It is not a product password-input interface and does not accredit controlling
terminal acquisition, echo handling or cancellation. Its fixed experimental
profile uses AES-256-GCM and bcrypt with a fresh 16-byte salt and 64 rounds.
The separate [Linux terminal experiment](TERMINAL.md) specifies the optional
`--terminal` route and the PTY/SSH checks it requires; the fixture-pipe results
above do not establish that route.
The harness checks invalid input refuses before creating a destination; the
positive path then uses the same native load, signature and shared-verifier
checks. The experimental writer uses exclusive creation and mode 0600, but
does not implement atomic package publication: a write or sync error may leave
a partial encrypted file. It does not accredit destination ancestry, manifest
binding or recovery from interrupted publication. The keygen negative cases
still run and still determine the report's
`creation_fail_closed` flag. Do not interpret that flag as approval or rejection
of a complete library-backed creation flow.

The report parent must exist and the destination must be new. `Dockerfile.openssh`
provides the pinned Ubuntu base used for the local test; distro packages are
resolved during image build, so retain actual version/hash receipts rather
than calling the package installation immutable. The local helper was built
with Rust 1.95 in the already pinned Rust image; the explicit CI job selects
the product's current Rust 1.89 minimum. That CI execution is not established
by the local run.

On the shared Windows account, coordinate Cargo and the complete opaque runtime
suite with RustWork (`--family rust` for the Docker/Python runtime command).
The local runtime used a read-only root and source/helper mounts, no network,
no capabilities, no-new-privileges, core limit zero, private temporary storage,
512 MiB memory, two CPUs and a 64-process limit. No real home, SSH agent socket,
device package or provenance tree was mounted. No credential fixture is retained;
the runtime report contains metadata only.

The `native-device-keys` workflow's `openssh-profile-observations` job is
explicitly enabled by `workflow_dispatch` with `openssh_profile_review: true`.
It remains disabled for ordinary pull requests and dispatches without that
input. No private fixture is uploaded as a workflow artifact.

Primary sources for the reviewed native behavior:

- [Passphrase input and EOF handling](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/readpass.c)
- [Keygen confirmation and private-file creation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-keygen.c)
- [Cipher selection, private envelope and comment binding](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/sshkey.c)
- [Foreground agent lifecycle and core-limit path](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-agent.c)
- [Comparison and remaining accreditation requirements](../../../docs/LINUX-KEY-PROTECTION-OPTIONS.md)
