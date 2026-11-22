# Candidate admission-metadata storage fixture

This experiment imports the shared Linux-test-only
[metadata storage component](../../../docs/LINUX-ADMISSION-STORAGE.md).
It tests publication and snapshot mechanics with public fixture records.
It does not establish a trusted installation root, producer evidence or
credential admission, and does not load or unlock a signing key.

| Case | Required observation |
|---|---|
| `roundtrip` | Complete exclusive publication, successful synchronization result and matching metadata after reopen |
| `existing-destination` | A second publish refuses without changing the existing directory, file or bytes |
| `concurrent-publishers` | Two owned publishers contend for one credential name; exactly one succeeds and the other observes an occupied destination |
| `snapshot-in-place` | Same-inode mutation to different valid canonical metadata invalidates the retained snapshot |
| `snapshot-replacement` | Replacement by different objects with identical bytes invalidates the old snapshot |
| `symlink-refused` | A symlink cannot stand in for the final directory or record |
| `hardlink-refused` | A record with multiple hard links refuses |
| `acl-refused` | Root access/default ACLs and a record access ACL each refuse while private modes and record bytes remain unchanged |
| `pre-rename-error` | An injected pre-rename error leaves no final entry and returns no durable-publication result |
| `post-rename-sync-error` | An injected error before parent sync preserves the published entry, reports indeterminate durability and refuses replacement |
| `crash-before-rename` | The owned publisher is killed at its barrier before rename; its exit is observed and the final entry is absent |
| `crash-after-rename` | The owned publisher is killed after rename; complete metadata can be visible, but no durable acknowledgement is credited and another publish cannot replace it |

The ordinary Rust host runs four pressure threads. An independent Python
observer retains pidfds and checks host and publisher lineage, executable,
credentials, NNP and stable start identities before releasing protocol phases.
Concurrent publishers remain alive for observation. Crash cases signal only
the exact child selected through SpawnOwner's own receipt/pidfd, and require
observed signal 9. Normal child exits must be observed as code 0. Intermediate
reports do not credit future exits or final resource release.

The ACL case requires three real extended-attribute rejection checks and reports
`access_policy_checks=3`; every other case requires zero. ACL attributes must
exist after installation. Unsupported or refused attribute installation fails
the experiment instead of skipping the check.

The two injected errors exercise classification branches; they are not observed
device I/O faults. Killing a process is not a power-loss test. Reading a complete
entry after a failed publisher is not proof of its successful acknowledgement,
creation origin or admission. Reports contain counters and ephemeral identities,
not record contents or real credentials. All creation-admission, producer-origin,
trusted-root, custody, recovery, endpoint-authority, MCP/server and lock ACK
fields must stay false.

## Reproduction

Build the auxiliary crate in a clean separate target, then run through RustWork
with `--family rust`, an isolated Ubuntu image and a new absolute report path:

```text
python run_lifecycle_probe.py --experiment rust-admission-storage --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner retains CPU1, 512 MiB RAM with no additional swap, 128 PIDs, bounded
tmpfs, no network, UID/GID 65534, no capabilities and NNP. The exact observer and
deadline allowlist applies. The metadata root is a new private directory on the
disposable ext-family store; it is not an operational registry location.
