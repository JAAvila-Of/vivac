# Retained child-exit observation

This experimental primitive distinguishes observing an owned child's termination
from accepting a caller's description of an exit. It does not establish complete
session closure, store admission or authority to unlock another session. Linux
custody remains unavailable in the product.

## Individual observation

`SpawnOwner::watch_child_exit` selects an existing entry of its healthy owner and
duplicates that entry's retained pidfd. It accepts a local owner token, never a
caller-selected PID, descriptor, `Receipt` or `ExitReceipt`. The owner must be in
its original process and creator thread, and the selected reference must still
be live before and after duplication. A failed capture does not synthesize exit
evidence or release the owner's existing resources.

The opaque `ChildExitWatch` retains that reference independently. One nonblocking
poll per `try_observe` call either leaves it pending or transfers the reference
once into an opaque `ObservedChildExit`. Positive observation requires `POLLIN`
without `POLLERR` or `POLLNVAL`; `POLLIN | POLLHUP` is permitted after reaping.
Hangup alone and error events cannot establish termination. A native failure or
invalid context permanently invalidates the watch. Neither type is cloneable,
serializable or constructible from public claims; neither exposes its descriptor.
Both remain bound to their original observing process and thread.

An observed reference remains retained and can be checked again, including after
the original owner has collected the child's status. It does not collect that
status itself, acknowledge the gate, release a reservation or authorize another
operation. A stopped process is still pending. A reference opened with flags
zero observes process termination rather than an individual thread's exit.
The kernel interface and its readiness semantics are documented in
[pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html).

## Acquisition is a separate obligation

The launcher requires the default `SIGCHLD` disposition and rejects
`SA_NOCLDWAIT` before `Command::spawn` and around acquisition. In the trusted
pre-exec callback the child opens its own pidfd and sends it in a single
nonblocking message over a private close-on-exec socket pair. The new program
does not inherit the channel. After spawn, the parent stores the `Child`,
checks the kernel credentials on that message and uses `waitid(P_PIDFD)` to
check that the received reference is waitable as its child without collecting
it. Later collection uses that pidfd. The parent never opens a pidfd by looking
up the returned PID. A missing or rejected pidfd leaves the entry uncertain
and collection refuses.

This capture removes the post-spawn lookup from the accepted launch path. A
different reaper can still consume the child's wait status before the parent
collects it. A trusted launch host must preserve that status for the owner.

The private Linux supervisor and keeper use a dedicated wait-host
profile. The owner checks that its creator is the original process leader,
that the kernel reports exactly one thread in `/proc/self/status`, and that
`SIGCHLD` still preserves child status. The checks run at enrollment and on
later owner operations, including immediately before spawn and after pidfd
acquisition. An observed violation permanently invalidates the owner. The
general launcher can still run in a multithreaded test host.

The dedicated profile assumes a trusted procfs mount and trusted host code.
Only the owner may collect its children's wait status. A single-thread
observation cannot certify that arbitrary code or a signal handler obeys that
rule. The private keeper starts before the ordinary command dispatcher, and
its supervisor freezes the registered native child set before starting work
on additional threads. Pilot acceptance still requires the same host and
syscall guarantees.

The current inherited control channel rejects and closes transferred
`SCM_RIGHTS` and `SCM_PIDFD` messages. This work does not relax that rejection.
The separate experimental [registration transport](LINUX-CUSTODY-PROCESS-REGISTRATION.md)
can retain candidate references from an authenticated supervisor. Those
candidates are not closure evidence or a complete registered session.

## Observation by the adopted keeper

The keeper duplicates the supervisor reference established by the authenticated
transport when it adopts the offer. It retains that reference alongside the
two registered process references and the locked directory. The retained set
survives abandonment of the protocol handle until the dedicated keeper exits.

The keeper can observe the three process references independently of the
channel and its deadline. It checks all three with nonblocking pidfd polls and
reports success only when each has exit readiness. Closing the channel while
the supervisor is alive leaves the observation pending, even if the two
registered children have exited. A polling failure permanently invalidates the
observation. The observation keeps the lock held and grants no closure or
permission to start another session.

