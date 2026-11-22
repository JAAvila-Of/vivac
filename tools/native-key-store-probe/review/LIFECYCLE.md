# OpenSSH parent-lifetime experiment

This development-only experiment investigates one condition of the candidate
[Linux custody profile](../../../docs/LINUX-OPENSSH-PROFILE.md): an unlocked
native agent must not remain usable merely because its foreground supervisor
died without running cleanup. It does not implement a vivac signing session.

## Mechanism under investigation

OpenSSH 9.6p1's `-D` option keeps the agent in the foreground. Its command-mode
parent polling is not enabled in that mode. Neither foreground execution nor
unlinking the Unix socket is a sufficient parent-lifetime mechanism.

The fixture uses a separate, single-threaded Python supervisor and an executor
that becomes the native agent through `exec`. Before execution, the executor
sets and checks `no_new_privs`, arms `PR_SET_PDEATHSIG` with `SIGKILL`, and checks
that its parent is still the exact expected supervisor. This last check closes
the case where the supervisor died before the signal was armed. The executor
does not daemonize or retain a second watchdog process.

The installed native binary is used with its original permissions, including
any setgid bit. New privileges are disabled before execution; credential changes
and provider hardening still require review. The experiment must establish
actual native exit rather than assume that an armed setting survived exec.
The creating parent thread must remain alive for the intended lifetime.
Python child setup here is an experimental mechanism, not an approved launcher
for a multithreaded Rust process or a new product dependency.

## Acceptance controls

Each linked loaded-agent case first signs a canonical vivac challenge and checks the
signature with the existing shared proof verifier. A retained Unix connection
must itself have completed a successful signing request before the trigger.

Supervisor return, default `SIGTERM` and `SIGKILL` are distinct triggers. Direct
agent death is a separate control and must not count as supervisor-death
coverage. After the trigger, the observer must see the retained native process
descriptor become readable within the fixture deadline, then observe refusal
through the existing connection and through a fresh connection. A timeout or
unexpected malformed reply is not evidence of refusal. Cleanup runs only after
the observations, so it cannot manufacture a successful death result.

A separate `native-unlinked-parent-kill` control uses the same unprivileged
supervisor/executor with `no_new_privs`, but explicitly verifies that its
parent-death signal is unset. After the supervisor receives `SIGKILL`, this
control must observe the native agent still alive and verify signatures through
both the retained connection and a new connection before cleanup. This unsafe
survival is the expected contrast, never an acceptable custody behavior.

Deterministic barriers also exercise supervisor death before arming and after
arming but before exec. Neither case may publish a usable agent socket. An
independently owned sentinel agent must remain able to sign across the cases;
this detects accidental cleanup outside the selected process tree.

The observer uses process descriptors for identity and signals, and acts as a
subreaper only for its own disposable descendants. It does not signal processes
selected by name, adopt an ambient agent, scan a user's session or infer process
death from a missing pathname. Stale paths after `SIGKILL` are possible and do
not themselves represent a signing capability.

## Reproduction and scope

Use the existing `Dockerfile.package` client image with a read-only root and
source/helper mounts, no network, all capabilities dropped, an ordinary numeric
UID/GID, a private owner-only tmpfs at `/tmp`, and a new metadata report path.
No persistent package volume is required: the key is synthetic, the writer
creates only ciphertext, and this experiment makes no filesystem-custody claim.

```text
python3 /probe/lifecycle_probe.py --writer /writer/debug/ssh-writer-graph-audit --verifier /proof/debug/openssh-proof-verifier-probe --report /reports/new-lifecycle-observations.json
```

The explicit `VIVAC_OPENSSH_DISPOSABLE=1` declaration is required. Synthetic
fixture input is not a product password channel. Do not mount a home directory,
login keyring, existing agent socket or real credential store. On the shared
Windows account, route this opaque Rust/runtime suite through RustWork with
`--family rust`; the fixture executes the isolated writer and proof verifier.

### Bounded Windows launcher

`run_lifecycle_probe.py` prepares one container from an existing local image and
build volume. It checks the selected container's configuration before starting:
one CPU, 512 MiB RAM with no additional swap, 128 PIDs, a 128 MiB private tmpfs,
and a single log file capped at 1 MiB. Source and build mounts are read-only.
It does not compile helpers, pull images or change global Docker/WSL settings.

The container runs GNU `timeout` as PID 1 with a 240-second deadline and a
five-second escalation interval, separately from the Windows polling loop.
The probe checks its resource controls before creating the synthetic fixture.
The launcher prepares a new report subdirectory owned by the fixture UID with
mode `0700` while the container is stopped. It retains the container ID and addresses cleanup only to that ID;
if cleanup cannot be confirmed, it reports the failure and retains the identity
for recovery. Reports are copied into a new explicitly selected local directory.
The report volume remains available after the stopped container is removed.

The Windows observer refuses recent or new errors in the selected RDP/session
event channels. It observes event records, not keyboard or mouse responsiveness;
an empty event query does not establish a healthy desktop. Resource limits and
timeouts do not guarantee that Windows, its drivers or the hypervisor cannot fail.

Run the launcher through RustWork with `--family rust`, selecting an existing
image, an existing build volume and a new absolute report directory:

```text
python run_lifecycle_probe.py --image LOCAL_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

Pure Python regression tests use mocks and do not start containers or helpers:

```text
python -B -m unittest test_lifecycle_probe test_run_lifecycle_probe
```

Process death does not atomically revoke a signature already returned or in
flight, and a fixture deadline is not a real-time kernel guarantee. This test
does not accredit terminal hangup, actual SSH disconnect, loader lifetime,
authenticated readiness, one session per store, package leases or serialization
of lock against signing. Those remain separate integration requirements before
the product backend can be enabled.

## Primary references

- [OpenSSH 9.6p1 agent source](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-agent.c)
- [Parent-death signal and its credential/thread caveats](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
- [No new privileges across exec](https://docs.kernel.org/userspace-api/no_new_privs.html)
- [Process descriptor identity and exit observation](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
- [Subreaper behavior](https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html)
- [GNU timeout and escalation](https://www.gnu.org/s/coreutils/manual/html_node/timeout-invocation.html)
- [PID namespace lifetime](https://www.man7.org/linux/man-pages/man7/pid_namespaces.7.html)
