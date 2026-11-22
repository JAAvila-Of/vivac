# Linux custody CLI design

This contract defines two commands for the accredited
[Ubuntu OpenSSH profile](LINUX-OPENSSH-PROFILE.md). For available commands, see
[local device signing keys](DEVICE-KEYS.md).

```text
vivac device-key open-linux <credential-id> [--json]
vivac device-key verify-linux-possession <credential-id> [--json]
```

Both commands select the current Linux account's
[provisioned custody store](LINUX-CUSTODY-PROVISIONING.md) by the canonical,
lowercase, 26-character credential ID returned by `create-linux`. The ID
encodes a 128-bit value, with a first character from `0` to `7`. Validate it and
the command arguments before accessing custody. Accept only that positional
argument and the optional `--json` flag. The commands operate outside project
trees and take their provider, package and endpoint from the provisioned
profile. Windows and macOS keep their path-based device-key commands.

## Attended reopening

`open-linux` holds the foreground process for the lifetime of the session. It
requires a trusted foreground controlling terminal, including when stdout is
redirected. The unlock secret is read from that terminal with echo disabled.
Before prompting, validate the admission and opening permit, acquire the
exclusive credential reservation, then consume the permit under that
reservation. The user sees the `CLOSE` prompt on the controlling terminal only
after native unlock, startup possession verification and activation of the
local proof listener.

Enter `CLOSE` on that terminal to request a clean close. The keeper stops new
proof admissions, retires the generation, verifies native teardown and exits
of the registered processes, and publishes a new opening permit exclusively.
Success requires durable publication and the final identity checks. Terminal
loss, loss of foreground ownership, cancellation and a failed credential
session invalidate proof admission. Handle `SIGHUP`, `SIGINT`, `SIGQUIT`,
`SIGTERM`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU` as cancellation. Cancellation
observed at or before the last publication authorization prevents the
exclusive rename. Once the rename takes effect, preserve the actual
publication outcome.

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
ends the session once its endpoint location changes, so the opener reports
`interrupted` and publishes no permit.

## Loss of the supervisor

If the supervisor ends without a clean close, the keeper reports
`opening_uncertain` or `interrupted` as described in the results table, and
the opening permit is not recreated. Native children start with a parent-death
`SIGKILL`, so the agent ends with the supervisor. The keeper observes the exits
of its own children but holds no exit evidence for the agent, so it removes
nothing the supervisor created. The
discoverable endpoint socket under the state directory, the session home with
its agent socket, and the startup endpoint directory under `/tmp` can remain.
`verify-linux-possession` refuses each of them. Retiring them and admitting the
credential again belong to the recovery procedure, which also requires a fresh
unlock.

## Results

For a valid invocation, `--json` writes one final newline-terminated object
with `credential_id` and `status` to stdout. The foreground command's prompts
go directly to the controlling terminal. Without `--json`, success goes to
stdout and failure diagnostics go to stderr. Reports contain only the bounded
status and the canonical ID. Device-key help advertises these commands on Linux.

| Command | Status | Exit | Meaning |
| --- | --- | --- | --- |
| `open-linux` | `closed` | 0 | The registered processes exited and a new opening permit was published durably and validated. |
| `open-linux` | `refused` | 1 | This attempt was stopped before the supervisor could consume the permit. This makes no assertion about permit availability. |
| `open-linux` | `opening_uncertain` | 5 | The supervisor could have consumed the permit, and the keeper lacks authenticated confirmation of consumption. |
| `open-linux` | `interrupted` | 5 | Consumption was confirmed and this attempt ended before publishing a new permit. |
| `open-linux` | `publication_uncertain` | 5 | The exclusive rename happened, and durability or final identity validation could not be established. |
| `verify-linux-possession` | `possession_verified` | 0 | The verifier validated fresh possession for the live local peer. |
| `verify-linux-possession` | `refused` | 1 | The exchange failed to establish fresh local possession. |

Use positive evidence from this attempt for these statuses. Once the keeper
may have authorized the supervisor to start, loss of contact before an
authenticated consumption witness is `opening_uncertain`. A later filesystem
lookup cannot settle that classification. A failed opening never recreates a
permit. Recovery uses a separate procedure.

Malformed input exits 2 with a usage diagnostic on stderr. If the final receipt
cannot be delivered, exit 5 when possible and report the delivery failure to
stderr. Receipt delivery does not change a completed publication. A missing or
incomplete receipt leaves the caller without confirmation. Do not retry an
opening automatically based on receipt absence.

## Acceptance

- Exercise both public commands on an isolated provisioned Ubuntu account.
  Verify selection by ID outside any project and refusal of invalid IDs,
  additional arguments, caller locations and unsupported platforms. Check
  that Windows and macOS still receive their existing device-key command forms.
- Verify refusal before consumption with a missing controlling terminal, a
  background process and a rejected admission. Exercise a second opener while
  the first holds the reservation. Confirm that only one generation can admit
  proof requests.
- Exercise loss of the supervisor before authenticated confirmation, including
  failure between unlink and directory sync. Exercise failure after confirmed
  consumption and after the permit rename. Assert the distinct JSON status and
  exit code for each, along with the observed permit state. Check clean
  `CLOSE`, SSH disconnect, managed cancellation during startup, active service
  and closing, and final stdout delivery failure.
- Check fresh proof from a separate terminal or a process without a terminal.
  Exercise stale locators, replaced peers, expired exchanges and retired
  generations. Check the parser, receipt and exit code on both success and
  refusal, including stdout redirection during attended reopening.
