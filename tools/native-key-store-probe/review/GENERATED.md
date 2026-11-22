# Disposable generated-secret creation

This fixture exercises the selected [secret policy](../../../docs/LINUX-OPENSSH-SECRET-POLICY.md)
in the standalone writer. It does not enable Linux custody or accredit the
production Rust launcher. Historical password-input modes remain unchanged.

The new `--generated-package-create ABSOLUTE_PACKAGE PROTECTED_COMMENT` mode
performs package preflight before opening the foreground controlling terminal.
It explains terminal-recording and secret-loss risks, requires the exact phrase
`CREATE IN TRUSTED TERMINAL` within 60 seconds, and then draws 32 new bytes from
the OS random generator. It displays their lossless 64-character lowercase hex
encoding only on `/dev/tty`. Exact re-entry with echo disabled must finish under
one 180-second deadline starting before disclosure. Secret generation is
independent of the existing key-seed and salt draws. Caller-owned buffers use
`Zeroizing`; the existing erasure limitations still apply.

The fixture observes the secret and terminal transcript transiently in memory,
never in reports or exported logs. It checks foreground ownership and echo at
the interaction seams, the child's arguments/environment while awaiting secret
confirmation, captured stdout/stderr, and terminal restoration where observable.
Unavailable observations are `null`, not fabricated successes.

| Case | Expected publication | Secret disclosures |
|---|---|---|
| Valid generated package | Yes | 1 |
| Wrong acknowledgement | No | 0 |
| EOF before acknowledgement | No | 0 |
| SIGTERM before acknowledgement | No | 0 |
| Mismatched confirmation | No | 1 |
| EOF during confirmation | No | 1 |
| SIGTERM during confirmation | No | 1 |
| Terminal hangup | No | 1 |
| No controlling terminal | No | 0 |
| Disposable authorization absent | No | 0 |

The positive control uses the retained native provider's `ssh-keygen -y` with
the observed secret delivered through its private PTY response channel. Its
public result must match the package's public key, without echoing the secret.
The package checker accepts the retained positive package again after all
negative cases. This tests native decryption and public extraction, not loading
an agent, signing or authenticating a person. The legacy report field
`native_unlock_tested=false` refers to agent activation; the separate
`native_public_extraction_tested=true` records this narrower control.

The normal observer has a 25-second per-case deadline and the bounded runner has
a 240-second Linux deadline. The normal ten-case run does **not** wait for the
real 60/180-second reader deadlines to expire. Two separate deadline experiments
each run the positive control and then withhold input at one prompt, without
shortening the writer's clock. Their observer ceilings are 80 seconds for
acknowledgement and 200 seconds for confirmation. Elapsed time begins when the
observer sees the prompt, slightly after the writer starts its deadline; the
metadata lower bounds allow that observation delay, not a relaxed reader guard.
Each report identifies which deadline it observed. Complete terminal replacement,
publication error-return paths and safe multithreaded Rust launching remain
separate gates. Unit
tests inject random-source failures without adding an environment bypass to the
writer. A successful check is not adoption of a production KDF policy.

The separate `--failure-matrix` mode combines the positive control with two
terminal-attribute drift cases and the existing writer's four process-exit
points. Drift toggles an input attribute on the disposable PTY, keeping echo
disabled, before acknowledgement or confirmation. Both cases must refuse and
restore the original terminal. This does not replace `/dev/tty`, move a terminal
between sessions or simulate a compromised endpoint administrator.

The publication cases select `key-synced`, `manifest-synced`, `before-publish`
and `after-publish` through the writer's existing disposable-only fault channel.
They expect exit 77. Before commit, no package is published and one encrypted
preparation remains for fixture inspection. After the exclusive rename, the
package remains published, passes the checker and decrypts with the observed
secret. Its retention is the expected result, not permission to overwrite it.
Bounded inspection of the case's own files checks that the literal unlock
secret was not written there. No inspected bytes or secret hash enter reports.

