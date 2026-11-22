# OpenSSH package publication and native binding experiment

This development-only fixture tests the [candidate package contract](../../../docs/LINUX-OPENSSH-PROFILE.md).
It uses synthetic credentials in a disposable Linux container and a separate
persistent filesystem volume. It does not enable device-key custody in vivac,
enroll a device or touch an installed key store.

## Boundaries under test

The isolated writer adds two guarded experimental operations:

- `--package-create <new-absolute-directory> <opaque-comment>` reads a public
  fixture password pair from its owned child pipe, encodes an encrypted key in
  memory and publishes the complete private package without replacing a target.
- `--package-check <absolute-directory>` checks structure, private filesystem
  metadata, exact membership, format, limits and revision. It performs no
  decryption, signing or identity authentication and writes no derived files.

The manifest has seven fixed fields. Filesystem device/inode identities belong
to runtime leases, not the persistent credential identity. A public digest does
not authenticate public IDs or supply rollback protection.

The fixture separately retains directory, key and manifest descriptors, validates
the exact encrypted bytes, and seals an anonymous ciphertext copy against
writes and size changes. Native `ssh-add` receives that descriptor in its owned
PTY and loads into a new private foreground agent. Before producing a proof,
the fixture compares the native protected comment and public key to the expected
manifest identity and revalidates the original package. The actual shared vivac
proof verifier checks the resulting signature.

The negative identity case changes only valid public IDs, requires the structural
check to succeed, reloads the same encrypted key in an empty agent and exercises
the same binding/proof gate. It passes only when the binding mismatch prevents
every signing call and proof output. It restores the package and independently
accepts the baseline again.

Other controls cover invalid input, malformed and oversized manifests/headers,
digest changes, links, unknown members, ownership, modes and named/default POSIX
ACLs. Masked ACL entries still count as ACLs. Parent replacement after retention
must preserve the replacement and its sentinel. Changing a key byte while the
agent retains the old identity must invalidate the operation before signing;
restoration requires a fresh lease and a successful baseline proof.

Four explicit process-exit seams test publication: key synced, manifest synced,
before rename and after rename. Before commit, no accepted destination may
appear. After commit, a complete package remains even though the process exited
with the injected failure status. Concurrent creation must have one winner,
and an existing destination must remain unchanged. These observations do not
simulate power loss or attest physical persistence.

## Reproduction and isolation

Build and test both `openssh-writer` and `openssh-verifier` with their own locked
graphs. The helpers are separate unpublished workspaces excluded from the
product package. Build `Dockerfile.package` for native client and ACL utilities;
it contains no SSH server or login-account changes.

Use a read-only container root and source/helper mounts, `--network none`, a
private tmpfs `/tmp` with mode 0700, a fresh dedicated writable filesystem volume
at `/store`, and a separate metadata-only report mount at `/reports`. The fixture
requires an ext-family mount and verifies that only loopback is present. It
refuses tmpfs and overlay as package storage. CHOWN and FOWNER are enabled only
to construct and restore synthetic negative ownership/mode cases; the fixture
does not receive general filesystem or network administration privileges.

With the explicit disposable declaration, the runtime invocation is:

```text
python3 /probe/package_probe.py --writer /writer/debug/ssh-writer-graph-audit --verifier /proof/debug/openssh-proof-verifier-probe --store-root /store --report /reports/new-package-observations.json
```

The report must be new. It contains public provenance, case outcomes and limits,
not terminal transcripts, passwords, seeds or generated device identities.
The fixture verifies and removes only its own temporary roots. The dedicated
store volume contains synthetic key fixtures and must not be reused for real
credentials. The manual workflow creates and removes a fresh volume and runs
only when `openssh_profile_review` is explicitly enabled.

On the shared Windows account, coordinate the complete Docker runtime and all
Cargo checks through RustWork (`--family rust` for the opaque runtime command).
See the workflow for the exact resource limits and mount configuration.

## Observed result

The disposable Ubuntu 24.04 run with OpenSSH 9.6p1-3ubuntu13.20 passed the complete
package and native-binding fixture on an ext-family volume. Both helpers were
built on Linux with Rust 1.89. Unit tests, format, debug/release Clippy and the
Linux dependency-policy check passed. The terminal/SSH fixture was also rerun
successfully with the Rust 1.89 writer.

The report distinguishes observed structure and native binding from
`custody_accredited: false`, `filesystem_persistence_attested: false` and
`power_loss_claim: false`. It records successful immutable snapshots, changed-ID
refusal before signing, cached-key revision refusal, one concurrent creator and
the before/after-commit outcomes. Remote CI, other filesystems, measured resource
and KDF policy, native-build accreditation and signing-session supervision remain
separate acceptance gates.
