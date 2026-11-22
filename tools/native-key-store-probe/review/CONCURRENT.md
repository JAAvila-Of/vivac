# Concurrent signing and lock experiment

This disposable fixture builds on the [sequential session](SESSION.md). It tests
the order between admission, native signing, local result publication and lock.
It does not enable Linux custody, authenticate clients, cancel a loader or test
an actual SSH disconnect.

## Ordering contract

The main thread creates and loads the owned native agent before request threads
start, and stays alive until they drain and the session closes. The agent remains
a direct child of the session process with the reviewed parent-death link.

The gate admits one signing request at a time. A second request receives an
explicit busy refusal while ready; requests during closing or after closure are
refused without new native work. There is no unbounded pending-request queue.

Admission, the closing transition and publication share one state mutex. Native
I/O and waiting for workers happen outside that mutex. An admitted request keeps
its epoch; closing advances the epoch and cancels every result not yet published.
Package revision and native process identity are still checked around signing.

Publication means placing the verified proof into the fixture's bounded local
result queue while holding the state mutex. A queued proof is already published
even if its consumer reads it later. This is a local publication boundary, not
evidence of delivery over a future client transport. A proof published before
closing is not revoked by lock.

Lock closes admission first, signals the owned agent by its retained process
descriptor, waits for active native/verifier work to drain, then releases the
session's descriptors. It acknowledges closure only after those steps succeed.
Competing lock callers serialize resource cleanup. Once admission has closed,
a timeout or uncertain exit leaves the gate closing without a successful acknowledgement; another caller
must confirm actual closure rather than infer it from the first caller's attempt.
Failure to acquire the initial state mutex refuses that attempt without claiming
that its closing transition occurred. The closing phases share one monotonic
deadline; a late completion cannot receive a successful acknowledgement.

## Actual overlap and controls

Signing and lock run in different request threads. Deadline-bounded barriers
establish overlap at these points:

- Before sending the native signing request.
- After sending its complete frame, before consuming the response.
- After receiving the native signature.
- After proof verification, before publishing the result.

The observer waits until admission is closing while the signing thread remains
paused. It tests refusal of a late request and absence of a close acknowledgement
before releasing the signing thread. Every worker must drain by its deadline;
an elapsed sleep alone is never evidence of overlap.

A sent request does not prove that OpenSSH was executing its signing algorithm
at a particular instant. A signature may already exist internally when lock
arrives. Cancellation suppresses publication and ends the native capability; it
does not claim to undo a native computation or revoke a proof already published.

The matrix includes a completed-before-lock control, parallel lock callers and
package mutation after verification but before publication. The mutation must
reach its selected seam and refuse for lease invalidation, not an unrelated
early failure. A retained connection that signed before the race and a new
connection must refuse after acknowledged closure. A separately owned sentinel
must remain able to sign after each case.

## Thread and resource boundaries

The threaded verifier uses the same existing proof-verifier executable with
bounded runtime and discarded subprocess output, without `preexec_fn`. The
parent disables core dumps and checks `no_new_privs` before starting threads.
Agent creation, PTY loading and fixture/sentinel operations that use child setup
run only when request threads are absent. This is not an approved Rust launcher.

The OS account and administrator remain trusted, as in the
[candidate profile](../../../docs/LINUX-OPENSSH-PROFILE.md). The private socket
directory does not exclude other processes running as the same UID. No client
authorization or team-principal mapping is established by this fixture.

Use the existing local image/helpers and a new report directory through RustWork
with `--family rust`:

```text
python run_lifecycle_probe.py --experiment concurrent --image LOCAL_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The [bounded launcher](LIFECYCLE.md#bounded-windows-launcher) retains its CPU,
memory, swap, process, tmpfs and independent deadline controls. Synthetic package
storage uses a separate fresh volume; only metadata/log reports return to Windows.
Selected RDP event errors refuse startup or interrupt the run. This does not
observe mouse responsiveness or change RDP/VPN/global Docker settings.

Pure regressions do not run native helpers or containers:

```text
python -B -m unittest test_concurrent_probe test_session_probe test_lifecycle_probe test_run_lifecycle_probe
```

Primary references: [Python subprocess thread restrictions](https://docs.python.org/3/library/subprocess.html)
and [the parent-thread lifetime caveat](https://www.man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html).