These are abrupt process exits. They do not exercise a real failed fsync, the
error-return cleanup path or complete memory erasure after interruption. The
report keeps `terminal_replacement_tested=false` and `fsync_failure_tested=false`.
Release of fixture resources follows observed child exit, independently of the
publication outcome. This mode cannot be combined with a deadline selection.

The `--publication-errors` matrix exercises ordinary error returns separately.
The disposable writer's `VIVAC_OPENSSH_PACKAGE_ERROR_AT` accepts the same four
canonical seams, but returns an error instead of exiting immediately. It is
exclusive with a nonempty `VIVAC_OPENSSH_PACKAGE_EXIT_AT` selection; empty values
mean no selection, preserving the historical channel's behavior. Unknown,
non-Unicode or conflicting nonempty selections refuse before interaction.
These hooks remain inside the standalone
disposable writer, not the product backend.

Before commit, ordinary unwinding runs the owned preparation's cleanup. The
three precommit cases require exit 1, no published package and no remaining
preparation. These seams occur after the preparation handle and identity have
been established. An earlier failure after directory creation but before its
handle/identity is validated may leave the private preparation for inspection;
the program cannot safely delete an unverified object by pathname. This matrix
does not establish cleanup for that earlier interval or for changed/unknown
objects, which the existing cleanup policy preserves.
After rename, every error in the remaining publication checks maps
to `CommittedIndeterminate`, and the committed preparation is not deleted. The
generated mode returns exit 2, keeps stdout empty and writes exactly:

```text
Experimental OpenSSH package committed; durability is indeterminate. Do not overwrite or retry automatically.
```

The postcommit case requires the retained package to pass the checker and native
public extraction, and requires that exact diagnostic. Historical writer modes
retain their generic exit-1 contract; their earlier evidence is unchanged. The
new matrix records `publication_error_returns_tested=true`, while keeping
`fsync_failure_tested=false`: a returned test error exercises the cleanup and
classification paths without making the filesystem's actual sync syscall fail.
Its five cases include a normal positive control. It cannot be combined with
the deadline or abrupt-interruption matrices.

Reports retain cases before validation and contain only resource, provenance,
helper identity and outcome metadata. Exceptions are represented by phase and
type, never their messages or captured input. Child exit uncertainty prevents
resource cleanup; observed exit precedes release of private test resources.
No terminal transcript or secret hash is retained.

Use a new helper build volume to preserve the earlier measurement binaries. The
existing verifier remains on the original read-only build volume; this fixture
records its identity but does not execute it because it makes no signing claim.
Run through RustWork, using a fresh absolute report directory:

```text
python run_lifecycle_probe.py --experiment generated --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_BUILD_VOLUME --generated-build-volume NEW_WRITER_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
python run_lifecycle_probe.py --experiment generated-deadline --deadline-case acknowledgement --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_BUILD_VOLUME --generated-build-volume NEW_WRITER_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_DEADLINE_DIRECTORY_A
python run_lifecycle_probe.py --experiment generated-deadline --deadline-case confirmation --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_BUILD_VOLUME --generated-build-volume NEW_WRITER_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_DEADLINE_DIRECTORY_B
python run_lifecycle_probe.py --experiment generated-failures --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_BUILD_VOLUME --generated-build-volume NEW_WRITER_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_FAILURE_DIRECTORY
python run_lifecycle_probe.py --experiment generated-publication-errors --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_BUILD_VOLUME --generated-build-volume NEW_WRITER_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_ERROR_RETURN_DIRECTORY
```

Both builds mount read-only. The runtime uses one CPU, 512 MiB RAM, no additional
swap, 128 PIDs, bounded tmpfs, no network, an unprivileged UID, dropped capabilities
and `no_new_privs`. Selected RDP errors refuse startup or interrupt only the owned
experiment. They do not measure remote click responsiveness.
