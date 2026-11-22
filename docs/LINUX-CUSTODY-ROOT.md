# Linux custody installation root

This is the selected deployment contract for the first Linux custody profile.
The administrative command previews and provisions a fixed account slot as
described in the [provisioning transaction](LINUX-CUSTODY-PROVISIONING.md).
The candidate reader and fixture writer can open the literal
namespace root and follow the fixed hierarchy; their
[fixed-namespace fixture](../tools/native-key-store-probe/review/RUST-FIXED-ROOT.md)
separates selection, binding mutation and mount-boundary checks. The earlier
candidate codec and reader also accept supplied synthetic roots; their
[disposable fixture](../tools/native-key-store-probe/review/RUST-ROOT-READER.md)
does not establish its administrative origin. The Linux creation command uses
the provisioned account store. The [administrative writer](LINUX-CUSTODY-PROVISIONING.md)
also retains a supplied-root fixture API. A disposable fixture exercises only its
own filesystem namespace. The existing
[metadata storage component](LINUX-ADMISSION-STORAGE.md) still accepts only a
caller-supplied fixture descriptor and supplies no installation authority.

The root gives a separate client an installation expectation independent of
the selected package and endpoint. It does not certify that a credential was
created by the [trusted attended producer](LINUX-CUSTODY-BOOTSTRAP.md), admit
a key, unlock it or grant organizational permission.

## Administrator provisioning, unprivileged use

The selected fixed prefix is `/var/lib/vivac/custody-v1/users/`. An explicit
administrator deployment procedure prepares one installation instance for each
participating local OS account. This is a prerequisite for this custody profile,
including on a personal machine; ordinary local capture has no such requirement.
The future deployment procedure must show its changes before they are approved.
There is no automatic elevation, privileged background service or per-operation
administrator request in the custody runtime.

The OS administrator and processes acting as the selected OS user remain
trusted. This is not protection against either actor forging public records,
changing the mount namespace or replacing the application. The fixed path is
interpreted in the trusted deployment's filesystem namespace; it cannot attest
that a container or disk image is a particular physical host.

