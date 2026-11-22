# SSH session lifetime experiment

This disposable experiment extends the [native loader lifetime checks](LOADER.md)
to a real SSH connection inside an isolated container. It does not enable Linux
custody or establish a production Rust process launcher.

The private SSH server listens only on container loopback. Both client and server
run as the same unprivileged fixture account, with no effective capabilities.
Transport keys are synthetic and temporary. The client pins the generated host
key, uses only its selected identity, and disables agent forwarding and connection
multiplexing. The container has no host ports, external network or host login
agent. Its fixture image gives the otherwise unused container account a shell;
it does not change a host account.

## What the disconnect means

The observer owns the SSH client process. It keeps that client's input pipe open
until the selected boundary, then kills only that client. Closing the client's
TCP connection must make the remote supervisor observe SSH input EOF, terminal
I/O failure or `SIGHUP`. A separate local control socket remains open throughout
the disconnect: closing that socket or sending a cancellation command cannot
stand in for loss of SSH.

The supervisor closes admission and stops its native loader and agent before
releasing its retained package, sealed ciphertext and terminal. Signal handlers
only record the event; cleanup runs in the supervisor's normal control flow.
Native parent-death links remain armed as a separate boundary.
The agent executor starts a separate session, and the observer checks its session
and process group before the trigger. This keeps the agent out of the SSH
terminal's hangup group while preserving its direct supervisor parent and kernel
parent-death link. Foreground mode alone does not provide this separation:
[OpenSSH 9.6p1](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/ssh-agent.c#L2269)
skips its daemon-session setup in that mode and installs a hangup handler.

The observer checks remote process identity, credentials and ancestry under its
owned SSH server before taking process descriptors. The SSH server may reap the
remote supervisor, so the report establishes supervisor exit through its retained
process descriptor, without inventing a locally collected exit code. Native
loader and agent codes come from their direct parent's actual wait results.

The observer retains a duplicate of the loader's separate PTY master until native
exit is observed. This prevents losing the SSH terminal from being confused with
closing the loader's last terminal master. Cleanup fallback cannot make a failed
disconnect control pass.

## Matrix

Both SSH modes, with a remote PTY and with pipes, have a normal-close control and
disconnect controls at three boundaries:

- Native password prompt observed with echo disabled, before any response.
- Response queued once while the loader is confirmed stopped at the handoff
  barrier described in [LOADER.md](LOADER.md#observable-boundaries).
- Readiness published after native loading and independent signature verification.

A disconnect during startup must not publish readiness. Each case requires
observed supervisor and native exits, refusal on retained and new agent
connections, and a separately owned sentinel that still signs afterward.
Acknowledgement requires native exit before resource release. A missing boundary,
unknown process identity or uncertain shutdown leaves an incomplete result.

These controls test explicit channel closure. They do not measure detection time
for a silent network partition, establish an idle timeout policy, authorize
clients sharing the same UID, or prove behavior across other OpenSSH versions.

## Reproduction

Build the small fixture layer from the existing local terminal image without
network access:

```text
docker build --pull=false --network=none -f Dockerfile.ssh-lifecycle -t LOCAL_SSH_IMAGE .
```

Run the suite through RustWork with `--family rust`, using the existing helper
volume and a new absolute metadata directory:

```text
python run_lifecycle_probe.py --experiment ssh --image LOCAL_SSH_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The [bounded launcher](LIFECYCLE.md#bounded-windows-launcher) preserves one CPU,
512 MiB RAM, no additional swap, 128 PIDs, a 128 MiB tmpfs, a read-only root and
source/helper mounts, and the independent 240-second Linux deadline. Package
storage is separate from reports and is never exported. Selected RDP errors
refuse startup or interrupt the test. Only the fixture's own resources are
removed; metadata reports remain available for review.
