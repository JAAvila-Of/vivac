# Candidate cooperative store reservation

This component reserves one anchored physical package directory before a
candidate Linux custody process launches native children. It is shared by
Linux test sources and the disposable probe. Normal Linux custody remains
disabled. The [isolation contract](LINUX-CUSTODY-ISOLATION.md) and
[inherited channel](LINUX-CUSTODY-IPC.md) still require separate authority for
discovery, requests and session replacement.

## Acquisition and scope

StoreReservation has two forms. The lease form first validates the
PackageLease and captures an independent lease snapshot. The directory form
starts from an already anchored directory descriptor: it checks that the
descriptor is a directory and records its identity, and captures no lease
snapshot. Spawn gates require the snapshot, so they refuse a directory-form
reservation. The session keeper's startup, republication of a permit without
an adopted lease, and both retirement locks, on the package directory and on
the admission directory, use the directory form. Both forms then follow the
rest of this section. Each opens `.` relative to the anchored directory with
O_DIRECTORY, O_NOFOLLOW and O_CLOEXEC. This fresh open creates a separate open
file description: duplicating the caller's descriptor would share its lock
and could let contenders appear to acquire the same reservation.

The candidate accepts only the ext filesystem family reported by fstatfs
magic 0xEF53. It makes one `flock(LOCK_EX | LOCK_NB)` call and validates the
anchored objects again. Contention, interruption and unsupported filesystems
return an error; there is no wait, retry or fallback. Acquisition writes no
lock file or package data. The filesystem magic does not distinguish ext2,
ext3 and ext4 or attest the underlying storage deployment.

This is cooperative exclusion for that directory. It neither excludes a
noncooperating process nor serializes copied credentials in other directories.
Same-UID processes and endpoint administrators remain trusted. PackageLease
binds directory, key and manifest objects; parent-path admission, permissions
and package provenance remain the caller's obligations. Renaming a pathname
does not transfer a reservation to the replacement directory.

## Ownership and release

SpawnOwner receives the reservation before its first native launch. It
validates the retained snapshot before admission and immediately before each
spawn. NativeSession also requires the owner's reservation to match its
PackageLease. The snapshot owns independent descriptors rather than borrowing
an Arc that would prevent the session gate from releasing its own lease.

The reservation is process-bound and rejects validation after fork in a
different process. Its descriptor is close-on-exec. Release closes the owned
descriptor; it never calls LOCK_UN, which could unlock a surviving inherited
copy of the same open file description. See
[flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html).

Successful shutdown releases the reservation only after observing all owned
native exits. If bounded abandonment cannot establish those exits, SpawnOwner
retains the reservation and uncertain child resources and poisons admission
for that process. A validation error does not release an existing reservation.

## A released lock is not an exit receipt

Process death can close the last reservation descriptor while a native child
survives. A new successful acquisition therefore proves only present
cooperative lock ownership. It proves no previous child exit, completed
shutdown, human approval or recovery authority. Operational replacement must
retain independent native-exit evidence and require a fresh unlock before it
can be admitted. No lock acknowledgement enables that operation here.

The [reservation fixture](../tools/native-key-store-probe/review/RUST-RESERVATION.md)
uses public markers to test contention, normal release, uncertain abandonment,
process death with a surviving unlinked child, pathname replacement and
in-place invalidation. Its deliberately unlinked child is a negative control,
not a production launch option.
