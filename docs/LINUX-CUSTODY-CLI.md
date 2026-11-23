# Linux custody CLI design

This contract defines four commands for the accredited
[Ubuntu OpenSSH profile](LINUX-OPENSSH-PROFILE.md). For available commands, see
[local device signing keys](DEVICE-KEYS.md).

```text
vivac device-key open-linux <credential-id> [--json]
vivac device-key close-linux <credential-id> [--json]
vivac device-key verify-linux-possession <credential-id> [--json]
vivac device-key retire-linux <credential-id> [--json]
```

The commands select the current Linux account's
[provisioned custody store](LINUX-CUSTODY-PROVISIONING.md) by the canonical,
lowercase, 26-character credential ID returned by `create-linux`. The ID
encodes a 128-bit value, with a first character from `0` to `7`. Validate it and
the command arguments before accessing custody. Accept only that positional
argument and the optional `--json` flag. The commands operate outside project
trees and take their provider, package and endpoint from the provisioned
profile. Windows and macOS keep their path-based device-key commands.

## The session lives in a user service

An open session does not belong to the terminal that opened it. Losing an SSH
connection, closing a terminal or suspending a laptop that holds the SSH client
does not end it. The session ends when it is closed, when it expires, when the
account's service manager stops it, or when the machine stops.

Each opening runs as a transient unit of the account's systemd user manager,
named `vivac-custody-<credential_id>-<generation>.service`, where the
generation is a new random identifier for that opening. The keeper is the
unit's main process. The unit is created with delegation, and the keeper starts
the supervisor, the native agent, the loader and the startup verifier in a
child cgroup of its own, called the session cgroup. Processes cannot leave a
cgroup by forking, so the session cgroup contains every process the session
started, including any that a parent-death signal missed. The unit's runtime
directory under `$XDG_RUNTIME_DIR` holds the session home, the agent socket,
the startup endpoint and the keeper's control socket. The service manager
removes that directory when the unit stops, so a lost session leaves nothing
under `/tmp`.

The account needs two things that an administrator or the account owner sets
up once:

- lingering, so the user manager keeps running without a login session. Without
  it, `open-linux` refuses and names the command an administrator runs:
  `loginctl enable-linger <account>`;
