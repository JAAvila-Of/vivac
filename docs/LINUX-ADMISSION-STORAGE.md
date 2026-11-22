# Candidate admission-metadata storage

This component implements storage mechanics for public test metadata. It does
not implement creation admission, select a trusted installation root or grant
custody. It is compiled only in Linux product tests and the disposable helper.
Normal Linux custody remains unavailable.

The [bootstrap contract](LINUX-CUSTODY-BOOTSTRAP.md) requires evidence from a
trusted attended producer before a client may trust a credential. A structurally
valid stored record cannot supply that evidence. This component's reader
returns metadata and a revalidatable snapshot, never an admitted credential or
an authenticated endpoint.

## Input and root boundary

The caller supplies an already-open directory descriptor. The storage component
checks descriptor flags, current real/effective UID and GID, private ownership
and mode, ACL absence and the selected ext-family filesystem profile. It does
not establish where that descriptor came from or validate its parent path as
the installation's trust root. No VIVAC_HOME, package argument or endpoint
response is promoted to that authority here.

The candidate envelope is canonical JSON of at most 2 KiB. It contains version
and policy/profile revisions, opaque installation/creation/device/credential
identifiers, a public Ed25519 key, ciphertext and manifest digests and their
lengths. Referenced files are each bounded to 8 KiB. Unknown fields, unsupported
revisions, malformed identifiers or keys and noncanonical encodings refuse.
There are no secret, status, approval or authority fields. Profile numbers and
digests are input metadata, not verification of the procedure they describe.

This is an experimental storage envelope, not the final operational admission
format. Its producer and installation-root provenance are still prerequisites.
The supplied identifiers cannot authenticate that root or make an imported
record authoritative.

The [resident creation-admission contract](LINUX-CREATION-ADMISSION.md) selects
a separate version 2 operational envelope and joint reader. This component
continues to accept experimental version 1 only, with its existing 8 KiB
per-file bounds. It does not implement that contract or migrate its records.

## Exclusive publication

A publish attempt creates one new private staging directory with an OS-random
name. It writes one new `record.json` with mode 0600 and a single hard link,
then synchronizes the file and staging directory. It publishes the complete
directory under the exact credential identifier with
`renameat2(RENAME_NOREPLACE)`, validates the published objects and synchronizes
the root directory before returning a successful durable-publication result.
Unsupported operations and errors have no overwrite fallback.

The rename is the metadata visibility boundary. Before it, a failure leaves no
final entry; any retained staging is uncommitted and is not discovered or
adopted by the reader. After it, the complete final entry may already be visible.
A subsequent error reports publication with indeterminate durability, preserves
the entry and returns no durable-publication acknowledgement. There is no
destructive rollback or retry that replaces an occupied destination.

Seeing a final entry does not prove that the writer received or returned a
successful acknowledgement. A read can return complete metadata after a writer
dies or reports a post-rename error. That observation establishes neither
durability of a previously unacknowledged operation nor creation admission.
The future admission producer/reader must preserve these distinctions.

File and directory synchronization are separate requirements; synchronizing
the file alone does not establish persistence of its directory entry. See
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) and
[rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html).

## Anchored, non-writing reads

The reader looks up only the requested final credential identifier. It checks
the private final directory and single regular record without following
symlinks, accepting multiple hard links, scanning the registry or repairing
partial state. Metadata reads and directory-shape checks are bounded.

A snapshot retains its directory and file descriptors, initial bytes and
metadata. Validation checks the current named objects against those retained
objects and bytes. In-place mutation and replacement by a different object with
identical bytes invalidate the snapshot. A later record is not silently read
into an earlier attempt. Publishing another credential does not by itself
invalidate an unchanged existing record.

Kernel device/inode values serve only current object comparisons. They are not
persistent installation identities or proof of historical creation. The
component does not defend against a malicious endpoint administrator or
same-UID process, which remain trusted by the surrounding profile.

## Acceptance and remaining work

The disposable native experiment uses public fixtures under a new private
directory. It checks roundtrip, occupied destinations, concurrent publishers,
in-place mutation, replacement, symlinks, hard links, extended ACL rejection, pre-rename failure,
post-rename synchronization uncertainty and publisher death on either side of
the rename. Injected I/O errors and process deaths are labelled separately;
neither is a physical power-loss test.

The ACL fixture installs actual access and default ACLs on the root and an
access ACL on a published record. A zero mask preserves modes 0700 and 0600;
the fixture checks that each attribute exists and that ownership, descriptor
flags and record bytes remain unchanged before requiring rejection.

The observer must independently check owned child identities and exits before
final resource release. The original resource, privilege, network and deadline
guards remain in force. All creation-origin, installation-root, admission,
custody, recovery, endpoint-authority and server acknowledgements stay false.

The [installation-root contract](LINUX-CUSTODY-ROOT.md) selects administrative
provisioning and fixed resident slots; this storage component does not implement
or authenticate them. Operational use still requires a trusted root resolver, durable installation
and location binding, producer-only evidence, native initial identity
validation and closure, and the crash/reconciliation rules of the complete
creation-admission transaction. None follows from successful fixture storage.
