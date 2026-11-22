# Requested Rust heap of the package checker

This disposable experiment measures the existing package checker with an optional
`heap-probe` feature in the standalone writer. The feature is disabled by default
and adds no dependency. It does not change the product or enable Linux custody.

## What the counter measures

The feature wraps `System` through Rust's global allocator. It tracks requested
live bytes from process startup, then measures the additional peak around the
same `package::check` function used by the normal helper. Key and manifest reads,
retained-object validation, decoding and final revalidation are inside the
window. Command arguments and harness setup remain alive outside it. JSON
serialization happens after the window closes.

Allocation hooks use only checked atomic arithmetic. Null allocations do not
increase live bytes; failed reallocations keep the original request live.
Reallocation conservatively records the old allocation coexisting with the new
request, including when the underlying allocator can resize in place. Invalid
arithmetic, nested or abandoned scopes invalidate the measurement. The CLI checks
that it is single-threaded before starting; the checker call graph creates no
threads and performs no KDF or native load.

The hard ceiling is **2,097,152 additional requested Rust bytes**. It has no
latency tolerance. This counter excludes stack, allocator metadata/reservation,
native allocations outside Rust's allocator and process mappings. It measures a
conservative requested-byte peak, rather than physical allocation residency or
the RSS reported in the [work measurements](KDF.md).

## Fixtures and controls

Every case runs in three fresh checker processes. An accepted synthetic package
provides the control.

The fixture-publication readback admits exactly one byte beyond the normal file
bound so that the oversized control reaches the checker. This auxiliary readback
does not change the original reader or the checker's 65,536-byte admission limit.

Mutations update public manifest size and digest and must have the same acceptance
result in the original, uninstrumented checker:

- A valid encrypted package.
- Maximum rounds and maximum key count.
- Truncated body fields declaring 1,048,575, 1,048,576 or 4,294,967,295 bytes.
- A truncated salt declaring 1,048,575 bytes.
- A near-64-KiB input with a large declared body field.
- Malformed input of exactly 65,536 bytes and oversized input of 65,537 bytes.

The prefix at the decoder's field boundary exercises allocation amplification;
the next value and maximum integer exercise refusal beyond that boundary. These
are measured adversarial cases for the locked grammar and input bounds, not an
exhaustive proof of all possible inputs. A final valid-package check confirms
that the independent control still works; this experiment has no signing-agent
sentinel and makes no signature claim.

Each raw observation includes the baseline, additional peak, live bytes after
the check, acceptance and validity flags. Summary peaks must equal the largest
raw observation. The driver retains observations before checking the ceiling,
so an excess is preserved in an incomplete report. It refuses malformed or
duplicate JSON fields, invalid counters and acceptance mismatches. Checker
processes must finish before descriptors and private fixture trees are released.

## Reproduction and separation

Compile the feature separately with the existing locked graph and declared
toolchain, through RustWork with `--family rust`. Use a fresh build volume and
retain the original helper volume; both are mounted read-only during measurement.
The original helper creates and checks fixtures. The instrumented helper receives
`--heap-package-check ABSOLUTE_PACKAGE_PATH` and emits one JSON line. Exit 0 means
valid instrumentation within the ceiling, including an expected parser refusal;
exit 2 records a ceiling miss, while exit 1 indicates setup or instrumentation
failure.

Run the bounded launcher with a new absolute report directory:

```text
python run_lifecycle_probe.py --experiment heap --image LOCAL_UBUNTU_IMAGE --build-volume ORIGINAL_HELPER_VOLUME --heap-build-volume NEW_INSTRUMENTED_HELPER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runtime retains one CPU, 512 MiB RAM, no additional swap, 128 PIDs,
128 MiB tmpfs and the independent 240-second Linux deadline. Selected RDP errors
refuse startup or interrupt the run. Source/helper hashes and raw observations
remain in metadata reports; package contents, passphrases and identity values
are not exported. Password/KDF policy and the production Rust launcher remain
separate requirements.