The prefix stores persistent application state, consistent with the role of
[`/var/lib`](https://uapi-group.org/specifications/specs/linux_file_system_hierarchy/#varlib).
Its authority comes from explicit administrative provisioning and validation,
not from the spelling of the path or the filesystem hierarchy convention.

The resolver uses the current numeric real/effective UID and GID, which must
agree and be nonzero, and the following fixed layout. The decimal UID component
has no sign or leading zeroes. It is a current local selector, not a person,
device identity or durable identity across account recreation.

| Relative to the fixed prefix | Ownership and mode | Purpose |
|---|---|---|
| `<uid>/` | root:root, 0755 | Administrator-controlled account slot |
| `<uid>/binding.json` | root:root, 0644, regular file with one hard link | Public installation instance and selected local UID/GID |
| `<uid>/state/` | selected UID:GID, 0700 | Private application state |
| `<uid>/state/packages/` | selected UID:GID, 0700 | Resident encrypted packages |
| `<uid>/state/admissions/` | selected UID:GID, 0700 | Producer-created admission records |
| `<uid>/state/openings-v1/` | selected UID:GID, 0700 | Initial and successor opening permits |
| `<uid>/state/endpoints-v1/` | selected UID:GID, 0700 | Optional endpoint socket directory |

The custody prefix and its `users` directory are root:root, 0755. All earlier
ancestors are root:root directories without group/other write permission.
The resolver rejects access/default ACLs on the traversed directories and access
ACLs on the binding file. The binding is public, contains no secret and need
not be concealed from other local users; credential records remain in the
private state subtree. There is no root-owned private signing key.
The public account slot is readable so an unprivileged reader can open a real
directory descriptor and check ACLs with fgetxattr. Search-only mode would not
permit that O_RDONLY directory open; the private state remains mode 0700.

The binding will be strict canonical JSON, at most 512 bytes, with exactly
`version`, `installation_id`, `owner_uid`, `owner_gid` and `profile_revision`.
Version and profile revision are 1. IDs use the storage candidate's canonical
26-character identifier encoding; this installation ID is independently sampled
from 128 OS-random bits during new provisioning, not derived from UID, hostname,
machine ID, device/inode or a copied package. UID/GID are nonzero 32-bit numeric
OS selectors, excluding the all-ones value reserved by chown for leaving an
owner unchanged, and must match the current process. Unknown fields, revisions,
noncanonical bytes and out-of-range values refuse. This defines the required
format. The candidate codec implements these checks, but its synthetic reader
does not establish provisioning history or producer evidence.

Provisioning must create a new empty state and new binding without replacing an
existing slot or adopting existing packages or records. The
[provisioning transaction](LINUX-CUSTODY-PROVISIONING.md) specifies ordering,
exclusive publication and failure classification. The operator reviews the
fixed-prefix plan before confirming provisioning for a specific account.
Partial provisioning is unavailable, not an installation to finish by reading.

## Root selection and current-object validation

The runtime does not consult `VIVAC_HOME`, `HOME`, `XDG_STATE_HOME`, the working
directory, account-name lookup, package arguments, an endpoint response or the
central server to select this root. Existing capture locations remain governed
by their existing rules. The
[XDG specification](https://specifications.freedesktop.org/basedir/0.8/) defines
environment-selected state locations; it does not authenticate their origin.
Likewise, [`getpwuid`](https://man7.org/linux/man-pages/man3/getpwuid.3.html)
can consult a password database backed by NIS or LDAP. It is not a local-only
installation trust primitive and is not part of this resolver.

The fixed-namespace candidate opens the fixed path from a retained filesystem-root
descriptor, validates every component and retains the account slot, binding,
state and relevant child descriptors. Descriptors are CLOEXEC and at least 3;
directories used for storage are real directory descriptors, not O_PATH handles.
No path component may be a symlink or magic link. Mount crossings below
`/var/lib/vivac/custody-v1` refuse, including bind mounts. The prefix itself is
the retained mount anchor. An administrator may mount it or an earlier
ancestor, including `/var`, `/var/lib` or `/var/lib/vivac`, subject to validation.
NO_XDEV starts when opening children of that anchor, including `users`.
The custody prefix and subtree must satisfy the selected
ext-family filesystem profile. An unsupported kernel/filesystem operation or
failed policy check refuses without a weaker path-resolution fallback.

[`openat2`](https://man7.org/linux/man-pages/man2/openat2.2.html) distinguishes
whole-path symlink rejection from final-component O_NOFOLLOW, and its
RESOLVE_NO_XDEV also rejects bind-mount traversal. The implementation must
demonstrate those properties rather than infer them from an absolute path.
Opening a supplied directory descriptor is not equivalent to resolving this
administrator-selected root.

Before admitting an activation attempt, the reader checks the current named
ancestors, binding object and bytes, ownership, modes, ACL absence and retained
child objects again. Any relevant replacement or binding change invalidates
the attempt; it does not silently select another installation. Device/inode and
mount identity are used only for current retained-object comparisons, never
persisted as installation identity. Unrelated records may be added without
invalidating an unchanged credential's snapshot. The exact resolver snapshot
API for an operational deployment remains a prerequisite. The candidate reader
retains and revalidates objects within the current namespace or supplied
synthetic root; neither entry attests a physical host or grants credential
admission. Administrative mutation and actual mount-boundary checks have their
own fixed-namespace fixture rather than being inferred from source flags.

Normal reads never create, repair, chmod, remove, adopt staging entries or
regenerate a missing binding. Missing, incomplete, changed or unsupported state
returns unavailable for custody while ordinary local capture continues.

## Resident package and admission location

An operational credential ID selects exactly
`state/packages/<credential_id>/` and
`state/admissions/<credential_id>/` under the retained installation. It cannot
carry a path, choose another installation or redirect the reader. Package
filenames and the package's own safety checks remain defined by the OpenSSH
profile; the admission record's file is `record.json` as in the storage candidate.

The future attended producer receives the validated installation internally.
It must bind its package and public record to that installation ID and credential
ID, and publish only in those resident slots. The reader compares those
identities, revisions, public key and exact package digests/lengths against the
independently read admission expectation, retaining both snapshots throughout
activation. A structurally valid package outside its resident slot is unavailable
for this profile. Disposable fixtures that accept package paths establish no
exception for the operational client.

This fixes the logical location relationship. The
[resident creation-admission contract](LINUX-CREATION-ADMISSION.md) specifies
the joint reader, producer evidence, publication order and partial-failure states.
Their implementation and native acceptance remain required. In particular,
a matching `installation_id` asserted by a record
does not demonstrate that the trusted producer wrote it.
The [candidate resident snapshot fixture](../tools/native-key-store-probe/review/RUST-RESIDENT-READER.md)
composes retained root, record and package checks using synthetic public records;
it does not supply that missing producer history or operational admission.

## Copies, resets and restoration

| Situation | Required behavior or limit |
|---|---|
| Copy only a package or import a public record | Does not create admission; no import/admit-existing path is offered |
| Copy private state to an installation with a different administrator binding | Installation ID mismatch refuses; the copied state cannot replace the binding |
| Relocate a package outside its resident credential slot | Unavailable for operational activation; no path-based fallback |
| Binding, package or admission changes during an attempt | Invalidate the attempt; no implicit reread or migration |
| Recreate an OS account, reuse its UID, clone a deployment or intentionally reset the installation | Requires explicit administrator reprovisioning with a fresh installation ID and fresh attended credential creation; it cannot adopt old admission |
| Restore old state under an unchanged binding, or restore an entire matching installation | Local bytes alone cannot reliably identify the restore or establish that its policy is current; no rollback-detection claim is made |

Administrators must treat custody state and its binding as endpoint-specific
state, outside project backups and central synchronization. This is a deployment
requirement, not an assertion that the binary can detect all external copying.
There is no supported automatic restoration, relocation or migration workflow
for this profile. A supported future recovery procedure must establish current
policy and the required native-process closure and fresh human actions before
claiming any recovery or replacement acknowledgement. Resetting an installation
ID is not evidence that old native processes exited or that a server revoked
the old credential.

A trusted administrator restoring all matching state, or a trusted same-UID
process reconstructing matching records, is outside the anti-substitution
guarantee. A random local ID and filesystem permissions cannot provide hardware
anti-rollback, organizational revocation or proof of producer history.

## Acceptance before operational use

The resolver requires native cases for wrong root/binding ownership and mode,
ACLs, symlinks in ancestors and leaves, bind mounts, malformed/oversized binding,
UID/GID mismatch, environment/path injection, missing/partial provisioning,
installation mismatch, resident-location enforcement and in-flight replacement.
Unprivileged fixtures may test a supplied synthetic root, but cannot accredit
the origin of the fixed administrative prefix. That boundary needs a separate
isolated provisioning-and-reader test, without changing the installation in use.

Provisioning durability, the attended producer and the complete admission
transaction must also pass their own acceptance. The
[candidate reader fixture](../tools/native-key-store-probe/review/RUST-ROOT-READER.md)
defines its narrower acceptance and remaining cases. Provisioning and credential
creation remain separate operations.
