# Native loader lifetime experiment

This disposable experiment adds native `ssh-add` and its owned terminal to the
[session lifetime boundary](SESSION.md). It tests startup cancellation, loader
failure and supervisor death. It does not enable Linux custody, establish an
approved multithreaded Rust launcher or test an actual SSH disconnect.

## Processes and secret channel

A single-threaded supervisor is the direct parent of both the agent and loader
executors. Each executor sets and checks `no_new_privs`, arms the kernel
parent-death signal with `SIGKILL`, checks the expected parent and executes the
selected native program. The observer checks process identity and credentials
after native execution; arming a setting before exec is not sufficient evidence.

The loader executor creates a session and uses only the fixture's new PTY as its
controlling terminal. It receives a sealed encrypted-key descriptor, not the
original key pathname. The synthetic passphrase travels only through that PTY
after the actual prompt is observed with echo disabled. It is never passed in
arguments, environment variables or a file. Terminal contents remain bounded in
memory and are not copied into metadata reports or logs.

Before the supervisor can be killed, it transfers a duplicate PTY master to the
independent observer using `SCM_RIGHTS`. The observer validates and retains it,
without reading, writing or changing terminal settings. Received descriptors are
close-on-exec; malformed or extra descriptors must be rejected and closed.

Retaining that master prevents supervisor death from closing the last master.
The crash controls must therefore observe the loader's `SIGKILL` exit with the
master still retained, rather than infer shutdown from EOF or terminal hangup.
The observer never cleans up a child or closes the retained master before making
the required exit observations.

## Observable boundaries

The fixture coordinates these boundaries with bounded control messages:

- Native prompt observed, echo disabled, no response written.
- Secret handed to the PTY while the native loader is confirmed alive and stopped.
- Native load completed successfully, before readiness publication.
- Validated readiness, for the normal-close control.

For the handoff boundary, the supervisor sends `SIGSTOP` through the loader's
retained process descriptor and waits for an actual stopped-child status before
writing the response once. Continue sends `SIGCONT`; cancellation or parent death
must end that stopped process. A premature native exit fails the selected seam.
This observes queued terminal input and a live process, not password consumption
or execution inside a decryption routine.

The loaded-before-readiness controls verify a real native signature independently
before triggering cancellation or supervisor death. This distinguishes ending an
already usable native capability from merely stopping an empty startup process.

## Closure and controls

Controlled cancellation closes admission first and attempts to stop both native
processes even if one stop fails. Loader and agent exit must be observed before
releasing the PTY, sealed snapshot, original package leases or acknowledging
closure. An uncertain exit remains unacknowledged and retains resource ownership;
it cannot be converted into successful closure by unwinding a cleanup stack.

For supervisor `SIGKILL`, the observer keeps process descriptors and acts as a
subreaper for its own descendants. It observes native exits before cleanup and
does not invent an acknowledgement from the dead supervisor. An already completed
loader has exit code zero; a loader still alive at the trigger must report
`SIGKILL`, as must the agent.

The matrix includes normal load/readiness/close, cancellation at prompt, at secret
handoff and before readiness, supervisor death at those same three boundaries,
and native loader death at the prompt. Negatives must reach the selected seam and
refuse for the selected cause. Interrupted startup must not publish readiness.
Retained and new native connections must refuse after shutdown, and a separately
owned sentinel must remain able to sign after every case.

The OS account and endpoint administrator remain trusted under the
[candidate profile](../../../docs/LINUX-OPENSSH-PROFILE.md). The private socket and
PTY do not exclude other trusted processes running as the same UID. This is not
client authorization or fresh human verification.

## Reproduction

Use an existing local image/helper volume and a new absolute report directory
through RustWork with `--family rust`:

```text
python run_lifecycle_probe.py --experiment loader --image LOCAL_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

When startup fails before adequate diagnostics are available, the same launcher
accepts `--diagnostic-startup` only for this experiment. It runs only the
cancel-at-prompt control, without handing a response to the native loader. It
always records an incomplete, diagnostic-only result and cannot accredit the
matrix, even if that control succeeds. Primary and cleanup errors remain
separate; diagnostic metadata must never include terminal contents or secrets.

The [bounded launcher](LIFECYCLE.md#bounded-windows-launcher) retains one CPU,
512 MiB RAM, no additional swap, 128 PIDs, a 128 MiB tmpfs and the independent
240-second Linux deadline. Package storage is a separate fresh volume; only
metadata/log reports return to Windows. Selected RDP errors refuse startup or
interrupt execution. No host terminal, login agent, RDP/VPN service or global
Docker/WSL setting is changed. Event records do not establish mouse responsiveness.

Pure tests use mocks without native executables or containers:

```text
python -B -m unittest test_loader_probe test_concurrent_probe test_session_probe test_lifecycle_probe test_run_lifecycle_probe
```

Primary references: [parent-death semantics](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html),
[controlling terminal setup](https://man7.org/linux/man-pages/man2/tiocsctty.2const.html)
and [Unix descriptor passing](https://man7.org/linux/man-pages/man7/unix.7.html).
