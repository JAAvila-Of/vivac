# Candidate administrative provisioning fixture

This experiment imports the shared Linux
[administrative writer](../../../docs/LINUX-CUSTODY-PROVISIONING.md) and
[candidate root reader](../../../docs/LINUX-CUSTODY-ROOT.md). It tests publication
mechanics under a supplied synthetic filesystem root. It does not install an
operational root, create a credential, admit a device or enable Linux custody.

## Separate administrative and user phases

The runner creates a fresh ext-family state volume and executes two containers
sequentially. The writer phase uses UID/GID 0 with only CAP_CHOWN and
CAP_DAC_READ_SEARCH. The reader phase uses UID/GID 65534 without capabilities
and mounts the same state volume read-only. Both use no network, NNP, a
read-only image root, CPU1, 512 MiB RAM without additional swap, 128 PIDs and
128 MiB temporary storage. Public source and the helper build are read-only.
Neither phase accesses the installed application's state.

The administrative phase creates the actual binding and transfers private
directory ownership to UID/GID 65534 through the candidate writer. The initial
fixture supplies only root-owned ancestors and explicit negative-case objects;
it cannot substitute preassembled bindings for the successful writer. Mixed
archive identities are verified after extraction, rather than inferred from
tar headers. The same helper and observer digests must identify both phases.

An independent observer checks executable identity, parent/start identity,
pidfds, UID/GID, NNP and capability sets. It checks every current thread, not
only the main process status. The ordinary Rust host runs four pressure threads.
Its owned publisher children are tracked by SpawnOwner receipts from birth.
Concurrent writers remain live at their pre-rename barriers for inspection.
Killed writers must have observed signal 9; ordinary exits must be code 0.
Final resources are credited only after the independently observed host exit.

## Cases and required observations

| Cases | Required observation |
|---|---|
| `new-slot` | A new generated binding, exact owners and modes, and empty `packages`, `admissions` and `openings-v1` are published and reopened by the user reader |
| `existing-destination` | A real initial publication remains unchanged when another attempt refuses |
| `existing-malformed`, `existing-symlink` | The occupied destination remains unchanged and is not adopted or repaired |
| `concurrent-writers` | Both children reach BeforeRename; one returns a publication and the other refuses with AlreadyExists |
| `wrong-scaffold-owner`, `wrong-scaffold-mode`, `scaffold-symlink`, `scaffold-acl` | Existing invalid hierarchy refuses; independent metadata/ACL/bytes/name snapshots remain unchanged |
| `missing-capabilities` | A root host with no effective/permitted capabilities refuses before writes |
| `lost-capabilities` | A provisioner constructed with privileges refuses without writes after they are dropped |
| `invalid-selectors` | Zero and unchanged-owner sentinel UID/GID values refuse without writes |
| `error-scaffold-2`, `error-scaffold-3`, `error-scaffold-4`, `error-scaffold-5` | An injected error before the selected scaffold sync returns unpublished |
| `error-stage-created`, `error-before-handoff`, `error-after-handoff` | An injected construction/handoff error creates no final slot |
| `error-binding-sync`, `error-packages-sync`, `error-admissions-sync`, `error-openings-sync`, `error-state-sync`, `error-stage-sync`, `error-before-rename` | An injected error at the selected pre-publication checkpoint returns unpublished |
| `error-after-rename`, `error-parent-sync`, `error-after-parent-sync` | An injected post-publication error preserves a complete final without crediting a successful result |
| `crash-before-rename`, `crash-after-rename` | Only the exact owned writer is killed at its barrier; the final is absent before rename and may be complete afterward |
| `lost-reply` | The owned writer completes publication internally, then dies before delivering its result to the caller; the final is not adopted as a retry result |

For `lost-capabilities`, the first observation occurs with one thread and
effective/permitted capabilities 5. After release, construction succeeds,
capabilities are dropped to zero while still single-threaded, and only then
are pressure threads created. The action observation requires zero capabilities
in every thread. The bounding set remains 5. This tests a live privilege guard,
not a constructor treated as a permanent authorization token.

The user reader runs each case afterward without writing. Complete finals may
open after indeterminate publication or lost reply, while absent, malformed,
linked or policy-invalid state refuses. Matching public binding hashes between
phases establish selected-byte continuity, not historical sync, operator receipt,
credential admission or recovery.

The writer observer separately checks preexisting scaffolds for the four
invalid-scaffold cases. `scaffold_unchanged` is true only after comparing actual
before/after state there; null means that particular check was not credited.
Snapshot equality excludes access times and directory timestamps and is not a
syscall trace. Source review separately checks that invalid existing components
are never repaired.

All root-bootstrap, provisioning-adoption, producer-origin, admission, custody,
endpoint-authority, recovery, MCP/server and lock-ACK fields stay false.
Candidate phase-observation fields become true only after the complete exact
case matrix, privilege and exit checks pass. A publication result is not an
operational installation or credential authorization.

## Scope and reproduction

Errors injected at callbacks exercise error classification; they are not actual
device I/O faults. Killing a process is not a power-loss test. The supplied root
does not prove the origin of the fixed deployment prefix. Actual bind-mount
substitution and live administrative binding replacement have a
[separate fixed-namespace fixture](RUST-FIXED-ROOT.md). Operator-facing deployment,
resident package/admission composition and the attended producer need separate
acceptance before operational use.

Build a clean auxiliary helper and run the dedicated Windows runner through
RustWork with `--family rust`, retaining the selected image, helper volume and
a new absolute report path:

```text
python run_provision_probe.py --image LOCAL_UBUNTU_IMAGE --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner verifies both container configurations before starting them, observes
selected RDP events and preserves real failures and exit codes. It removes only
its stopped containers and temporary state; report and helper volumes remain.
