# Self-hosted server platform contract

The self-hosted server and its clients target Linux, Windows and macOS
independently. A server platform does not select a client's platform. The first
pilot uses Ubuntu 24.04; that deployment choice is not a permanent Linux-only
server requirement.

This is an implementation and acceptance contract. No deployable central server
or native Windows/macOS persistence adapter is provided by this document. The
current isolated coordinator's durable store is Linux-specific. A successful
client test, cross-compilation or Linux container on another host does not
establish native server support.

## Common authority and protocol

All server adapters use the same canonical requests, signature purposes,
identifiers, authority revisions, policy decisions and resource limits. The
[shared-authority contract](TEAM-AUTHORITY.md) governs membership, publication,
revocation, trusted-client bootstrap and recovery on every platform.

The client retains its content-decryption keys and device signing material.
The coordinator orders authorized transitions and retains shared ciphertext;
it cannot manufacture participant approval or obtain a client's unlock secret.
The server's own transport credentials are separate from client content keys.

Local work capture remains independent of network availability. A local write
does not become shared authorization because a server is reachable. Every new
shared admission requires the current authority and the exact reviewed scope.

The policy and canonical history layers must not depend on paths, process IDs,
OS account names, descriptor numbers or platform permission formats. Those
objects cannot enter signed records as substitutes for participant identity.

## Native persistence boundary

Each adapter must establish and retain the identity of its private local root,
database and permitted recovery objects throughout an operation. Validation
must cover ownership, permissions and ACLs, links, object replacement, filesystem
capabilities, bounded members and lengths, and concurrent access. The initial
supported filesystem and mount profile must be identified and accredited for
each platform. Unknown or unsupported storage refuses before admission.

Linux's current experiment uses retained descriptors, procfs paths and SQLite's
Unix VFS. These are implementation mechanisms, not portable protocol inputs.
Windows and macOS need native equivalents that preserve the same guarantees;
substituting an ordinary pathname or accepting broader permissions is not an
equivalent adapter. Existing validated filesystem primitives may be reused only
after their guarantees are checked for the coordinator's access pattern.

Extracting the Linux adapter must preserve its existing accepted/refused inputs,
canonical bytes, checkpoint results, transaction ordering, errors and native
locking behavior. Capture a baseline before the refactor and compare it after
rebuilding; introducing another adapter is not permission to weaken Linux.

The shared transaction contract requires:

- Decode, verify and replay retained history against independently established
  trust and the client's accepted checkpoint; storage cannot appoint its own key.
- Recheck current authority and the exact predecessor before committing a new
  transition. Conflicting writers cannot both succeed against that predecessor.
- Commit the transition and all associated state changes atomically, including
  the operation identity and any consumed authorization.
- Acknowledge durable success only after the adapter's required synchronization
  and validation succeed. An uncertain commit requires reopen and reconciliation;
  it is never reported as an unpublished operation safe to repeat blindly.
- Keep retries subject to current authorization. A retained historical receipt
  is not a renewed permission or a new content-key grant.
- Preserve limits on complete history, database, journal, memory and work.
  Reaching a limit refuses; it does not truncate verified evidence.

File handles, locking rules and journal recovery belong to the adapter. The
authority layer must not close auxiliary descriptors that invalidate another
connection's locks, follow a renamed path to a different database, or replace
its accepted checkpoint with an unverified reopened state.

## Bounds and performance acceptance

The current isolated authority-store profile retains these ceilings across
adapters unless a separately reviewed profile explicitly changes them:

| Resource | Current ceiling |
|---|---|
| Complete admitted history | 1,024 operations |
| Database file | 2 MiB |
| Rollback journal | 4 MiB |
| SQLite value length | 8 KiB |
| SQLite statement text | 4 KiB |

Other profile-specific request, pending-evidence and ceremony bounds remain in
their existing contracts. A backend cannot raise them through database metadata
or platform detection. SQLite initialization and allocation policies must remain
separate from the product's artifact engine.

The [performance budgets](PILLARS.md#budgets) and their repeatability policy
continue to govern local capture and reading. Shared admission has a different
path: native persistence, signature verification, current authority and network
transport. Its service latency and memory profile must be measured and fixed
before that service is offered; a local 5 ms write budget is not a promise of
remote acknowledgement in 5 ms. Record verified-history size, request profile,
hardware, OS, filesystem, cache state, native synchronization and transport
conditions for each series. Compare equivalent shapes across platforms and
retain every sample, including failures and restarts.

## Process and service boundary

The initial server must support an explicit foreground lifecycle on each
accredited platform, with bounded startup, admission shutdown and exit reporting.
Running as an OS-managed service requires a separately verified native adapter.
Development and tests never install services or alter the operator's normal
configuration. Service installation is an explicit administrative operation.

Neither infrastructure account access nor service ownership establishes a
Vivac organizational role. Transport authentication, invitation consumption,
first-administrator acceptance, trusted-client enrollment and recovery must
compose with current authority before a deployable service is offered.

## Acceptance matrix

The client/server interoperability matrix has nine OS combinations: each of
Linux, Windows and macOS as a client against each as a server. Apply identical
protocol vectors and authorization results across that matrix. Publish only
the OS versions, architectures and storage profiles actually accredited.

Shared conformance tests cover canonical bytes, signatures, replay, permissions,
revocation, concurrent predecessors, exact retries, restart, stale checkpoints,
lost responses and the outcome of each atomic transition. A substituted backend
or in-memory fixture can test policy but cannot certify native persistence.

Each native adapter additionally requires actual permission and ACL checks,
link and replacement refusal, competing processes, database/journal recovery,
injected synchronization errors and process death around commit boundaries.
Keep failures and distinguish crash recovery from physical power-loss evidence.

End-to-end acceptance must also cover authenticated transport, real trusted-client
bootstrap, admission, recovery and encrypted artifact transfer with verified
complete retention. The current fixture authority and local web listener do not
satisfy those requirements.

Tests run in disposable installations with separate homes, state roots and
credentials. Compilation, runtime and native subprocesses retain the workspace's
resource coordinator and platform resource guards. No adapter is enabled by a
successful test of a different platform, and a missing adapter has no weaker
fallback.
