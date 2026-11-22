# Linux custody process isolation and recovery boundary

This is the design and acceptance contract for Linux custody. The Linux
provisioning and creation commands use the fixed account store. Foreground
reopening, the custody transport and recovery each have separate acceptance
requirements.

## Authority and process roles

An organization may operate a central self-hosted vivac server for shared
projects, devices, membership, governance, synchronization, audit and backup.
That role does not acquire client signing or content-decryption keys. The
[shared authority contract](TEAM-AUTHORITY.md) and
[encryption pillar](PILLARS.md#encryption) still apply.

| Role | Custody boundary |
|---|---|
| Central synchronization service | Stores and transports authorized encrypted shared state; does not unlock client credentials or receive their secrets |
| Resident client MCP process and ordinary local commands | Continue local work independently of custody; future credential requests may use an explicitly unlocked, authenticated session without starting or repairing it |
| Explicit foreground custody process | Owns one store's session, native creator thread, package lease, signing gate and native process identities |
| Private native agent and loader | Belong to that custody process and the admitted package/provider revision; never replace them with an ambient login agent |

Today MCP exposes no device-key operations and does not host a Linux supervisor.
Connecting the candidate's SpawnOwner or NativeSession directly to that resident
process would share its process-wide uncertainty state across sessions. The
intended product boundary therefore places each explicitly unlocked session in
its own foreground custody process. No implicit user service, detached restart
or mandatory container is introduced.

A Linux client reached through SSH is itself a trusted execution endpoint.
Co-location with the synchronization server does not merge these roles or
establish secrecy from the endpoint administrator or other processes acting as
the same OS user. Process isolation contains lifecycle failure; it does not
establish a hostile same-UID security boundary, organizational membership or
human approval.

## Session failure and continued local work

The proposed [foreground unlock lifecycle](LINUX-DEVICE-KEYS-DESIGN.md#explicit-session-lifecycle)
still requires an explicit terminal interaction. Non-terminal requests never
prompt, unlock, create, repair or regenerate a store. Secrets do not travel
through MCP, command arguments, environment variables or readiness receipts.

One live session per store remains a requirement. A process boundary alone does
not enforce that cross-process exclusion. A second process must not treat its
fresh address space or an available owner slot as permission to unlock the same
store while an earlier session remains uncertain.

Within a session, the [shared gate](../tools/native-key-store-probe/review/RUST-CONCURRENT.md)
orders closing against publication. Explicit lock acknowledges completion only
after owned native exits, actual worker join and accredited resource release.
The [abandonment policy](../tools/native-key-store-probe/review/RUST-ABANDON.md)
permanently refuses new custody in an uncertain process and retains its bundle.
Drop is not lock and does not establish a publication cutoff.

That refusal belongs to the affected custody process. It must not stop local
capture, the resident client's ordinary requests, the central service or an
independent store's healthy session. The experimental four-owner limit is not
an organization-wide capacity or a server sizing claim. The cost and capacity
of an operational process host require measurement before adoption.

Detected SSH input/terminal loss invalidates readiness and starts explicit
closure. A silent network partition is not necessarily detected by those
signals and remains a separate acceptance requirement. Reconnection never
inherits readiness or silently restarts the session.

## Conditions for another session of the same store

Starting another process is an attempt, not evidence of recovery. All of the
following must hold before admitting a replacement session:

1. The previous session no longer admits work. Its supervisor and every native
   process that could load or use the credential have independently observed
   exits. A retained process identity must support that observation; a numeric
   PID, missing socket, stale receipt, file-lock release or successful Drop
   cannot replace it.
2. No previous worker or retained native connection can still produce a result
   to be accepted as belonging to the new session. Previously published proofs
   remain published; their organizational freshness is a separate protocol check.
3. The exact existing encrypted package and selected native provider are
   admitted again. Replacement, repair, rotation, enrollment or content-key
   recovery are separate operations and receive no implicit permission.
4. A person explicitly unlocks again through the controlling terminal under
   the [secret policy](LINUX-OPENSSH-SECRET-POLICY.md). No retained passphrase,
   retry cache or server escrow supplies that interaction.
5. The new session has a distinct generation and independently validates its
   native identities and readiness. Old requests, results, permissions or
   readiness receipts are not transplanted into it.

If the evidence needed to establish old native exit has been lost, automatic
replacement refuses. A future recovery implementation must specify who keeps
that evidence across a failed supervisor and how unclean runtime state is
handled. This document does not invent a PID-file cleanup procedure or claim
that restarting MCP, a container or the foreground process proves closure.
Unavailable custody remains an explicit error while unrelated work continues.

### Candidate storage for a fresh process

A disposable ext4 experiment evaluates a one-use opening permit. A fresh
process opens the selected store independently and takes its exclusive
reservation. It accepts an exact permit bound to the installation and
credential. Before consuming the permit, it retains a foreground controlling
terminal and prepares the dedicated owner. A request without a terminal leaves
the permit available. The attempt consumes the permit by removing it and
synchronizing the directory before it can request unlock input. The registered
path waits for keeper confirmation before configuring the retained terminal and
prompting. A missing, malformed or
mismatched permit withholds input even when the lock is available. Failure to
synchronize withholds input for that attempt.

The trusted creation producer publishes the first-opening permit. Publishing
another permit requires the keeper to consume complete authenticated closure
while it still owns the corresponding reservation and choose between releasing
the store with a permit and advancing a live successor under the retained lock.
A stored permit represents the result of the release transition. It carries no reusable exit
proof, process handle, secret or session readiness.
An old package without a permit would remain unavailable to this entry
point. The package layout, creation transaction and authority surface still
need integration. The current installation's trusted UID and administrator
boundary applies. A restored matching snapshot can defeat local history and
requires its own recovery policy.

The candidate reader selects `state/openings-v1/<credential_id>` beneath
the retained account state. The collection has mode 0700 and each permit is
a regular file with mode 0600 and one link. The reader rejects symlinks and
mount crossings and revalidates the collection, permit and admission against
their current names. The existing package and admission retain their exact
member lists.

The 48-byte permit uses the eight bytes `VVCOPEN\0`, one version byte
equal to 1, one kind byte (1 for first opening or 2 for completed closure),
six zero bytes and the 32-byte SHA-256 digest of canonical `record.json`.
A valid inspection reports the encoded kind. It grants no authority to
consume the permit or unlock the credential.

In the opt-in producer experiment, the attended producer publishes the
first-opening permit only after proof of possession, closure and durable
publication of the exact package. It retains the package reservation across
exclusive creation of the permit, synchronization of the file and its
collection, and final admission validation. An earlier failure cannot replace
an existing permit or remove a permit it may already have published. The
admission remains a separate commit. An orphan permit is unavailable without
the independently committed record it binds.

The independent observer checks contention and reader refusal after the
permit is synchronized and before admission becomes visible. A controlled
failure before the admission rename leaves the permit and the package in
place. The reader continues to refuse the orphan. When admission succeeds
the reader identifies the permit without consuming it.

The opt-in attended entry acquires the package reservation before reading the
permit. It validates the retained file and its current name, unlinks that
name, and checks that the retained file has zero links and the name is
absent. It synchronizes the collection and repeats the checks. A successful
consumption returns a single-use launch value holding the same reservation.
Only that value starts the native providers and reaches the attended prompt
on this path. A missing or invalid permit, or a failed synchronization,
withholds the launch value. No error path republishes the permit.

The native experiment uses the first-opening permit issued by the attended
producer. A separate verifier checks the first signature, and a new process
refuses the consumed permit before the second prompt. In the synchronization
case an isolated child returns EIO for the actual fsync syscall. The child
and a later unfiltered process both refuse before requesting input.

In the release experiment, the launch value can register the active
supervisor using a bootstrap kind with a distinct authenticated commitment.
The keeper can publish a clean-close permit only for this kind and only
after receiving the bound closure, confirming its exact offer and generation
on the authenticated channel, and observing the registered exits. The
supervisor waits for that confirmation and the keeper's write-side close
before exiting. The keeper derives release from the retained process
references after the exchange.

The private foreground trial keeps the same reservation and native agent
after the initial possession proof. The keeper displays a close prompt on
the retained terminal. A `CLOSE` line requests the authenticated shutdown
described above. Hangup, end of input, or observed exit of the supervisor or
agent cancels the session without issuing a new opening permit. Terminal
ownership and attributes are rechecked at prompt output and input. A change
that wakes none of the watched descriptors may remain unnoticed while idle.
The initial proof grants no enrollment authority. Discovery and authorization
for independent clients remain separate from this trial.

Publication and live handoff consume the same closure transition. A
publication attempt consumes it before any storage change. The keeper
retains the package lock until process exit.

Publication synchronizes a private staging file first, renames it without
replacement, and synchronizes the collection. The keeper rechecks foreground
terminal validity and pending hangup after staging and before the rename.
Cancellation at this check removes the validated staging file. Once the
rename completes, a later hangup does not reverse it. The keeper uses the
completed rename to distinguish unpublished failure from indeterminate
durability. That distinction survives later reservation and origin checks.
A fresh process still needs the exact admitted record and the package
reservation before consuming it. Without the authenticated closure, or
after the keeper dies while the previous session is still active, this path
has no permit to consume.

The earlier storage experiment uses public synthetic permits and a synthetic
prompt boundary. It tests consumption, contention, synchronization errors and
process death. The experiments supply no evidence about physical power loss
and do not publish or repair state for an installed credential.

The experimental [child-exit observation primitive](LINUX-CUSTODY-EXIT-WITNESS.md)
separately retains references derived from an actual child owner. It does not
supply the complete registration, store binding, external evidence keeper or
replacement authority required here.

## Acceptance before product integration

| Boundary | Required observation |
|---|---|
| Uncertain session in its own process | It refuses new custody without a successful lock acknowledgement |
| Resident host and independent process | Both keep answering while the uncertain process and its native child are still present |
| Retained ownership | The uncertain child's identity and a selected leased capability remain observable until explicitly observed termination |
| Another store's loaded session | Its exact credential still produces an independently verified proof across the other session's failure |
| Same-store replacement | Refuses while old ownership or exit evidence is uncertain; only the explicit replacement conditions admit a new generation |
| Stale runtime metadata | Cannot authorize connection, cleanup, readiness or replacement by itself |
| Client/server separation | Captured local work does not wait for custody or the network; the central service receives no unlock secret or private key |

An experiment with an ordinary Rust host and disposable worker processes can
accredit the process boundary. It cannot accredit the actual MCP loop, an
authenticated custody IPC protocol, same-store exclusion, restart recovery,
organizational authorization or central server operation. Each requires its
own implementation and evidence before enabling the Linux backend.

The [inherited control-channel candidate](LINUX-CUSTODY-IPC.md) separately defines
a first bootstrap component for already known processes. It does not resolve
endpoint discovery or same-store exclusion and replacement.

The [cooperative directory reservation](LINUX-CUSTODY-RESERVATION.md) separately
defines physical-store exclusion before native launch. Its release supplies no
old-native-exit evidence and cannot authorize same-store replacement.
