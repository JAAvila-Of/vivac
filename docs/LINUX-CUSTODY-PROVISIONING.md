# Administrative installation provisioning

This is the transaction contract for the [installation root](LINUX-CUSTODY-ROOT.md).
The Linux provisioning command and its [disposable fixture](../tools/native-key-store-probe/review/RUST-PROVISION.md)
share the transaction. The fixture keeps administrative and unprivileged
reading phases separate. Linux credential creation is still gated separately.

Provisioning establishes a new installation instance for an explicitly selected
numeric OS UID/GID. It creates no key, secret, enrollment, membership or credential
admission. The binding is public metadata. Neither the administrator nor the
central service receives a client signing or content key through this operation.

## Administrative boundary

The operator must review the concrete fixed-prefix changes and target account
before executing the administrative operation. UID/GID must be nonzero 32-bit
values other than the all-ones unchanged-owner sentinel of
[chown(2)](https://man7.org/linux/man-pages/man2/chown.2.html).
Account names, NSS lookups, environment paths and imported bindings
cannot select the root or its identity. Selecting the intended current OS
account is the administrator's responsibility. UID reuse does not prove identity.

Run `vivac device-key provision-linux --uid <uid> --gid <gid>` to review
the fixed paths, modes and owners. The preview performs no filesystem changes.
After reviewing it, run the same command with `--yes` under the administrative
identity. `--json` reports the plan or the durable binding and slot. Malformed
arguments return exit 2. Unpublished refusals return exit 1. An indeterminate
publication or a failed delivery after a durable commit returns exit 5.

This procedure runs separately from ordinary custody and capture. The writer
requires real/effective UID and GID 0 and privileges to set target ownership and
verify private directories after handoff. The planned disposable administrative
test phase uses only CAP_CHOWN and CAP_DAC_READ_SEARCH, all other capabilities
dropped, NNP, no network, a read-only image root and new dedicated state volumes.
These capabilities permit ownership changes and private-state read/search;
see [capabilities(7)](https://man7.org/linux/man-pages/man7/capabilities.7.html).
This capability profile requires native acceptance. Missing privilege refuses;
it cannot leave state root-owned and defer repair.

The separate client phase retains its nonzero UID/GID, no capabilities and
normal resource limits. There is no privileged background service or automatic
sudo in ordinary use. An administrative fixture does not authorize provisioning
the installation in use.

## Prepare the fixed scaffold

The writer anchors and validates the fixed hierarchy from the deployment's
filesystem root. Existing ancestors must satisfy the root contract. Newly
needed `vivac`, `custody-v1` and `users` directories are created exclusively
under retained parents and assigned exact root:root modes. Existing components
are validated, never chmodded, chowned or followed through links for repair.

Concurrent scaffold creation may return EEXIST. This permits only a fresh
policy-checked open of that shared scaffold component, not adoption of an
account slot or arbitrary bytes. The writer synchronizes scaffold objects and
their parent entries from deepest to shallowest through the existing
administrative anchor before relying on the path for durable publication.
A new `vivac` entry requires synchronizing its retained `/var/lib` parent;
synchronizing `users` alone cannot persist the whole newly created path.

Failure can leave a partial administrative scaffold. It creates no account
installation. Readers never create missing components or finish preparation.
The selected ext-family and mount policies apply, with no weaker fallback.

## Build one new account slot

Each attempt allocates a new private staging directory under retained `users`,
on the final slot's filesystem. Its reserved name has an OS-random suffix and
cannot be a decimal UID. Allocation is exclusive and attempted once; there is
no staging scan, reuse or adoption.

The procedure independently samples the 128-bit installation ID and staging
suffix. Each request is fixed-size `getrandom` with GRND_NONBLOCK. Unavailable
entropy, short results, interruption or errors refuse without fallback or retry,
avoiding a wait for initial entropy readiness; see
[getrandom(2)](https://man7.org/linux/man-pages/man2/getrandom.2.html).
The caller cannot supply an old installation ID or preassembled binding.

The writer creates only these objects within its new stage:

- `binding.json`: new root:root regular file, mode 0644, one hard link,
  canonical binding bytes, at most 512 bytes;
- `state`: selected UID/GID, mode 0700;
- `state/packages`, `state/admissions`, `state/openings-v1` and
  `state/endpoints-v1`: selected UID/GID,
  mode 0700, empty.

The stage stays root:root 0700 during construction. Only newly created objects
may have their modes and owners set through retained descriptors. Private
directory handles are opened before ownership handoff and retained for
validation and synchronization. Post-handoff verification must pass with the
administrative fixture's limited capabilities; modes cannot be weakened for it.

Before publication, the writer checks exact directory shape, empty private
leaves, binding type/link count/canonical bytes, target selectors, owners,
modes, ACL absence, filesystem and current object/mount identities. Checks are
bounded to this new hierarchy, without scanning users or credentials. The stage
takes final mode 0755 before its final synchronization. The selected OS user
and administrator remain trusted; this does not prevent either from modifying
private state after handoff.

## Synchronize and publish exclusively

All final ownership/mode changes precede the relevant synchronization:

1. Synchronize the complete binding after its final bytes and mode.
2. Synchronize `packages`, `admissions`, `openings-v1` and `endpoints-v1`, then
   parent `state`.
3. Revalidate the complete stage, close its writable binding handle and
   synchronize the final-mode staging directory.
4. Publish the entire slot at the exact decimal UID with
   `renameat2(RENAME_NOREPLACE)` relative to the same anchored `users` directory.
5. Revalidate the final named objects against retained descriptors and bytes,
   synchronize `users`, then validate the selected objects again.
6. Only then return durable-publication success.

There is no overwrite, copy/delete or ordinary-rename fallback. Two writers for
one UID may build different private stages, but only one may publish that final
name. The loser preserves the winner and refuses; it cannot adopt that binding
as its own result. The rename is a visibility boundary, not a cross-store
credential-admission transaction.

File sync does not persist its containing entry; rename does not supply the
whole durability result. See
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) and
[rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html).
Buffers, traversal and retries are bounded. Filesystem I/O can still block in
the kernel: no hard wall-clock bound is promised for stalled storage, and the
procedure never runs on the capture path.

## Failures and lost acknowledgements

| Boundary | Required outcome |
|---|---|
| Before account rename | Unpublished by this attempt; scaffold/staging may remain; another writer's final is untouched |
| Occupied destination | Refuse without replacement, repair, adoption or success for that slot |
| After rename, before final checks and parent sync finish | Published with indeterminate durability; preserve final and return no durable-publication success |
| After successful final checks and parent sync | Durable-publication success may be returned |
| Reply lost or writer dies after rename | A complete final may exist; existence cannot prove success was returned or received |

There is no destructive rollback after rename. A retry sees an occupied slot
and refuses rather than writing a fresh identity over it. This contract offers
no automatic staging cleanup, resume or recovery operation. A later
administrative resolution requires its own explicit procedure.

A reader may structurally validate a complete final after unacknowledged
publication. That supplies neither a historical synchronization claim nor a
retroactive ACK. Partial construction remains outside the final UID name and
unavailable; malformed/incomplete finals refuse. Readers never record a
provisioning ACK, repair state or turn installation validity into credential
admission. Attended creation and its admission transaction remain separate gates.

## Endpoint directory extension

`state/endpoints-v1` is an optional collection for the
[endpoint locator](LINUX-CUSTODY-IPC.md#candidate-socket-locator). The directory
does not revise the installation identity, binding format or `profile_revision`.
New account provisioning prepares an empty `endpoints-v1` collection in the
account stage, with the same UID, GID, mode 0700, filesystem and ACL policy as
the other private collections. It validates and synchronizes the new
directory before publishing the complete account under its final UID name.

For an existing account, run `vivac device-key provision-linux-endpoints [--json]`
as the provisioned user to preview the change. Rerun with `--yes` to publish
the directory. The command runs without elevation. It returns exit 1 for an
unpublished refusal and exit 5 if publication or receipt delivery cannot be
confirmed. In JSON mode a durable publication reports `status: "durable"`.
The writer opens the fixed installation, checks the
administrator's binding and retains the validated `state` directory. It selects
no path from command arguments, the environment or an endpoint. It creates a
uniquely named private staging directory beneath that `state` on the same mount.
The writer verifies the stage's ownership, mode, ACL policy, emptiness and
retained identity, and synchronizes the stage. Under the retained `state` it
publishes the empty directory as `endpoints-v1` using an exclusive rename.
It reopens the final by name, validates the retained identity and installation,
and synchronizes `state` before acknowledging durable publication.

The operation never writes the binding, key packages, admission records or
opening permits. An occupied destination refuses even if the occupant is a
valid directory. A failure before the rename is unpublished. The writer may
remove its own validated empty stage without touching the occupied destination.
A failure after rename, including parent synchronization, reports publication
with indeterminate durability and preserves the final directory. A retry
refuses the occupied name. Directory validity after a lost acknowledgement
does not establish what the previous writer reported or synchronized.

The current non-discovery reader selects its named collections directly and
can continue using a compatible installation with the additional sibling. The
old provisioning command still refuses an existing account. The discovery
reader treats a missing or invalid endpoint directory as unavailable and never
installs, repairs or adopts it. Directory publication is independent of session
activation and does not start a listener or recreate permission to open one.
The final directory is structurally visible as soon as it is renamed. Any
later contents are governed by the separate endpoint lifecycle, so the writer
must distinguish the empty staging requirement from validation of the final
directory's identity and policy.

Acceptance requires an isolated account with existing custody data, an older
binary exercising that account before and after the addition, and an account
provisioned with the directory from the first publication. Compare binding,
package, admission and permit bytes and object identities across the extension.
Exercise wrong owner, mode, ACL and mount, non-directory and symlink occupants,
colliding publishers, and interruption on each side of the exclusive rename.

## Native acceptance before adoption

The fixture must independently verify actual owners, capability sets,
process identities and observed exits. Its target-user reader must run without
administrative privilege. Required cases include:

- A new complete slot reopened by that reader, with empty private leaves and
  binding generated by the real writer.
- Existing, malformed and symlink destinations left untouched, and concurrent
  writers for one UID with exactly one publication.
- Invalid scaffold owner/mode/ACL/mount and missing capabilities refused before
  relying on the hierarchy.
- Failure at each scaffold sync, ownership handoff, binding/private-directory
  sync, pre-rename, post-rename and parent-sync boundary, correctly classified.
- Owned writer death before/after rename, partial construction and lost reply
  without false ACK, rollback or adoption.
- Reader refusal on administrative binding replacement, actual bind mounts and
  incomplete state, with fixed-prefix selection independent of environment.

Injected I/O errors, process death and physical power loss are distinct evidence;
none is credited as another. The candidate fixture defines its narrower
acceptance. This specification supplies no deployment change, recovery tool
or operational custody support.
