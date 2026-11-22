# Sequential OpenSSH session experiment

This development-only fixture joins the retained package, sealed encrypted load
and parent-lifetime experiments. It does not enable Linux device-key custody.
The gate is sequential: it does not establish exclusion between concurrent
signing and lock requests, a cancellable loader, or actual SSH-disconnect behavior.

## Admission and closure

The session process is the direct parent of its native agent. The executor uses
the reviewed `no_new_privs`, parent-death signal and expected-parent handshake.
The session keeps the package descriptors and original encrypted revision while
loading only a sealed ciphertext descriptor through an owned PTY.

A socket becoming available is insufficient for readiness. The gate must first
validate the package, observe an empty owned agent, load its immutable encrypted
snapshot, compare the exact native public key and protected comment, and verify
a canonical vivac possession proof with the existing verifier. Package and
process identity are checked around native work. The proof establishes possession
only; it grants no organizational membership or authority.

As in the [candidate profile](../../../docs/LINUX-OPENSSH-PROFILE.md), the OS
account and endpoint administrator are trusted. A `0700` socket directory does
not exclude other processes running as the same UID. This fixture has no client
authentication protocol and does not equate a UID with a team principal.

Only a ready session accepts signing. A changed lease or failed process check
closes admission and prevents publication of a successful result. Restoring the
file does not reopen the same session. The post-sign mutation case must discard
the result even though the agent already produced a signature internally.

Idle lock closes admission before stopping the owned agent. Success requires
observed process exit and resource closure. A failed shutdown stays unacknowledged;
it cannot become a successful lock because a socket pathname disappeared.
The positive case retains a connection that signed before lock, then checks
refusal through that connection and a new connection after lock. An independently
owned sentinel must remain able to sign after every case.

## Controls and limits

The matrix covers:

- Ready, sign, idle lock and idempotent lock.
- Structurally valid changed identity refused before readiness.
- Package mutation after load but before readiness.
- Cached-key package mutation before a signing request.
- Package mutation after native signing but before publishing its result.
- Lock during startup, with no readiness subsequently published.
- Native agent death before a signing request.

All keys and passwords are synthetic fixture data. The new private store lives
on its own disposable filesystem volume; only the metadata report directory is
copied back to Windows. Store storage is not part of the report mount. Cleanup
addresses only the fixture's selected process descriptors, roots and volumes.
This experiment does not attest power-loss durability or generic native-provider
provenance.

Use the existing local client image and previously built helpers with the
[bounded Windows launcher](LIFECYCLE.md#bounded-windows-launcher), through
RustWork with `--family rust`:

```text
python run_lifecycle_probe.py --experiment session --image LOCAL_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The same one-CPU, 512-MiB, no-additional-swap, 128-PID and 128-MiB-tmpfs limits
apply. GNU timeout supplies the separate 240-second Linux deadline. Selected RDP
event errors refuse startup or interrupt the run; this observes event records,
not mouse responsiveness. No RDP, VPN or global Docker/WSL settings are changed.

Pure tests run without containers or native helpers:

```text
python -B -m unittest test_session_probe test_lifecycle_probe test_run_lifecycle_probe
```