## Complete prior-session closure

The [isolation contract](LINUX-CUSTODY-ISOLATION.md#conditions-for-another-session-of-the-same-store)
continues to govern another session. An implementation of that boundary must:

- Keep an independently established reference to the previous supervisor and
  every process able to load or use the credential. An individual child proof
  neither enumerates that set nor establishes a process's role.
- Bind the complete registration to the retained admitted store, the session's
  fresh generation and its actual launch lineage. Six device/inode numbers or
  caller-labelled roles cannot create that binding.
- Refuse secret entry, loading and readiness until registration is complete.
  Death during registration leaves the attempt uncertain; an omitted process
  cannot be treated as absent merely because it was never registered.
- Preserve exclusion and evidence across supervisor failure. A released lock,
  EOF, missing PID or socket, destructor or copied exit record is insufficient.
- Establish that the old generation admits no work and that its workers and
  retained native connections cannot publish an accepted result in the next
  generation. Process observation does not replace gate and protocol checks.
- Consume a complete, current closure capability before the new attended unlock,
  with newly admitted objects/provider and a fresh generation. Individual child
  observations are not that capability and cannot be combined by a public
  constructor taking arbitrary claims.

The operational keeper of this evidence, registration before secret access,
transfer protocol and recovery after losing the keeper remain integration
requirements. This primitive introduces no implicit user service, detached
restart, reboot exception or persistent closure receipt. Unavailable evidence
continues to refuse replacement while unrelated local work remains available.

The active-predecessor experiment binds a distinct OpenSSH launch kind
before attended input. The registered supervisor keeps the channel from the
keeper's original confirmation. After serving a proof, it can consume its
session's closure evidence only after the gate has drained and the exact
native exits have been collected. The closure frame binds the confirmed
offer and generation, and carries no descriptors.

The keeper authenticates the frame on that channel while the supervisor is
still live. It independently observes the provider exits and then the
supervisor exit through the retained references. It consumes the evidence
once to advance the generation while keeping the original lock. The
disposable observer verifies possession from both generations and refuses
reopening when the old supervisor exits without sending the closure frame.
The fixture also sends an incomplete frame and a frame with a different
generation from the authenticated supervisor. Both cases refuse the
successor before its terminal prompt.

The connection-loss case shuts down the original transport after the previous
session has signed and drained its gate, while its supervisor is still alive.
The supervisor and keeper both refuse the closure exchange. The keeper still
observes the registered exits and holds the directory lock, and the successor
receives no terminal prompt. A separate descriptor is blocked even after
the keeper drops the protocol handle. The observer then terminates the
disposable keeper with SIGKILL and acquires the directory lock through the
same independent descriptor. That acquisition carries no closure evidence.

The keeper-death case pauses after the independent verifier accepts the
signature and before the supervisor starts draining its gate. The loader has
already exited at this point. The observer checks the live keeper, supervisor
and signing agent, then sends SIGKILL to the keeper through its pidfd. It
observes the supervisor and agent exits with their own retained references
and records their actual signals. The foreground supervisor can also receive
SIGHUP when the terminal's session leader dies. The observer sends no signal
to either descendant and checks the directory lock before the keeper signal
and after the observed exits. This measures the effects of the crash on the
launch chain. It grants no authority to reopen after the crash.

## Disposable acceptance

Tests must observe a live child as pending, an independently confirmed stopped
child as pending, termination before collection, and the same reference after
collection. Unknown owner tokens refuse; poll errors and hangup-only events
cannot produce a proof; one watch cannot produce two proofs. Native results
retain helper/source identities and actual owned process observations under the
existing isolation, resource and RDP controls. These cases establish the
individual primitive only, with no real signing key or session activation.

The marker's control receive must preserve its original absolute deadline when
retrying an interrupted socket receive. Linux can interrupt a timeout-enabled
receive after stop and resume, even without a signal handler; see
[signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html). That fixture
transport behavior does not add retries to the exit watch's single poll or turn
an observation error into a pending or completed result.
