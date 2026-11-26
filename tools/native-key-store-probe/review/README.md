# oo7 format observations

These development-only tests reproduce boundaries of oo7 0.7.1, fixed to commit
`4251a43abce2a0633fa35cdf3cd360af1fa88bc0`. They do not implement a vivac backend,
approve the provider or exercise device signing. All payloads and passwords are
public test material. No D-Bus, keyring daemon, PAM listener or SSH server starts.

The five observations cover an empty container, deletion of all items, removal
of external attribute hashes, short encrypted blobs and the absence of an upper
KDF work-factor bound in the strength check. The short-blob test catches the
observed panic; its panic diagnostics are expected. A passing suite means the
observations were reproduced, not that those behaviors satisfy vivac's contract.
The dedicated workflow job runs only on explicit workflow dispatch with
`oo7_format_review` enabled. It is not a prerequisite for ordinary native-store
checks or the product's Rust toolchain.

## Reproduction

Use a disposable Linux build environment with Python 3.11 or later, Git and
Rust 1.95. Keep the source checkout read-only and provide a new build directory
whose parent already exists. The runner verifies the selected commit, archives
it into a temporary copy and appends only the review module. It never modifies
the checkout. It refuses archive links except package LICENSE links to the root
license, which it materializes as regular files; archive counts and expanded
sizes are bounded. It prints the archive and fragment digests with the commit.

```text
git clone --depth 1 --branch 0.7.1 https://github.com/linux-credentials/oo7.git /tmp/oo7-source
python3 run_oo7_format_review.py --source /tmp/oo7-source --target-dir /tmp/oo7-format-build
```

Run the Python command from this directory. Cargo uses the upstream lockfile
and runs only the `vivac_format_review` module. The runner checks that all five
named observations passed so an empty test selection cannot credit the review.
It deliberately refuses an existing build destination. It forwards only build
toolchain/path/locale environment settings to Cargo; this is not a host sandbox.
Use disposable build caches without user credentials and do not mount a real
home, keyring, SSH directory or project tree into the build environment.

On the shared Windows account, wrap the Docker or other opaque Rust build/test
invocation with RustWork and `--family rust` as required by the workspace. Keep
the build and runtime inside the managed command; do not invoke this driver
unmanaged just because its entry point is Python.

The reviewed Linux container used
`rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`,
with a read-only root and source/review mounts, private temporary storage,
separate Cargo cache/build volumes, two CPUs, 3 GiB memory and a 256-process
limit. This establishes a reproducible build environment; it does not accredit
the daemon's filesystem access, native lifecycle, package safety or SSH behavior.

## Interpretation for an adapter

Do not equate successful password validation with possession of the expected
credential: an empty keyring must still fail exact-item and payload binding.
Require exactly one expected item and compare complete attributes as well as
the protected device/credential/public-key binding. A MAC-protected item is not
a claim that its outer container is intact or fresh. Reject unsupported members,
counts, protection revisions and work factors before allowing provider access.

The observations do not prove key disclosure or signature forgery. A child
panic must become a bounded unavailable/refused result; it must never leave a
stale signing session usable or disrupt ordinary local capture. Whole-package
rollback and replacement require a separate trust analysis; a mutable public
digest alone cannot supply authenticated freshness.

See the [provider review](../../../docs/LINUX-DEVICE-KEYS-REVIEW.md) for source
evidence, startup side effects and the remaining admission requirements.
