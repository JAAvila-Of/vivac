# Experimental bcrypt work measurements

This fixture measures the existing 64-round OpenSSH profile on the Ubuntu 24.04
pilot build. It does not adopt a production KDF or passphrase policy, establish
password entropy, admit the backend or measure the parser's auxiliary Rust heap.

OpenSSH's serialized `bcrypt` KDF options contain a salt and a 32-bit rounds
counter. The counter is an iteration count for `bcrypt_pbkdf`, not bcrypt's
logarithmic password-hash cost. [The format](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/PROTOCOL.key)
and [implementation](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/openbsd-compat/bcrypt_pbkdf.c)
define that distinction. A measured derivation time or a minimum input length
does not by itself establish resistance to password guessing.

## Measurement windows

Each execution records one series of seven samples per case. Series A and B use
separate containers, fresh private storage and the same immutable image and
helper builds. Caches are not flushed; every initial sample is retained. These
small series report observed samples and maxima, not an estimated p99.
Provider admission and baseline-fixture preparation happen before the timed
cases and can warm executable and filesystem caches. These are prepared-fixture
measurements, not cold-start or first-installation latency.

| Case | Measured window | Initial nominal target |
|---|---|---|
| Package creation | Helper startup, encrypted creation and package publication/synchronization on the admitted store filesystem | 2,000 ms |
| Activation proof | Structural check, retained package and sealed ciphertext, owned agent startup, native load, exact identity and independently verified proof | 2,000 ms |
| Unlocked proof | Retained-revision validation, native signing, existing verifier and post-validation; the agent is already loaded | 50 ms |
| Structural refusal | Complete package-check process and dispatch guard for a controlled malformed input | 50 ms |

These targets distinguish attended setup from repeated unlocked work. The human
password-entry delay is excluded by the automatic synthetic fixture response.
Provider admission occurs before measurement. Exit and resource cleanup follow
the measured activation window and remain mandatory for a complete result.

The [latency policy](../../../docs/PILLARS.md#latency-tolerance-and-repeatability)
supplies a 10% tolerance band: 2,200 ms and 55 ms respectively. Each case records
its raw ordered samples and whether its maximum is nominal, within tolerance or
above the band. An isolated above-band sample remains visible and calls for
investigation; it does not alone establish a repeated rejection. Comparing both
series does not turn these small samples into a production percentile claim.
`status=observed` means that the measurement controls completed, including any
slow samples. It never means `performance_admitted=true`.
An ordered attempt ledger also records elapsed time when an operation raises an
exception, preserving earlier samples in the incomplete report. Completed series
must reconcile all 70 successful attempts with the per-case arrays. This does
not promise recovery of in-memory attempts after external termination or power
loss; such a run remains incomplete.

## Refuse excessive work before dispatch

The profile requires exactly 64 rounds, a 16-byte salt and the selected
authenticated cipher. The fixture mutates rounds to 0, 1, 16, 63, 65 and
4,294,967,295. It also exercises a short input that declares a large SSH field.
Every mutation starts from an accepted package and updates the manifest digest
and size, so a stale public checksum cannot substitute for the intended refusal.

The existing Rust helper's structural check must refuse each input. A central
guard dispatches native loading only after an accepted check. Negative controls
count attempted dispatches and fail if that callback is reached; they never send
an excessive-round key to native decryption. Source review establishes that this
helper check performs parsing, not a KDF. This is not an instrumented counter of
KDF calls inside the Rust decoder.

## Memory and ownership

The report records Linux `RUSAGE_CHILDREN.ru_maxrss`, in KiB, after owned child
processes have been collected. This is the child-process RSS high-water mark,
including process and mapping overhead. It is not simultaneous aggregate memory,
the observer's RSS, a per-operation memory delta or the parser's auxiliary heap.
The separate 2 MiB parser auxiliary-memory requirement remains unmeasured here.

Selected provider objects are retained and revalidated under the
[provider contract](PROVIDER.md). Only synthetic credentials are used; unlocked
agents exit before sealed descriptors, package leases and private trees are
released. An independent sentinel must still sign. Uncertain exit, incomplete
samples or lost ownership leave an incomplete result. Reports contain timing and
resource metadata, not passphrases, key contents or proof bodies.

## Reproduction

Use the bounded Windows launcher through RustWork with `--family rust`, one
separate lease per series and a new absolute report directory each time:

```text
python run_lifecycle_probe.py --experiment kdf --series A --image LOCAL_UBUNTU_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_DIRECTORY_A
python run_lifecycle_probe.py --experiment kdf --series B --image LOCAL_UBUNTU_IMAGE --build-volume EXISTING_BUILD_VOLUME --reports-dir NEW_ABSOLUTE_DIRECTORY_B
```

The launcher preserves one CPU, 512 MiB RAM, no additional swap, 128 PIDs,
128 MiB tmpfs and an independent 240-second Linux deadline. It monitors selected
RDP errors and exports only metadata reports. The synthetic persistent-package
store is separate and removed after collection. Use the retained raw samples,
component/helper hashes and controls when comparing series; do not discard the
first sample or average percentiles to obtain a passing result.
