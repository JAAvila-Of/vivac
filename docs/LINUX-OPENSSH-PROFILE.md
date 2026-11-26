# Linux OpenSSH custody profile

The [local bootstrap contract](LINUX-CUSTODY-BOOTSTRAP.md) separately defines
the producer-only admission state needed before an independent client may
trust a custodian's credential. The Linux creation command publishes a
separate admission record under the
[resident admission transaction](LINUX-CREATION-ADMISSION.md). The private
endpoint verifier opens that admission independently of the package manifest.

This contract describes the Linux creation command and the private custody
paths exercised in the isolated Ubuntu profile. Public reopening and
enrollment signing require separate acceptance.

## Native protection and trust

The candidate requires an accredited OpenSSH installation supplied for the
Linux system. Native `ssh-add` decrypts and loads the encrypted key into an
owned foreground `ssh-agent`; that agent holds the unlocked key and signs.
The maintained Rust encoder only creates OpenSSH's encrypted persistent format.
It is not a fallback provider that decrypts and signs inside vivac when native
capabilities are missing. Executable provenance, capabilities and exact supported
versions remain admission gates.

The pilot's target is Ubuntu 24.04. Its
[provider experiment](../tools/native-key-store-probe/review/PROVIDER.md) separates
administrator-trusted local package metadata, retained executable objects and
observed native capabilities. It selects fixed system paths before executing
anything, checks ownership and write permissions, and retains descriptors and
digests across the operation. A package version or OpenSSH banner alone admits
nothing. Other distributions and untested builds require their own evidence.
The accredited amd64 build, `1:9.6p1-3ubuntu13.20`, was still published for
Ubuntu 24.04 in the security channel when reviewed on 21 November 2026. It is
the Ubuntu 24.04 fixed package in
[USN-8928-1](https://ubuntu.com/security/notices/USN-8928-1).

The local package database is not an attestation of repository acquisition.
Matching its stored file checksums establishes local consistency, not an
authenticated vendor signature. The endpoint administrator, dynamic loader and
system libraries remain inside the trusted system boundary. The experiment
reports that boundary explicitly; it does not fetch or verify origin over the
network while writing or signing.

This meets the intended native-provider boundary without assuming that every
platform has DPAPI's account binding. The existing macOS profile also uses a
separate passphrase-protected native store. Neither precedent requires hardware
nonexportability. OpenSSH does not supply macOS's executable ACL: private paths
and sockets restrict other accounts, not every process acting as the same OS
user. The user account and endpoint administrator remain trusted.

The person holds the passphrase; the persistent package contains ciphertext;
the owned local agent holds unlocked key material. A coordinator receives
neither passphrase nor seed. Agent availability does not establish fresh human
verification, organizational membership or permission to enroll a device.
An SSH target is the trusted execution endpoint; forwarding a laptop's login
agent is not part of this profile.

This interpretation does not change [PILLARS.md](PILLARS.md). The native provider,
package, work bounds and lifecycle must all pass before implementation is
enabled. The caller-owned secret buffers are zeroized on normal exit, including
refusal paths. Complete erasure of provider internals, compiler copies, swap or
abnormal termination is not promised. The reviewed encoder's ordinary plaintext
temporary becomes ciphertext in place on its successful bounded path; it still
has no defensive wipe guarantee for arbitrary internal bugs or future errors.

## Protected identity layout

Windows and macOS retain the common 84-byte credential payload. OpenSSH has its
own versioned protected layout: native Ed25519 seed/public-key data and the
exact ASCII comment `vivac.device-key.v1:<device_id>:<credential_id>`. Both IDs
use the canonical 26-character encoding. The comment contains no seed,
passphrase, personal identifier or private recovery data.

The comment is inside the authenticated private ciphertext. Once loaded, the
agent returns it as public metadata; putting the entire 84-byte payload there
would disclose the seed and is forbidden. A filename, public manifest or outer
public-key header alone does not establish the protected identity.

Activation must load exactly one expected key into an empty owned agent, compare
the returned comment and public key to the expected identity, and verify the
signature over the existing canonical vivac challenge. The existing challenge,
proof format and verification rules do not change. No private-key export is
needed to make this comparison. A structurally valid package is not necessarily
an unlocked, identity-validated credential.

## Persistent package version 1

The experimental package has exactly two regular files and no subdirectories:

| Member | Contract |
|---|---|
| `key.openssh` | One encrypted Ed25519 key, AES-256-GCM, bcrypt with a 16-byte salt and exactly 64 experimental rounds; nonempty and at most 64 KiB. |
| `manifest.json` | UTF-8 JSON, at most 2 KiB, strict versioned schema with duplicate and unknown fields refused. No secret data. |

The fixed manifest fields are `version` (1), `profile`
(`openssh-ed25519-aes256gcm-bcrypt-v1`), `device_id`, `credential_id`,
`public_key` (32 bytes as lowercase hex), `key_sha256` (lowercase hex),
and `key_size`.
Member names are constants, never paths supplied by the manifest. The native
header and manifest public key must agree. The experimental work factor alone
is not an adopted production KDF policy or performance claim. The selected
[secret and KDF contract](LINUX-OPENSSH-SECRET-POLICY.md) requires a newly generated
secret and attended delivery. The Linux creation command and the disposable
writer use that interaction. Integrated performance on the pilot runtime
remains an acceptance requirement.
Structural fields do not admit existing experimental packages.
The package has at most 67,584 bytes of member content in total. `key_size` is
a JSON unsigned 64-bit integer between 1 and 65,536, never a string, negative
value or floating-point value. Device/inode identities are retained and checked
within each operation, not persisted as credential identity in the manifest.

The manifest's digest and runtime filesystem identities detect replacement,
mutation and incomplete preparation relative to retained handles. They do not
authenticate public IDs, enrollment, physical uniqueness or freshness. Replacing
both manifest and key with another complete valid credential is not prevented
by a public digest. Protected-identity comparison and shared admission remain
necessary; ordinary backup/recovery does not include this package.

## Filesystem and publication

The destination is a new absolute directory outside Git and `.vivac`, under an
existing owner-only private parent. Parent, preparation and members are accessed
through retained directory/file handles. Reject symlinks, traversal, extra hard
links, foreign ownership, permissive modes, access/default POSIX ACLs and any
unexpected member. Re-resolving a selected pathname must still identify the
same retained object. SELinux labels are not treated as POSIX ACLs.
The owner-only policy applies to the immediate parent, package and members;
ordinary ancestors such as `/` need not have mode 0700, but every selected
component is opened without following symlinks and revalidated by identity.

The first package experiment admits local ext-family filesystems only, subject
to successful native ACL queries and exclusive rename/directory synchronization.
Its filesystem magic is shared by ext2/ext3/ext4 and is not proof of a specific
variant. Other filesystems, including tmpfs and overlay, refuse; passing the
experiment is not approval of every filesystem used by Linux distributions.
Filesystem breadth needs separate accreditation before the product profile is
enabled.
Here, local describes the kernel's admitted ext-family filesystem interface;
it does not attest where its backing block storage or hypervisor is located.
The profile does not infer physical locality, encryption or durability from
that filesystem magic. Unknown type, failed metadata/ACL queries or missing
required operations refuse.

No directory or file is created before password confirmation and complete
encrypted encoding in memory. A new unpredictable sibling preparation uses
mode 0700; files use exclusive creation and mode 0600. After writing and syncing
the key, the creator writes and syncs the complete manifest, verifies the exact
members, identities and encrypted digest, then syncs the preparation directory.

The commit is `renameat2` with `RENAME_NOREPLACE` relative to the retained parent,
followed by parent-directory synchronization. There is no check-then-replacing
rename fallback. A competing destination is never overwritten. Reserved
preparation names cannot be selected as committed stores.

The successful rename is the commit point even if a later check or directory
sync fails. Such a failure is an indeterminate durability result, not permission
to delete the published package or overwrite it on retry. Before commit,
cleanup removes only expected members of the still-owned preparation through
retained handles; it never recursively removes an unknown or replaced subtree.
Those identity checks and unlinks are not an atomic conditional-unlink primitive
against a hostile process with the same UID. The private-parent/account trust
boundary still applies; observing a replacement at a check refuses cleanup.
Interruption may leave an incomplete preparation for explicit inspection, but
ordinary reads do not adopt it.

Every later read verifies exact membership, private metadata, filesystem
identities, sizes and digest before and after native access. Load/sign operations
must retain the key object and revision across provider access so a pathname
replacement cannot select another file. The external loader can receive a
sealed anonymous ciphertext snapshot through an inherited descriptor while the
original file lease remains held and revalidated. No persistent rewrite is expected during OpenSSH
load or signing; any change refuses. Recovery and key rotation need separate
explicit operations.

## Foreground lifetime boundary

The [process isolation and recovery contract](LINUX-CUSTODY-ISOLATION.md)
separates each explicit foreground session from the resident client and central
synchronization service. A new process is not proof that an uncertain previous
session ended or authorization to unlock the same store again.

A live signing session needs more than a private socket or an old readiness
receipt. The foreground supervisor owns the agent and any active loader. Native
process identities and the retained package revision must remain valid across
each operation. Clients must not discover or adopt an ambient login agent.

Startup may publish readiness only after an empty owned agent has loaded and
validated exactly the protected credential. Closing admission precedes lock:
new requests refuse, admitted work finishes or is cancelled, and successful
shutdown is acknowledged only after the owned native processes have exited.
A proof already returned is not revoked by closing the session. An uncertain
shutdown must not be reported as a successful lock.

Supervisor death must also end native signing when no cleanup handler can run.
OpenSSH 9.6p1's `-D` foreground mode does not enable its command-mode parent
polling; foreground execution alone is not a parent-lifetime guarantee.
Deleting a socket pathname is insufficient: an already connected client may
retain access to the process. Acceptance must observe native process exit and
refusal through both an existing connection and a new connection, after first
verifying the same process's baseline proof with the shared verifier. The
observer must use a retained process descriptor to establish native exit
independently of socket state. It must preserve an unrelated owned sentinel
agent throughout the experiment.

The first Linux lifetime experiment investigates a kernel parent-death signal
with `SIGKILL`, an explicit expected-parent check after arming, and
`no_new_privs` before executing the native foreground agent. These are candidate
mechanisms, not a portable process-launch implementation. A privileged exec or
later credential change can clear the parent-death setting; the installed
binary's setgid bit cannot be ignored or removed to make the experiment pass.
The signal is tied to the creating parent thread, which must remain alive for
the entire intended session. A multithreaded production launcher needs its own
review of that boundary and of safe child setup.
Deterministic barriers must test death before arming, which the expected-parent
check must refuse, and death after arming but before native exec, which must
terminate the executor without publishing an agent endpoint.

The comparison also needs an explicit unlinked native control: after its
supervisor dies, verify whether the retained and new connections can still sign
before cleaning up the owned process. A surviving control demonstrates the
missing lifetime boundary; it must never count as acceptable custody behavior.
The [bounded lifetime fixture](../tools/native-key-store-probe/review/LIFECYCLE.md)
keeps that comparison separate from the linked-agent rejection cases.

Process death is not an atomic revocation of a signature already in flight and
does not establish a real-time scheduling bound. Fixtures must distinguish the
trigger, observed native exit and failed subsequent access. Terminal hangup,
actual SSH disconnect, loader interruption, package-lease invalidation,
readiness authentication and serialized lock/sign requests remain necessary
integration cases; a parent-death experiment alone does not cover them.

The [sequential session fixture](../tools/native-key-store-probe/review/SESSION.md)
joins retained-package validation, sealed load, proof-gated readiness and idle
lock in the direct parent of the native agent. Its deterministic fault seams
check refusal before readiness and discard after signing when a revision changes.
Sequential calls and startup barriers do not establish concurrent lock/sign
exclusion or SSH-session lifetime; those remain separate acceptance requirements.

The [concurrent lock fixture](../tools/native-key-store-probe/review/CONCURRENT.md)
defines a shared ordering point for closing admission and publishing a verified
proof to a bounded local result queue. Closing cancels results not yet published
and acknowledgement requires native exit and drained work before descriptor
release. The fixture distinguishes native requests sent, responses received,
proofs verified and proofs published. It does not claim to revoke a computation
or establish delivery through a future authenticated client transport.

The [loader lifetime fixture](../tools/native-key-store-probe/review/LOADER.md)
extends the experimental boundary to native loading and its owned PTY. An
independent observer retains the PTY master while forcing supervisor death, so
native exit cannot be attributed merely to closing the last master. Startup
cancellation must not publish readiness, and successful closure requires both
native processes to exit before releasing the protected revision. The handoff
barrier observes queued terminal input, not execution inside native decryption.

The [SSH lifetime fixture](../tools/native-key-store-probe/review/SSH-LIFECYCLE.md)
adds a real loopback SSH transport with and without a remote PTY. Its selected
boundaries distinguish loss of SSH input or terminal from a cancellation command
on the separate observer channel. Supervisor exit is observed independently of
native loader and agent exit. These controls concern explicit channel closure;
silent network partitions and other OpenSSH versions require their own evidence.

The resident opening-permit fixture also exercises authenticated close and cold
reopening through OpenSSH 9.6p1 in an isolated Ubuntu 24.04 guest. An independent
verifier accepts a signature after the predecessor consumes the initial permit.
Authenticated close publishes a single-use permit while the predecessor still
holds the package reservation. A new SSH session consumes that permit and
signs with a different generation. The next connection is refused before a
passphrase prompt. The passphrase is entered through the remote PTY with echo
disabled. The fixture uses a disposable loopback server and synthetic keys.

In the disconnect case, the observer terminates only the SSH client after a
verified signature. It waits for the SSH session process, keeper, predecessor
supervisor, and native agent to exit, then checks that the old sockets refuse
connections and that the package reservation can be acquired. A new SSH
connection is refused before a passphrase prompt. This experiment covers that
explicit disconnect after signing with the selected build and guest kernel.

## Acceptance experiments

The package fixture must run on an actual admitted persistent filesystem, not
infer persistence from a tmpfs test. It checks successful publication and native
load/proof, destination races, unsafe ownership/modes/ACLs, symlinks/hard links,
extra members, malformed manifests/headers, truncated files, changed digests
and filesystem identities. Native identity tests must reject changed public IDs
even when the public digest is recomputed.

Process interruptions before key sync, manifest sync and publication must never
produce an accepted partial destination. Interruption after commit may leave a
complete package. These tests establish process-death behavior, not simulated
power loss or hardware durability guarantees.

The maintained OpenSSH decoder is not a zero-allocation validator: even a
64 KiB truncated file can declare a bounded SSH field near 1 MiB, allocate that
temporary and then refuse. Parsing performs no KDF work. Account for this
amplification in measurement; the file cap alone is not a peak-memory claim.
The locked decoder caps each declared SSH field at 1,048,575 bytes. Its
single-key grammar prevents an unbounded member loop; malformed prefixes must
be exercised by the fixture. A 2 MiB parser auxiliary-memory ceiling is a budget
to measure before admission, not an achieved result inferred from source.
The [instrumented heap fixture](../tools/native-key-store-probe/review/HEAP.md)
measures the complete check's additional requested Rust heap with conservative
reallocation accounting. Its case results and exclusions are recorded in
[Performance](PERFORMANCE.md#requested-rust-heap-of-the-package-checker--2026-11-12).
This counter does not measure stack, native allocator overhead or process RSS.
The complete package, provider lifecycle, supported native builds and measured
KDF/resource policy remain separate gates.

The [disposable package experiment](../tools/native-key-store-probe/review/PACKAGE.md)
records the publication, retained-revision and native-binding observations.
Those results do not enable this profile in the product.
The separate [parent-lifetime experiment](../tools/native-key-store-probe/review/LIFECYCLE.md)
defines the native-process death and connection-refusal controls.

## Native interface references

- [Exclusive rename](https://man7.org/linux/man-pages/man2/rename.2.html)
- [File and directory synchronization](https://man7.org/linux/man-pages/man2/fsync.2.html)
- [POSIX ACL access and inheritance](https://man7.org/linux/man-pages/man5/acl.5.html)
- [Anonymous files](https://man7.org/linux/man-pages/man2/memfd_create.2.html)
- [Kernel-enforced file seals](https://man7.org/linux/man-pages/man2/F_ADD_SEALS.2const.html)
- [Parent-death signal and credential caveats](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
- [No new privileges across exec](https://docs.kernel.org/userspace-api/no_new_privs.html)
- [Process descriptors and exit observation](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
- [OpenSSH 9.6p1 agent lifecycle source](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-agent.c)