- the `sessions-v1` collection, added by the
  [session collection extension](LINUX-CUSTODY-PROVISIONING.md#session-collection-extension)
  in the same way as `endpoints-v1`.

The kernel must provide cgroup v2 with `cgroup.kill`, which Ubuntu 24.04 does.
An unsupported service manager or cgroup layout refuses before any permit is
consumed; there is no fallback to a session held by the terminal.

## Attended opening

`open-linux` requires a trusted foreground controlling terminal, including when
stdout is redirected, and holds it only while the opening is in progress. It
proceeds in this order:

1. Validate the arguments, the admission and either the opening permit or the
   evidence for a reopening (below), without consuming anything.
2. Start the session unit. The keeper writes a session record to
   `sessions-v1/<credential_id>` with an exclusive rename and synchronizes it.
   The record holds the generation, the unit name, the session cgroup's
   identity and the kernel's `boot_id`. It holds no secret.
3. The keeper starts the supervisor in the session cgroup. The supervisor
   acquires the exclusive credential reservation and consumes the permit
   under it, as before.
4. The keeper asks `open-linux` for the unlock secret over its control socket.
   `open-linux` accepts the connection only from a peer with the account's UID
   whose process belongs to the session unit. It reads the secret from the
   terminal with echo disabled, under the existing single 15-second deadline,
   and passes it to the keeper, which passes it to the loader.
5. After native unlock, startup possession verification and activation of the
   local proof listener, the keeper reports the session open. `open-linux`
   prints its receipt and returns the terminal.

`SIGHUP`, `SIGINT`, `SIGQUIT`, `SIGTERM`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU`
cancel the opening while `open-linux` is waiting for the secret or for
activation. They never reach the session itself, which lives outside the
terminal's session. The prompt also has its own deadline, because a terminal
inside a multiplexer such as tmux keeps running after the SSH connection drops
and no hangup arrives.

When an opening is cancelled or fails after the permit was consumed, the keeper
is still alive in its unit. It kills the session cgroup, waits until the kernel
reports it empty, and publishes a new opening permit exclusively, as a clean
close does. The credential stays openable. If the keeper itself cannot finish,
the reopening evidence below applies.

## Closing and expiry

`close-linux` asks the keeper over its control socket for a clean close. It does
not need a terminal. The keeper stops new proof admissions, retires the
generation, kills the session cgroup, waits until the kernel reports it empty,
publishes a new opening permit exclusively, verifies it and removes the session
record. Success requires durable publication and the final identity checks.
Cancellation observed at or before the last publication authorization
prevents the exclusive rename. Once the rename takes effect, preserve the
actual publication outcome.

A session closes itself the same way when it expires:

| Limit | Default | Measured from |
| --- | --- | --- |
| Idle | 12 hours | the last proof or signing request the session served |
| Maximum | 7 days | activation |

A stop request from the service manager, including system shutdown, is a clean
close request too. The unit allows the keeper 30 seconds to publish before the
manager kills it.

If the keeper is gone, `close-linux` asks the service manager to stop the
recorded unit, which ends every process in it, and confirms that the unit's
cgroup no longer exists or is empty. It then reports `interrupted`: no permit
was published, and the next `open-linux` reopens with that evidence.

## Reopening after a lost session

A session can end without a clean close: the machine loses power, the kernel
kills the keeper, or the user manager is killed. The permit is then consumed and
not republished. A released reservation is
[not an exit receipt](LINUX-CUSTODY-RESERVATION.md#a-released-lock-is-not-an-exit-receipt),
so `open-linux` does not reopen on the lock. It reopens on positive evidence
from the kernel that no process of the recorded session survives:

- the current `boot_id` differs from the one in the session record, because no
  process survives a restart; or
- the recorded unit's cgroup no longer exists, or exists with a different
  identity, because the kernel cannot remove a cgroup that still has
  processes; or
- the recorded cgroup exists with the same identity and its `cgroup.events`
  reports `populated 0`.

With that evidence and the unlock secret, the new keeper retires the old
session record, removes the old generation's endpoint socket after checking its
identity, writes its own record, and continues from step 3 without a permit.
The receipt says `reopened`. Without the evidence, `open-linux` refuses and
names `close-linux`, which ends the recorded unit and produces it. A session
record without a matching admission, or one that fails validation, refuses and
is left in place.

The credential is not replaced for any of this. Replacement is for losing
custody itself, such as a forgotten unlock secret or a device that leaves the
holder's control; `retire-linux` handles the local half of it.

## Independent verification

`verify-linux-possession` opens its own current admission for the given ID and
uses the derived locator to attempt one fresh proof within 20 seconds. It uses
the [local endpoint protocol](LINUX-CUSTODY-IPC.md#candidate-independent-verifier-rendezvous)
to check the live peer, current generation and signature against the retained
admission. It can run without a controlling terminal. Its success reports
possession at the time of the exchange. The command grants no reusable
capability. The proof exchange has no operation for changing custody or
signing an enrollment challenge.

A listener that replaces the discoverable endpoint is refused. The supervisor
ends the session once its endpoint location changes, and the keeper closes it
and republishes the permit.

## Attended retirement

`retire-linux` removes a credential's stored custody state from the current
account. It applies to a credential the holder no longer wants and to one whose
custody is lost, such as a forgotten unlock secret. Retirement is the local
half of replacement. Team revocation is a separate act, and neither waits for
the other.

Before changing anything, the command requires, in this order:

- a trusted foreground controlling terminal, as `open-linux` does;
- the current joint admission for the ID: the root, the admission record and
  the resident package, validated as the
  [joint reader](LINUX-CREATION-ADMISSION.md#joint-non-writing-reader) requires.
  A package without a record, a record without a package or a mismatch between
  them is refused. Partial creation state needs its own procedure;
- no open session: either no session record, or reopening evidence for the
  recorded one. An open session is refused and the command names `close-linux`;
- the exclusive credential reservation. A held reservation, such as another
  retirement, is refused without waiting or retrying;
- for the permit, the endpoint name and the session record, either no entry or
  an entry of the expected type, owner, mode and link count. Any other object
  under those names is refused.

The terminal then shows the credential and device IDs and states that the
encrypted key will be deleted, that its unlock secret will open nothing
afterwards, and that the team must revoke the credential separately. The
operator types the credential ID exactly within 60 seconds. Any other input,
end of input or timeout is refused.

Retirement addresses every object through retained descriptors. Before each
unlink it checks that the named object is still the retained one, and it never
removes recursively or removes an unexpected member. It proceeds in this order:

1. Revalidate the snapshots, then rename `admissions/<credential_id>` to
   `admissions/<credential_id>.retiring` with `renameat2(RENAME_NOREPLACE)`
   and synchronize the admissions directory. This rename is the commit: from
   then on no reader admits the credential. The tombstone keeps `record.json`,
   so the remaining steps still validate the package against it. No removal
   starts until that synchronization succeeds.
2. Remove the opening permit if present, then synchronize `openings-v1`.
3. Remove the endpoint socket if present, then synchronize `endpoints-v1`.
4. Remove the session record if present, then synchronize `sessions-v1`.
5. Remove `key.openssh` and `manifest.json`, then the package directory, then
   synchronize `packages`. The reservation is held until this step ends.
6. Remove the tombstone's `record.json` and the tombstone, then synchronize the
   admissions directory.

The tombstone name is not a canonical ID, so the joint reader never selects it.
Validation of the admissions collection accepts these names. Cancellation is
handled as in `open-linux` until the commit rename is attempted. After that,
the command completes the steps and reports the actual outcome; terminal loss
does not stop it.

A crash or failed step after the commit leaves the tombstone, and the credential
stays unavailable. Running `retire-linux` again with the same ID finds the
tombstone and no admission. It requires the terminal and the typed
confirmation again, synchronizes the admissions directory, validates what
remains against the tombstone's record, and continues from the first object
still present. If both the admission and the tombstone exist, the command
refuses and touches neither. An object that no longer matches the retained
identity stops the retirement and remains in place.

Retirement does not claim:

- Team revocation. Until the team revokes it, the enrolled credential stays
  valid for any surviving copy of the encrypted key together with its unlock
  secret.
- Erasure. The filesystem, its journal and the storage device can keep the
  encrypted bytes. Their protection is the unlock secret.

After success, the terminal reminds the operator to revoke the credential in
the team.

## Not offered: sealed unlock

The unlock secret is typed at every opening. Sealing it to a TPM so that a
session opens at boot without a person is not part of this profile. It would
make opening unattended, which needs its own contract, and Ubuntu 24.04's
systemd predates per-user credential sealing. When it is specified, it must be
optional and detected, and a failure to unseal must fall back to asking for the
secret, never to replacing the credential.

## Results

For a valid invocation, `--json` writes one final newline-terminated object
with `credential_id` and `status` to stdout. Prompts go directly to the
controlling terminal. Without `--json`, success goes to stdout and failure
diagnostics go to stderr. Reports contain only the bounded status and the
canonical ID. Device-key help advertises these commands on Linux.

| Command | Status | Exit | Meaning |
| --- | --- | --- | --- |
| `open-linux` | `open` | 0 | A session from an opening permit is active in its user service. |
| `open-linux` | `reopened` | 0 | A session is active after a lost one, on the kernel evidence above. |
| `open-linux` | `refused` | 1 | No session is open from this attempt, and the credential is as openable as before it. |
| `open-linux` | `opening_uncertain` | 5 | Contact with the session unit was lost after it could have consumed the permit, without confirmation of either outcome. |
| `open-linux` | `interrupted` | 5 | No session is open and no permit was republished. The next `open-linux` reopens on the evidence above. |
| `open-linux` | `publication_uncertain` | 5 | After a failed opening the permit rename happened, and durability or final identity validation could not be established. |
| `close-linux` | `closed` | 0 | The session's processes are gone and a new opening permit was published durably and validated. |
| `close-linux` | `refused` | 1 | No session for this ID is open in this account. Nothing changed. |
| `close-linux` | `interrupted` | 5 | The session ended without a published permit. The next `open-linux` reopens on the evidence above. |
| `close-linux` | `publication_uncertain` | 5 | The exclusive rename happened, and durability or final identity validation could not be established. |
| `verify-linux-possession` | `possession_verified` | 0 | The verifier validated fresh possession for the live local peer. |
| `verify-linux-possession` | `refused` | 1 | The exchange failed to establish fresh local possession. |
| `retire-linux` | `retired` | 0 | The admission, package, permit, endpoint entry and session record are gone, and each directory change was synchronized. |
| `retire-linux` | `refused` | 1 | This attempt changed nothing. |
| `retire-linux` | `retirement_incomplete` | 5 | The credential can no longer be opened, and some of its stored state remains or its removal is not known to be durable. Running the command again finishes it. |

Use positive evidence from this attempt for these statuses. A later filesystem
lookup cannot settle `opening_uncertain`; the next `open-linux` resolves it on
the kernel evidence or refuses. A failed opening never recreates a permit
without evidence that the session's processes are gone.

Malformed input exits 2 with a usage diagnostic on stderr. If the final receipt
cannot be delivered, exit 5 when possible and report the delivery failure to
stderr. Receipt delivery does not change a completed publication. A missing or
incomplete receipt leaves the caller without confirmation. Do not retry an
opening automatically based on receipt absence.

## Acceptance

- Exercise the public commands on an isolated provisioned Ubuntu account.
  Verify selection by ID outside any project and refusal of invalid IDs,
  additional arguments, caller locations and unsupported platforms. Check
  that Windows and macOS still receive their existing device-key command forms.
- Verify refusal before consumption with a missing controlling terminal, a
  background process, a rejected admission, lingering disabled and a missing
  `sessions-v1`. Exercise a second opener while the first holds the
  reservation. Confirm that only one generation can admit proof requests.
- Open a session over SSH, drop the SSH connection, reconnect and verify
  possession without unlocking again. Repeat inside tmux, and with the prompt
  left unanswered inside tmux until its deadline.
- Cancel during the prompt and during activation; expect `refused` and a
  republished permit. Close with `close-linux`, with a manager stop, and by idle
  and maximum expiry with shortened limits; expect a republished permit each
  time.
- Kill the keeper, then the whole unit, then power the VM off with a session
  open. Expect `interrupted` or no receipt, then `reopened` after the secret.
  With the session cgroup still populated, expect `refused` until
  `close-linux` ends it. Expect no session files left under `/tmp`.
- Fail between permit unlink and directory sync, and after the permit rename;
  assert the distinct JSON status, exit code and observed permit and record
  state for each. Check final stdout delivery failure.
- Check fresh proof from a separate terminal or a process without a terminal.
  Exercise stale locators, replaced peers, expired exchanges and retired
  generations. Check the parser, receipt and exit code on both success and
  refusal, including stdout redirection during opening.
- Retire a credential holding a permit and one left by a lost session with its
  endpoint socket and session record. Afterwards `open-linux` and
  `verify-linux-possession` refuse, `create-linux` succeeds, and other
  credentials' state is byte-identical.
- Verify that retirement changes nothing when refused for a missing terminal, a
  background process, an open session, a held reservation, a wrong
  confirmation, end of input, timeout, cancellation before the commit, partial
  creation state, an unexpected object under the permit, endpoint or record
  name, or both an admission and a tombstone.
- Stop the VM abruptly after the commit rename and after each removal and
  synchronization. After restart, the credential is either unchanged and
  retirable, or unavailable and finished by a second run. Inject an I/O error
  at each synchronization; expect `retirement_incomplete`, then completion on
  the next run. Replace an object between its check and its unlink; expect the
  retirement to stop and the object to remain.
