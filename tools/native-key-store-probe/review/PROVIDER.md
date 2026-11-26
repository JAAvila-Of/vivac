# Ubuntu OpenSSH provider experiment

This disposable experiment checks the local provider boundary for the Ubuntu
24.04 pilot. It does not enable Linux custody, admit other distributions or
replace the [lifetime experiments](LIFECYCLE.md). It establishes observations
about an exact installed build, not a minimum supported version inferred from
an OpenSSH banner.

## Trust before execution

The experiment selects fixed absolute system paths for `ssh`, `ssh-keygen`,
`ssh-add`, `ssh-agent` and the package-query tool. It ignores executable lookup
through `PATH`. Before executing a selected object, it checks that the resolved
path and directory chain are controlled by root and cannot be written by group
or others. The executable must be a bounded regular ELF file with execution
permission. Set-user-ID execution is refused. The packaged agent's set-group-ID
bit does not authorize elevated credentials: the disposable process must inherit
`no_new_privs`, and its actual credentials are checked after execution.

The observer retains open descriptors, records SHA-256 digests, and executes
through references to those descriptors while it remains alive. It checks object
identity and contents before and after the operations. Retaining an object
protects selection from pathname replacement; it does not make a file immutable
against the trusted endpoint administrator or attest the dynamic loader and all
shared libraries. The administrator and the OS account remain trusted under the
[candidate profile](../../../docs/LINUX-OPENSSH-PROFILE.md).

The fixture checks the OS identity, installed package identity and file ownership
in the local package database. Ubuntu supplies the selected OpenSSH tools in
[`openssh-client`](https://packages.ubuntu.com/noble/openssh-client). Local
[`dpkg-query`](https://manpages.ubuntu.com/manpages/noble/man1/dpkg-query.1.html)
metadata identifies a package and installed version. Matching the package's
stored MD5 file checksums is an additional integrity check against that local
database. It is not a cryptographic provenance proof or a security verification.

[Ubuntu archive verification](https://documentation.ubuntu.com/security/software-integrity/archive-verification/)
authenticates package acquisition through signed repository metadata and package
hashes. This experiment does not reconstruct or attest that acquisition chain.
Its report labels the local database as administrator-trusted and records
`archive_signature_verified=false`. A banner, a digest without a trust anchor,
an APT candidate or a distribution name cannot stand in for authenticated origin.

## Internal producer selection

The disposable creation writer and admitted launch probe share
[`UbuntuProvider`](../../../src/device_keys/linux_ubuntu_provider.rs). Each selects the
fixed system paths before starting its agent and loader. The observer protocol
accepts no agent or loader executable descriptor. The Python observer retains
an independent selection to inspect the native processes.

This selector accepts only Ubuntu 24.04 on amd64 with the locally observed
`openssh-client` build `1:9.6p1-3ubuntu13.20`. A different installed build requires
new capability evidence and an explicit profile change. It retains the five
executable objects and OS/package metadata, checks their identity and SHA-256
contents, and compares the four OpenSSH binaries with the local dpkg checksums.
The MD5 dependency serves only that legacy database comparison.

Package queries execute the retained `dpkg-query` object through `SpawnOwner`,
with fixed arguments, a rebuilt environment, bounded output and an observed
normal exit. The owner retains the other provider objects while the query is
alive. Its write descriptors are released after confirmed shutdown before the
reader requires final EOF.

The producer retains and revalidates its selection through attended preparation,
native work, closure, publication and its retained publication result. An
uncertain owned native exit retains the provider through the store reservation.
These checks do not attest the archive,
dynamic loader or libraries, supply an admitted-credential API, or enable normal
Linux custody.

The selector's filesystem unit tests require a disposable UID 0 process and a
root-owned temporary parent without group/other write permission. This permits
testing the actual root-ownership checks without relaxing them for fixtures.
They belong in the isolated helper environment, separate from the product test
suite.

## Observable capabilities and refusals

Only synthetic disposable credentials are used. The fixture is created by the
reviewed encrypted-format writer, not by adopting direct interactive keygen
creation. Native `ssh-keygen` must decrypt it and return the exact public key.
An owned foreground agent starts empty; native `ssh-add` must load exactly the
expected public key and protected identity comment. A raw challenge signature
must pass the existing verifier, and an altered proof must fail.

Wrong passphrases, damaged authenticated ciphertext and loading without a
terminal must refuse without adding an identity. A private executable with unsafe
ownership must be rejected before execution. A poisoned `PATH` must not redirect
any selected native operation. The owned agent must exit before descriptor and
fixture release; retained and new connections must then refuse. A separately
owned sentinel must still sign after those controls.

Terminal output, passwords, ciphertext and proof bodies are never copied into
metadata reports. The report contains only bounded package metadata, component
digests and observations. Primary and cleanup errors remain separate. A timeout,
unknown identity, changed object or uncertain cleanup leaves an incomplete
result. Provider capability observations do not establish the KDF budget or a
safe production Rust process launcher.

## Reproduction

Use an existing local Ubuntu review image and helper volume through RustWork with
`--family rust`, selecting a new absolute metadata directory:

```text
python run_lifecycle_probe.py --experiment provider --image LOCAL_UBUNTU_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The [bounded launcher](LIFECYCLE.md#bounded-windows-launcher) preserves one CPU,
512 MiB RAM, no additional swap, 128 PIDs, a 128 MiB tmpfs, a read-only root,
source and helper mounts, and the independent 240-second Linux deadline. This
capability fixture uses private temporary storage and makes no new persistent
filesystem claim. Only metadata reports are exported. Selected RDP errors refuse
startup or interrupt execution; cleanup affects only the fixture's own resources.
