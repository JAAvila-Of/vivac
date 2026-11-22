# What it costs

The [performance pillar](PILLARS.md) gives a read **50 ms** and a write
**5 ms**. What follows is every number behind the summary in the
[README](../README.md#what-it-costs), including measured budget misses.

Measured on **18 September 2026** at ten thousand nodes, 200 calls per cell,
p50 / p99 in milliseconds, on a tree with its derived index in place — which
is what a tree has after the first read of it. The CLI columns start a fresh
process every time and include what that costs; the MCP columns are a
resident server, which is how an agent calls.

**It is measured four times over, because two things were each hiding behind
one number.** The first is the shape of the tree: what `brief`, `open` and
`tree` cost is governed less by how many nodes a tree holds than by how many
are still open, so each table below is the same ten thousand nodes with 134
open fronts against 3,023. The second is the machine, which the table this
replaces never named at all.

## Reading

**Linux**, a container built on `rust:1.89-bookworm`, twelve cores, kernel
5.15 under WSL2 — not bare metal, and slower than the runner CI uses:

| | CLI, 134 open | CLI, 3023 open | MCP, 134 open | MCP, 3023 open |
|---|---|---|---|---|
| `brief` | 11.7 / 15.5 | 14.6 / 19.0 | 0.3 / 0.5 | 3.6 / 5.3 |
| `why` | 15.2 / 18.8 | 14.3 / 17.3 | 6.0 / 7.1 | 3.9 / 5.2 |
| `open` | 15.7 / 20.4 | 14.9 / 20.1 | 3.2 / 4.2 | 20.3 / 23.1 |
| `find` | 17.5 / 19.9 | 16.8 / 18.9 | 6.2 / 8.8 | 5.8 / 7.0 |
| `tree` | 15.7 / 22.9 | 18.9 / 22.1 | not a tool | not a tool |

**Windows 11**, same trees, same sources, a working machine with a browser on
it that would not close:

| | CLI, 134 open | CLI, 3023 open | MCP, 134 open | MCP, 3023 open |
|---|---|---|---|---|
| `brief` | 17.9 / 46.7 | 20.9 / 52.7 | 0.5 / 0.7 | 3.1 / 4.6 |
| `why` | 19.9 / 48.5 | 20.4 / 49.8 | 6.3 / 7.7 | 4.6 / 5.8 |
| `open` | 20.1 / 49.2 | 22.1 / 52.3 | 3.3 / 4.2 | 22.1 / 27.1 |
| `find` | 23.9 / 52.8 | 24.3 / 55.9 | 7.0 / 9.6 | 6.9 / 8.1 |
| `tree` | 20.8 / 49.7 | 26.0 / 57.0 | not a tool | not a tool |

Read `why` against `open` on the MCP columns and the first variable stands on
its own: `why` barely moves between the two shapes, because a lineage is
bounded by depth, while `open` goes from 3.2 to 20.3 ms out of the same ten
thousand nodes.

**Linux meets the 50 ms a read is given, tail included. Windows does not, and
what misses is worth naming.** Every CLI row there has about the same p99,
near 50, while the medians sit between 18 and 26. A tail that is the same
across five commands whose medians differ is not the tree's — it is what
starting a process costs on that machine, and it measured 44 to 48 ms there
before any of this work. An agent does not pay it: the MCP column is the same
tree read through a server that is already running.

> The tree this project keeps of itself is 38% open. Whether a tree stays that
> open on the way to ten thousand nodes is still not measured, and saying so
> costs less than assuming it either way.

## Writing

**A write does not grow with the tree** — the server appends against the tree
it is already holding — but it does grow with how many repositories a lane
declares, because each write reads one `HEAD` per repository inside the lock.
The same day and the same two machines, two writers arriving at a realistic
rate, p99 in milliseconds for zero, one, five and ten repositories:

| | 0 | 1 | 5 | 10 |
|---|---|---|---|---|
| Linux | 3.9 | 4.0 | 5.2 | 4.3 |
| Windows | 9.2 | 9.4 | 11.4 | 14.6 |

**The 5 ms a write is given is met on Linux and missed on Windows**, and the
shape of the miss says where it comes from: the tail is already there with no
repositories declared at all, where the work itself takes 1.4 ms. It belongs
to the filesystem rather than to this program, which is a reason to publish it
rather than to leave it out.

## Local artifact custody

Measured during implementation on **11 October 2026**, with Rust
`1.94.1 (e408947bf 2026-03-25)`, on Windows and an external NTFS fixture.
These measurements do not replace the older ordinary-write results above.

The ignored Rust benchmark uses 10,000 prior distinct artifact receipts and
100 new receipt commits. It excludes retaining file bytes, which happens
before the write lock, and includes the lock, validation, actual append,
response and unlock. Its synthetic receipts do not demonstrate file capture.
The release test binary also includes the test allocation counter.

| Initial release test measurement | Result |
|---|---:|
| Complete receipt commit, p50 | 6.2101 ms |
| Complete receipt commit, p99 | 13.2288 ms |
| Complete receipt commit, maximum | 13.4189 ms |
| Complete receipt commit, peak auxiliary heap | 30,230 bytes |
| Ordinary node note baseline, p99 | 8.2353 ms |
| Resident `Project.write` receipt route, p99 | 7.3864 ms |

**This initial receipt commit did not meet the p99 < 5 ms budget on this machine.**
The ordinary-write baseline and resident route also missed it. The resident
route used 10,200 prior receipts and one producer; its peak auxiliary heap was
10,430 bytes. Neither result authorizes skipping
integrity checks or increasing the budget. Debug probes on both C: and H:
also missed the budget; they locate costs but are not release compliance
measurements.

A later Windows release run retained an authoritative read-only log handle
while opening a fresh writer for every append. With the same 10,000-receipt,
100-commit fixture, complete receipt p50 was 0.337 ms, p99 was 0.5822 ms and
maximum was 0.5847 ms; peak auxiliary heap was 30,234 bytes. The resident
`Project.write` route measured p99 at 0.6195 ms. A node note in the capture
context measured p99 at 0.4813 ms; this is not a measurement of ordinary
indexed CLI startup. These stable-log measurements meet the receipt budget.

A separate concurrent run appended one valid external node note before each
of 100 receipt commits, outside the timer and allocation counter. It verified
all note and receipt sequences, but measured receipt p99 at 104.1996 ms and
peak auxiliary heap at 10,615,890 bytes. It failed both budgets: the CLI
rebuilt the tree under the lock when the log changed. The stable-log results
do not establish compliance for this concurrent case.

After replacing the CLI refold under the lock with a complete bounded tail
refresh, a release run measured the same 100 external-note/receipt pairs:
p50 0.6039 ms, p99 0.8001 ms, maximum 1.1469 ms and peak auxiliary heap
614,503 bytes. All pairs and their sequences were verified before the budget
assertions. The stable-log route measured p50 0.4316 ms, p99 0.6925 ms,
maximum 0.6983 ms and peak auxiliary heap 30,234 bytes. Both runs meet the
receipt time and auxiliary-memory budgets in this Windows environment.

The resident MCP receipt route was measured separately with the same 100
external-note/receipt pairs. Complete `Project.write`, including its stale
refresh, append, response, unlock and resident log bookkeeping, measured p50
0.654 ms, p99 1.0764 ms, maximum 5.1692 ms and peak auxiliary heap 10,305
bytes. Both the resident log and disk log retained every pair and sequence.
The maximum exceeded 5 ms and remains included; the budget is a p99 bound.
This measures the resident receipt route, excluding initial project loading,
retaining file bytes and MCP tool-envelope serialization.

Functional tests also measured confirmation at the exact refresh limits:
65,536 bytes with two events peaked at 614,562 auxiliary bytes; 64 events
with 12,472 bytes peaked at 620,452 bytes. These tests verify the memory bound,
not the receipt p99. The refresh rejects larger or incomplete changes rather
than applying a prefix or rebuilding the tree under the lock.

Allocation tests count additional live heap on the calling thread after
loading the tree and constructing the request. They do not measure process
RSS, other-thread allocations or Git child-process memory. Git preflight
additionally uses one short-lived thread requesting a 128 KiB stack; its
prefix output is limited to 4 KiB with an 8 KiB read buffer. Receipt commits
and the measured store check do not start that thread. A store check hashes
all 10,000 retained objects in its fixture even
when it displays only 20 details; its measured peak auxiliary heap is
1,468,048 bytes. Reading one metadata page over resident MCP peaks at 10,470
bytes. The streaming content guard peaks at 262,144 bytes of auxiliary heap;
its separately bounded stack buffer is 131,072 bytes.

Reproduce the receipt measurement with:

```sh
cargo clean -p vivac --release
cargo test --locked --release measure_one_hundred_complete_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --nocapture --test-threads=1
cargo test --locked --release measure_one_hundred_receipt_commits_after_external_writes -- --ignored --nocapture --test-threads=1
cargo test --locked --release measure_one_hundred_resident_receipt_commits_after_external_writes -- --ignored --nocapture --test-threads=1
```

For complete file capture, `tools/measure-artifact-custody.py` runs the Rust
binary in fresh processes against exact 1 MiB and 10 MiB synthetic UTF-8 files.
It passes a 280-character description and verifies that description, retained
sizes and digests after every sample. The default fixture
is a Git repository, so custody preflight checks are included. Use an external
temporary directory: the script refuses ancestors containing an existing
Vivac project. Record the binary profile, compiler and verified filesystem:

```sh
python tools/measure-artifact-custody.py target/release/vivac --samples 100 --profile release --rust-build "<rustc --version>" --filesystem "<verified filesystem>" --output /external/path/capture-results.json
```

The script reports every sample and exits nonzero if either capture budget
is missed. A passing unit-test exit code is not proof of a timing budget;
the receipt benchmark explicitly reports its timing result.

On the same date, the production release binary was measured on Windows 11
10.0.26200, on C: NTFS, with the Git fixture and 100 fresh processes per size.
The binary SHA-256 was
`d151aa5490c6785a3c017fba0d0f7855a45fce044fe29006c07eabfd942c4d86`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 359.88765 | 1493.1933 | 1502.4693 | misses 500 ms |
| 10 MiB | 700.06395 | 840.0416 | 909.9241 | meets 2000 ms |

The first three 1 MiB captures were its three slowest samples. They are
included in the result; subsequent speed does not erase the cold calls a
person can encounter. The 10 MiB phase ran after the 1 MiB phase in the same
fixture. The script exited 1 because the 1 MiB budget was missed.

After the authoritative capture loader and fresh append changes, another
production release run used the same Windows build, C: NTFS, Git fixture and
100 fresh processes per size. Its binary SHA-256 was
`1e199b505ea87eac89176f775f0942560e3772010e717c26a3d3eb49c51affea`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 358.8445 | 486.5926 | 544.7317 | meets 500 ms |
| 10 MiB | 783.96445 | 1482.0935 | 1520.8508 | meets 2000 ms |

All initial samples are included. The 1 MiB maximum exceeded 500 ms; the
budget is a p99 bound, not a guarantee for every call. The 10 MiB phase again
followed the 1 MiB phase in the same fixture. This run does not cover writes
from another process during capture.

A final Windows production run after the bounded concurrent-refresh changes
used the same compiler, C: NTFS, Git fixture and 100 fresh processes per size.
Its binary SHA-256 was
`fd78df8d11355109a2f3e3c698a2788bd48157abeb3b9086a1af028313f943c0`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 369.42955 | 547.2441 | 598.3838 | misses 500 ms |
| 10 MiB | 757.23825 | 1089.0896 | 1140.9693 | meets 2000 ms |

Every sample remains included. This run exited 1: passing receipt commits and
the earlier complete-capture run do not demonstrate compliance for this
production binary. The 10 MiB phase followed the 1 MiB phase in one fixture.
Another Rust 1.88 Cargo workspace test process was present on the machine
during this run. This observation does not establish the cause of the miss
and does not remove any samples from the result.

The next Windows run parallelized the independent prefix and ignore queries
at each Git preflight, retaining every validation boundary. It used the same
compiler, filesystem, Git fixture and 100 fresh processes per size. Its
production binary SHA-256 was
`541469ee6aa0cb88a311f95224cb1df4a248dae7fb2981815f4191f9a0b13682`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 299.6059 | 525.355 | 542.7037 | misses 500 ms |
| 10 MiB | 671.7452 | 913.2691 | 970.2335 | meets 2000 ms |

All samples remain included. This run exited 1. The lower median does not
demonstrate compliance with the complete-capture p99 budget.

The Linux run used Rust 1.89.0 (`29483883e`, 2025-08-04), a Debian container
under WSL2 kernel 5.15.167.4 and overlayfs, with a normal user rather than
container root. It used the same Git fixture and 100 fresh processes per size,
without dropping initial samples. The production release binary SHA-256 was
`09412d11e424f9d5a66439b6a438ff6e35da767b995075d6f753bbef48427cb0`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 57.119 | 69.568 | 71.025 | meets 500 ms |
| 10 MiB | 373.921 | 420.400 | 435.691 | meets 2000 ms |

The Linux release test benchmark, with 10,000 prior receipts and 100 commits,
measured complete receipt p99 at 0.051751 ms and the resident `Project.write`
route at 0.085406 ms. The ordinary note baseline was 0.055137 ms. The receipt
benchmark's peak auxiliary heap was 30,214 bytes. These results meet the
receipt budget in this environment. They do not establish performance on a
native Linux filesystem, macOS or Windows. This Linux run preceded the later
Windows capture-loader changes and is not a verification of those changes.

A later Linux run included the bounded capture loader and concurrent tail
refresh. It used the same Rust 1.89.0 compiler, normal container user,
overlayfs and 100-process Git fixture. The production binary SHA-256 was
`8b5dd20b3189ed71a7ed9538530b0adc7740678504b51efc65310ab1294e6475`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 58.291004 | 101.216669 | 113.721984 | meets 500 ms |
| 10 MiB | 386.516065 | 448.007066 | 506.43832 | meets 2000 ms |

The stable receipt benchmark measured p99 0.170201 ms and peak auxiliary heap
30,214 bytes. With one external note before each of 100 commits, the CLI
receipt route measured p99 0.080947 ms and 614,533 bytes; resident
`Project.write` measured p99 0.114723 ms and 10,351 bytes. Every note and
receipt sequence was verified. These runs included all samples and met both
receipt budgets; they preceded the parallel Git-query change.

After that Git-query change, the Linux fixture passed again with the same
compiler and filesystem and 100 fresh processes per size. Its production
binary SHA-256 was
`ac9716b01218a6e2441f43e24897a946f1c0717e1babb68b82cf47b65383cc0c`.

| Complete capture | p50 (ms) | p99 (ms) | Maximum (ms) | Budget |
|---|---:|---:|---:|---|
| 1 MiB | 53.409253 | 78.09799 | 82.162055 | meets 500 ms |
| 10 MiB | 385.74916 | 465.95884 | 472.006428 | meets 2000 ms |

All samples remain included. This container result does not establish
compliance on Windows or native macOS.

### Artifact web metadata and downloads

The web inventory uses the same resident LIST metadata construction as CLI
and MCP. Run its release benchmark explicitly:

```sh
cargo test --locked --release --bin vivac -- --ignored --exact artifacts::tests::measure_resident_inventory_with_ten_thousand_artifacts_and_decisions --nocapture
cargo test --locked --release --bin vivac maximum_download_uses_bounded_auxiliary_memory_and_leaves_no_temporary -- --nocapture
```

On Windows on 12 October 2026, 100 samples with 10,000 retained artifacts,
10,000 decisions and a page of 20 measured p50 24.358 ms, p99 30.835 ms and
218,887 bytes of peak auxiliary heap. All samples are included. The measured
region includes replay validation, producer metadata and decision references
to any retained revision. Context loading, HTTP handling, serialization and
file-byte inspection are excluded; this is not an end-to-end page latency.
The resident metadata budget is p99 below 50 ms and auxiliary heap at most
2 MiB, excluding the loaded tree.

The release download test copies and verifies a 10 MiB retained object,
then reads the same unnamed handle. It asserts at most 2 MiB of auxiliary
heap, including a 128 KiB allowance for the Git-query thread, and no remaining
temporary name. A separate unit test verifies that changing the original
object after preparation does not change bytes read from the download handle.
The Windows run passed. This test does not measure browser memory or network
latency and does not establish the separate capture latency budget.

The corresponding Linux run on the same date used Rust 1.89.0, a normal
container user and overlayfs. With the same 100-sample resident fixture, it
measured p50 19.605422 ms, p99 34.173547 ms and 218,887 bytes of peak auxiliary
heap. The 10 MiB release download test also passed. These container results
do not establish native macOS compliance or clear the earlier Windows
capture latency result.

## Static bundle custody

Measured on **14 October 2026**, with Rust
`1.94.1 (e408947bf 2026-03-25)`, Windows x86_64 and fixtures on the
system drive's NTFS user-temp directory. The release test binary uses the
test allocation counter. Each case has 100 measured samples; p50 and p99
use nearest rank. These results do not establish Linux or macOS budgets.

| Complete retention | Members | Total bytes, including manifest | p50 / p99 / max (ms) | Peak auxiliary heap (bytes) |
|---|---:|---:|---:|---:|
| 1 MiB | 1 | 1,048,576 | 88.7432 / 157.0263 / 161.9411 | 198,710 |
| 10 MiB | 1 | 10,485,760 | 746.4015 / 917.4501 / 924.6102 | 198,710 |
| Many small members | 64 | 1,048,576 | 216.2113 / 382.3627 / 386.1025 | 265,640 |

Retention includes complete UTF-8 and sensitive-content inspection, metadata
and source-identity checks, Git guards, manifest construction, file synchronization
and native exclusive publication. The input uses synthetic UTF-8 bodies and
64 KiB streaming chunks. Fixtures are outside a Git repository; these numbers
do not measure repository-query overhead. Fixture setup and removal of each measured object are
outside the timer. Adding the 128 KiB stack allowance keeps all three cases
below the 2 MiB auxiliary-memory ceiling. Both 1 MiB cases meet the 500 ms p99
budget; the 10 MiB case meets 2,000 ms. Retention excludes the tree receipt.

A separate benchmark commits 100 receipts over 10,000 prior distinct bundle
receipts, with one producer. Its p50 / p99 / maximum were
**0.4074 / 0.6210 / 0.6593 ms**, with **33,498 bytes** peak auxiliary heap.
Each synthetic receipt describes 64 members, a 32 KiB manifest and a 1 MiB
delivery. The timer includes the lock, refresh, guards, expected-revision check,
actual append, projection, JSON response construction and unlock. Fixture setup,
context loading, prepared metadata, retained-byte copying and stdout serialization
are excluded. The test checks all recorded event sequences and folded receipts
after measurement. It meets the strict 5 ms p99 receipt budget and 2 MiB memory
ceiling; it does not measure process startup, HTTP or member retention.

Reproduce from the repository with the shared Rust command coordinator where
required by the development environment:

```sh
cargo clean -p vivac --release
cargo test --locked --release --bin vivac artifacts::bundle::tests::retained_bundle_release_benchmark -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --release --bin vivac artifacts::tests::measure_one_hundred_complete_bundle_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --exact --nocapture --test-threads=1
```

## Described artifact revisions

Measured on **15 October 2026**, in release mode on Windows with Rust
`1.94.1` and external NTFS fixtures. Each receipt benchmark uses 10,000 prior
V2 receipts and 100 new commits, all with 280-character descriptions. Earlier
artifact receipt results above used the previous event format.

| Receipt route | p99 (ms) | Peak auxiliary heap (bytes) |
|---|---:|---:|
| Complete file receipt commit | 0.7397 | 35,084 |
| Complete bundle receipt commit | 0.7027 | 36,528 |
| Capture context after external writes | 1.0118 | 617,063 |
| Resident `Project.write` after external writes | 1.0206 | 12,461 |

All samples remain included. Each route meets the strict 5 ms p99 budget and
2 MiB auxiliary-memory ceiling in this environment. These measurements include
receipt validation and persistence, but exclude retaining source bytes and
process startup. The allocation counter measures additional live heap on the
calling thread, not process RSS or Git child-process memory.

The complete-file script used the freshly built release executable, a temporary
Git repository on NTFS and 100 samples per size, each with a 280-character
description. It verified every returned description, retained size and digest.

| Complete file capture | p50 (ms) | p99 (ms) | Maximum (ms) | p99 budget (ms) |
|---|---:|---:|---:|---:|
| 1 MiB | 261.0702 | 311.4429 | 339.7010 | 500 |
| 10 MiB | 625.66645 | 866.5226 | 875.4242 | 2,000 |

Both sizes meet their time budgets, with all samples included. These timings
include CLI process startup, custody preflight, synchronized local retention
and the receipt. This script does not measure memory and does not establish
capture performance on other operating systems or filesystems.

## SQLite application retention with a fixed runtime

Measured on **15 October 2026**, Windows x86_64, local NTFS temporary
directories outside Git, with rustc **1.94.1 (e408947bf 2026-03-25)** and a
fresh release build. Each case has 100 measured captures. Fixture preparation,
warmup, object removal and receipt append are outside the capture timer; source
opening, guards, logical SQLite export, complete runtime materialization and
verification, synchronization and exclusive publication are inside it. All
samples remain included; p99 is the 99th sorted sample.

Each complete revision contains HTML, JavaScript, one normalized database,
the fixed runtime and a manifest. The runtime is 1,357,427 bytes, including its
license notices. Complete revision sizes below include every member and the
manifest. The second case includes the runtime plus exactly 1 MiB of other
content; runtime bytes are never discounted.

| Complete revision | Bytes | p50 (ms) | p99 (ms) | Maximum (ms) | Strict p99 ceiling (ms) |
|---|---:|---:|---:|---:|---:|
| Minimum fixture | 1,366,756 | 37.2750 | 234.2437 | 250.9065 | 500 |
| Runtime plus 1 MiB | 2,406,003 | 232.9526 | 364.8883 | 394.5581 | 500 |
| 10 MiB | 10,485,760 | 1,424.9915 | 1,998.8241 | 2,024.4926 | 2,000 |

All three p99 values pass in this run. The 10 MiB margin is only **1.1759 ms**,
and its maximum exceeds two seconds. This is one machine's measurement, not
evidence of a broad margin, repeatability across machines or another filesystem.

| Auxiliary memory | Minimum (bytes) | Runtime plus 1 MiB (bytes) | 10 MiB (bytes) |
|---|---:|---:|---:|
| Peak live Rust heap | 203,264 | 203,264 | 203,264 |
| Peak SQLite heap | 178,488 | 222,000 | 230,736 |
| Stack allowance | 131,072 | 131,072 | 131,072 |
| Embedded compressed runtime | 453,701 | 453,701 | 453,701 |
| Conservative total | 966,525 | 1,010,037 | 1,018,773 |

The totals conservatively add the separate Rust and SQLite peaks, a 128 KiB
stack allowance and the embedded compressed bytes. They fit the 2 MiB ceiling;
the full runtime body is streamed rather than allocated. These counters do not
measure process RSS or browser memory.

The Windows capture retains the newly written runtime handle through complete
byte verification. Revalidating its name uses a separate attributes-only handle
and the existing metadata and identity comparisons. That second handle cannot
read content and does not pin the name. Historical revisions and author files
retain their existing open and verification paths.

The separate complete SQLite receipt benchmark used 100 samples after 10,000
prior distinct receipts, with 64-member, 32 KiB manifest metadata describing a
2 MiB delivery. Its p99 was **0.6304 ms**, maximum **0.6421 ms**, and peak
auxiliary heap **44,365 bytes**. It includes lock, refresh, guards, compare-and-swap,
append, projection, response construction and unlock. Retained-byte copying,
fixture preparation, tree loading and stdout serialization are excluded.

Reproduce from the crate directory, coordinating each Rust command separately
with the shared Windows RustWork launcher where required:

```text
cargo clean -p vivac --release
cargo test --locked --release --bin vivac artifacts::bundle::tests::retained_sqlite_application_release_benchmark -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --release --bin vivac artifacts::tests::measure_one_hundred_complete_sqlite_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --exact --nocapture --test-threads=1
```

## Read-only WAL application retention

Measured on **20 October 2026**, Windows 11 x86_64, on an AMD Ryzen 5 5600G
(6 cores, 12 logical processors), using Rust and Cargo **1.94.1** with the
MSVC target. The source commit was
`d54ff6a37c89d590efbf88027c223e3a2a847149`. A release package clean removed
the previous executables; the test target was then freshly compiled. Its SHA-256
was `8ab70a4901ff05530515c7b1b58c32562816fd9b67504505db628b696c1d2e52`.
The checkout was on H: NTFS and the synthetic fixtures on C: NTFS, outside Git.

Each case used **100 captures** through the complete product capture path,
including source opening and identity checks, native WAL leases, logical
inspection and export, runtime materialization and verification, synchronization
and publication. Fixture preparation, calibration, cleanup and receipt append
were outside the timer. All samples remain included; p99 is the 99th sorted
sample. The producer connection stayed open, and database, WAL and SHM bytes
were checked for invariance after each case.

Each retained revision contains HTML, JavaScript, a standalone logical database,
the fixed 1,357,427-byte runtime and its manifest. Database-heavy fixtures
checkpoint the bulk data before a final pending update; WAL-heavy fixtures
checkpoint before the bulk transaction. Their source DB/WAL sizes were
8,192/4,152 bytes for the minimum, 929,792/4,152 and 8,044,544/4,152 bytes for
the database-heavy cases, and 8,192/939,392 and 8,192/8,095,832 bytes for the
WAL-heavy cases. The benchmark emits source digests for each generated fixture.

| Complete application | Input bytes | Retained bytes | p50 (ms) | p99 (ms) | Maximum (ms) | Strict p99 ceiling (ms) |
|---|---:|---:|---:|---:|---:|---:|
| Minimum | 1,369,867 | 1,366,937 | 42.3685 | 173.4051 | 243.8824 | 500 |
| Runtime plus 1 MiB, database-heavy | 2,408,925 | 2,406,003 | 176.9497 | 245.9534 | 247.5047 | 500 |
| Near 10 MiB, database-heavy | 10,485,760 | 10,482,841 | 1,271.7121 | 1,350.8699 | 1,355.6966 | 2,000 |
| Runtime plus 1 MiB, WAL-heavy | 2,422,565 | 2,406,003 | 178.9671 | 252.6409 | 267.7621 | 500 |
| Near 10 MiB, WAL-heavy | 10,485,760 | 10,427,513 | 1,279.0820 | 1,482.3390 | 1,585.5744 | 2,000 |

Both input and retained size limits apply. The two largest fixtures fill the
10 MiB input limit; their retained revisions are respectively 2,919 and 58,247
bytes below the retained limit. These are measured sizes, not a discounted
runtime or an increase in the production limit.

| Auxiliary memory | Minimum (bytes) | Runtime plus 1 MiB, either layout (bytes) | Near 10 MiB, either layout (bytes) |
|---|---:|---:|---:|
| Peak live Rust heap | 204,370 | 204,370 | 204,370 |
| Peak SQLite heap | 211,456 | 254,968 | 263,704 |
| Stack allowance | 131,072 | 131,072 | 131,072 |
| Embedded compressed runtime | 453,701 | 453,701 | 453,701 |
| Conservative total | 1,000,599 | 1,044,111 | 1,052,847 |

All five cases meet their latency and 2 MiB auxiliary-memory budgets in this
run. The totals add the separate Rust and SQLite peaks, the stack allowance
and the compressed runtime; the complete runtime body is streamed. They do not
measure process RSS or browser memory. A direct Windows CPU sample was 78%
before the coordinator wait and 34% during the run, after the fourth case.
There was no exact start sample or continuous load trace. These observations
establish neither sustained idle conditions nor performance on another machine,
filesystem or operating system. They exclude the receipt, whose ordinary write
budget still applies separately.

Reproduce from the crate directory, coordinating each Rust command separately
with the shared Windows RustWork launcher where required:

```text
cargo clean -p vivac --release
cargo test --locked --release --bin vivac artifacts::bundle::wal_bench::retained_wal_sqlite_application_release_benchmark -- --ignored --exact --nocapture --test-threads=1
```

## Reviewed web-media retention

Measured on **19 October 2026**, on Windows x86_64, with Rust 1.94.1
(`e408947bf`), Cargo 1.94.1 and the release profile. Fixtures used the local
NTFS temporary volume, separately from the repository volume. Each case has
100 complete captures. Each benchmark runs with one test thread under its own
shared RustWork lease.
The benchmarks construct their fixtures outside the timer and remove each
retained object after its measured capture.

| Fixture | Complete revision bytes | p99 / max (ms) | Peak auxiliary heap (bytes) |
|---|---:|---:|---:|
| PNG, near 1 MiB | 1,042,759 | 37.18 / 93.55 | 363,124 |
| PNG, near 10 MiB | 10,480,084 | 211.39 / 237.79 | 366,708 |
| High-expansion RGBA16 PNG, at most 1 MiB | At most 1,048,576 | 36.68 / 48.88 | 375,988 |
| Two MP4 files, near 1 MiB | 1,047,294 | 341.25 / 376.20 | 201,682 |
| Two MP4 files, near 10 MiB | 10,484,480 | 955.09 / 1,042.09 | 201,682 |

PNG fixtures use deterministic noise, or a uniform 2048 by 2047 RGBA16 image
for the high-expansion case. That case expands to 33,540,095 scanline bytes,
including filter bytes, close to the complete revision's 32 MiB limit. Each
MP4 fixture contains two videos of 4096 samples each, with one VCL and one
UTF-8 user-data SEI NAL per sample. Together they exercise the shared limits of
8192 samples and 16,384 sample NAL units; the larger case increases SEI text.
Codec configuration remains separately restricted to one SPS and one PPS per
video. These are inspection fixtures, not browser playback measurements.

The measured capture includes source opening, format and accessible-text
inspection, digest comparisons against the review declaration, complete member
copying, manifest construction, identity checks, synchronization, verification
and exclusive native publication. It excludes tree loading, receipt append,
CLI startup and browser execution. It does not measure standalone check or
restore latency. Add a 128 KiB stack allowance to each heap value: the largest
combined auxiliary total is 507,060 bytes, below 2 MiB. Every measured p99 fits
its 500 ms or 2,000 ms ceiling in this run; these results do not establish
performance on another filesystem or machine.

Reproduce from the crate directory, coordinating each Rust command separately
with the shared Windows RustWork launcher where required:

```text
cargo clean -p vivac --release
cargo test --locked --release --bin vivac artifacts::bundle::tests::retained_reviewed_media_release_benchmark -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --release --bin vivac artifacts::bundle::tests::retained_reviewed_video_release_benchmark -- --ignored --exact --nocapture --test-threads=1
```

## Larger captures and explicit custody policy

Measured on **20 October 2026**, from commit
[`4a18bd2`](https://github.com/JAAvila-Of/vivac/commit/4a18bd21962d84a3251e97299eb2a893f72b9dc3), using a freshly built
release Rust test executable with the test allocation counter. The compiler was
`rustc 1.94.1 (e408947bf 2026-03-25)`. The machine ran Windows 11 Enterprise
10.0.26200, build 26200, on an AMD Ryzen 5 5600G with 12 logical processors.
Both C: and H: used NTFS; temporary source and retained fixtures were on C:,
and the executable was on H:.

The release test executable's SHA-256 was
`6bf83075d3f64772909d4348ba086ac07bb467ac3575c736ca6c654cc3e9d74d`.

Each capture cell has **100 samples**, including initial calls. Percentiles
use the nearest rank. The timer covers complete backend retention, including
inspection, copying, hashing and publication of the retained object. The policy
is passed to the backend; its project-configuration lookup is not timed here.
The timer also excludes project loading, receipt commits, fixture preparation,
source verification and cleanup. These are backend measurements, not complete
CLI or browser latency.

Files contain valid synthetic HTML. Static bundles contain an HTML entrypoint
and two CSS files; their exact retained size includes the generated manifest.
The permissive fixtures place a synthetic sensitive marker near the end of
the HTML body, before its closing tags. Each sample verifies the returned
inspection, including detection and the review requirement. Bundle calibration
also verifies the inspection in the retained manifest, outside the timer.
Source hashes are checked outside the timer. Repeated captures use the same
prepared source; these are not cold-cache results.

| Profile | Policy | MiB | p50 / p99 / maximum (ms) | Peak Rust heap (bytes) | Heap + stack allowance (bytes) |
|---|---|---:|---:|---:|---:|
| File | Strict | 1 | 46.634 / 67.893 / 70.763 | 131,219 | 262,291 |
| File | Strict | 10 | 425.008 / 561.778 / 562.840 | 131,219 | 262,291 |
| File | Strict | 100 | 4,406.394 / 6,069.747 / 6,628.525 | 131,219 | 262,291 |
| File | Allow insecure | 1 | 48.065 / 66.076 / 113.241 | 131,266 | 262,338 |
| File | Allow insecure | 10 | 408.333 / 460.506 / 504.677 | 131,266 | 262,338 |
| File | Allow insecure | 100 | 4,846.141 / 11,254.579 / 11,487.514 | 131,266 | 262,338 |
| Static bundle | Strict | 1 | 95.158 / 122.907 / 206.339 | 201,368 | 332,440 |
| Static bundle | Strict | 10 | 765.520 / 1,143.051 / 1,517.166 | 201,368 | 332,440 |
| Static bundle | Strict | 100 | 7,690.778 / 12,207.946 / 14,008.264 | 201,368 | 332,440 |
| Static bundle | Allow insecure | 1 | 95.144 / 144.162 / 146.970 | 201,415 | 332,487 |
| Static bundle | Allow insecure | 10 | 790.844 / 1,173.457 / 1,186.673 | 201,415 | 332,487 |
| Static bundle | Allow insecure | 100 | 7,764.007 / 13,679.653 / 14,231.647 | 201,415 | 332,487 |

All twelve capture p99 values meet their respective bounds: 500 ms at 1 MiB,
2,000 ms at 10 MiB and 20,000 ms at 100 MiB. The auxiliary totals remain below
2 MiB under the accounting described below. Each ignored capture test passed.

Peak Rust heap is measured by the test allocator. The auxiliary total adds a
fixed 128 KiB stack allowance; stack usage is not directly sampled. The text
copy uses a 64 KiB + 3-byte local buffer, and these bundles have shallow
directory traversal. The result applies to these fixtures and excludes RSS,
filesystem caches and child-process memory. These fixtures are outside Git.

CPU samples use global Windows system counters at one-second intervals over
each whole native test process, including fixture preparation and cleanup.
They can include unrelated programs and do not isolate the capture timer.
Their reported mean is an unweighted interval mean, not a time-weighted mean.

| Profile and policy | MiB | Global CPU mean / p50 / maximum (%) | Intervals |
|---|---:|---:|---:|
| File, strict | 1 | 22.19 / 20.74 / 30.08 | 6 |
| File, strict | 10 | 17.07 / 15.10 / 34.51 | 44 |
| File, strict | 100 | 23.92 / 19.14 / 99.18 | 463 |
| File, allow insecure | 1 | 29.62 / 27.99 / 41.67 | 6 |
| File, allow insecure | 10 | 16.40 / 14.84 / 31.25 | 42 |
| File, allow insecure | 100 | 28.55 / 25.65 / 95.96 | 519 |
| Static bundle, strict | 1 | 14.79 / 14.99 / 19.05 | 11 |
| Static bundle, strict | 10 | 19.53 / 13.54 / 76.69 | 83 |
| Static bundle, strict | 100 | 24.70 / 18.25 / 100.00 | 828 |
| Static bundle, allow insecure | 1 | 14.93 / 14.84 / 17.97 | 11 |
| Static bundle, allow insecure | 10 | 18.83 / 17.06 / 44.14 | 88 |
| Static bundle, allow insecure | 100 | 30.86 / 24.09 / 100.00 | 848 |
| Permissive receipt | — | 68.59 / 68.59 / 68.59 | 1 |

The receipt process was shorter than one sampling interval. Its single CPU
observation does not establish what caused the receipt latency. CPU samples
were sorted for ranking; they are not a chronological trace.

The separate receipt fixture starts with 10,000 distinct prior strict artifact
receipts and commits 100 new permissive receipts with 280-character
descriptions and synthetic object metadata. Its timer includes fresh policy
authority, the write lock, guards, the first required configuration upgrade,
append, response and unlock. It excludes copying retained bytes and loading
the project context. Its allocation figure is peak Rust heap, without an
observed stack measurement.

This receipt run measured p50 **0.967 ms**, p99 **6.5917 ms**, maximum
**8.4889 ms**, and peak Rust heap **44,486 bytes**. It completed all 100
commits, then failed the unchanged **p99 < 5 ms** assertion with exit code
101. **The capture results do not establish receipt-budget compliance.**
The failed receipt result is retained. The later diagnosis and candidate
measurement below do not establish its cause.

### Receipt phase diagnosis and candidate measurement

A fresh release test executable from
[`3da1637`](https://github.com/JAAvila-Of/vivac/commit/3da1637f3d2eb196dbc5fa91f62078d2688af6d0)
adds optional test-only phase instrumentation to the real commit route. Its
SHA-256 was
`4dcd1068f95bf729cd3570dfd72c4aca29aaf696323fb0aae4348bcfba1ccd84`.
Compiler, machine and filesystem were the same as above. Production builds
exclude the profiling hooks; the capture backends are unchanged.

One diagnostic run completed 100 commits over 10,000 prior receipts, retaining
every sample by index. It measured p50 / p99 / maximum at **0.6459 / 3.8097 /
5.3731 ms**, including its instrumentation. The first commit spent 2.7605 ms
in the append configuration-marker/write phase and took 3.8097 ms overall.
The second commit spent 4.6473 ms in the fresh policy lookup and took 5.3731 ms
overall. These locate elapsed time in that run; they do not prove the cause
of the earlier receipt tail. The diagnostic does not certify a budget.
Its whole-process global CPU observation was 20.00%, over one interval.

The six append subphases are nested within the capture's emit phase. Residual
time is total minus the five capture phases and unlock; the append subphases
are not subtracted again. Residual p50 / p99 / maximum was 500 / 1,300 /
7,400 ns, including profiling-wrapper overhead.

Exactly one subsequent compliance run used the same executable with phase
collection inactive and the original budget assertion unchanged. The test-only
hooks remain compiled and check their inactive state. All 100 commits are
included, including the first configuration upgrade:

| Complete permissive receipt | Result |
|---|---:|
| p50 | 0.594 ms |
| p99 | 3.4348 ms |
| Maximum | 6.3954 ms |
| Peak Rust heap | 44,486 bytes |
| Whole-process global CPU mean / p50 / maximum | 22.33 / 22.33 / 22.33% |
| CPU intervals | 1 |

This candidate run passed the **p99 < 5 ms** assertion with exit code 0. Its
maximum exceeded 5 ms and is retained: the budget is a percentile bound, not a
guarantee for every call. The earlier 6.5917 ms p99 remains above as a failed
observation. No production optimization was made between these runs. These
observations do not establish why their tails differ. The receipt timing and
memory exclusions above still apply.

To reproduce a capture cell, run it separately in release mode:

```sh
cargo clean -p vivac --release
cargo test --locked --release --bin vivac artifacts::policy_budgets::file_strict_one_mib -- --ignored --exact --nocapture --test-threads=1
```

Substitute `file` or `bundle`, `strict` or `allow_insecure`, and `one`, `ten`
or `hundred` in the test name to select each cell. Run the receipt case
separately:

```sh
cargo test --locked --release --bin vivac artifacts::tests::measure_permissive_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --exact --nocapture --test-threads=1
```

The optional phase diagnostic is separate from compliance:

```sh
cargo test --locked --release --bin vivac artifacts::tests::diagnose_permissive_receipt_commit_phases_with_ten_thousand_prior_artifacts -- --ignored --exact --nocapture --test-threads=1
```

## Context

Context is the budget that actually binds an agent, so the payloads are
measured the same way.

| | before | now |
|---|---|---|
| `vivac_open` over ten thousand nodes | 1,993,053 bytes | **599,012** |
| `why --json` on a node deep in a real tree | 86,894 bytes | **7,139** |
| a write over MCP, p99 | a full fold of the log | **0.6 ms**, flat in the size of the tree |

Across every node of three real trees, `why --json` went from 8.8, 6.7 and 5.7
times the weight of the prose to 1.5, 1.8 and 2.1. **A payload nobody asked
for costs the same context as a tool nobody calls.**

## Two things worth knowing

**The first ordinary read after an upgrade is slower, once.** The index is
derived, and an ordinary read that does not recognise its format folds the log
and writes a new one. Custody diagnostics and previews keep reads pure: they
fold again until an ordinary command such as `vivac brief` rebuilds the index.

**These numbers do not reconcile with the ones they replace, and cannot.**
That table named no machine, and the fixture behind it came from a generator
that exists nowhere any more — so its shape, which governs three of the five
rows, cannot be recovered to compare against. What replaced it is kept: the
script, the fixtures it builds from a fixed seed, and one file per run
recording what it measured and where.

## Continuous agent custody

Measured on **6 October 2026**, on this Windows development machine: 1,068
nodes, 5,217 events, eight native prompt references and eight current managed
destinations. Each cell has 10 warmups and 200 calls. The coordinator executable
was 314 seconds old before measurement. The derived index was regenerated
outside the timer; configuration, event and destination hashes stayed equal.

| Operation | p50 / p99 (ms) |
|---|---:|
| 0.19.0 prompt hook, no agents | 42.672 / 96.862 |
| Coordinator prompt hook, no agents | 56.661 / 132.688 |
| Coordinator prompt hook, eight agents in automatic mode | 64.294 / 125.710 |
| Resident MCP reconciliation, manual mode, no agents | 6.578 / 9.523 |
| Resident MCP reconciliation, automatic mode, eight agents | 17.876 / 21.972 |

The MCP calls exclude server startup and initialization. The hook calls include
process startup, reconciliation and the existing session nudge. Both resident
MCP tails fit 50 ms in this fixture; the complete hook does not. These are steady
cycles without import or materialization work, not latency claims for writes,
cold indexes, other platforms or ten thousand nodes. Configuration being current
is not evidence that a harness loaded an agent.

## Agent inventory and synchronization previews

Measured on **7 October 2026**, on Windows 11 AMD64 with 12 logical CPUs:
10,000 nodes, 200 open, fixture seed `20260908`, three unmanaged Claude Code
agents and both Claude Code and Codex configured. Plans target three new Codex
files with model `inherit` and effort `high`. The release executable was 517
seconds old before measurement, with Cargo idle. Each cell has 10 warmups and
200 measured calls; percentiles use the nearest rank.

| Operation | CLI p50 / p99 (ms) | Resident MCP p50 / p99 (ms) |
|---|---:|---:|
| Inventory | 22.276 / 69.523 | 13.480 / 19.973 |
| Synchronization plan | 26.573 / 73.284 | 16.282 / 29.318 |
| Current-version comparison | 21.231 / 68.242 | 11.838 / 14.005 |

CLI measurements include process startup; MCP measurements exclude server
startup and initialization. All three resident MCP tails fit 50 ms in this
fixture; complete CLI calls exceed it. These numbers do not establish the cause
of the CLI tail or performance on other machines.

A separate inventory call without the derived index took **67.606 ms**. That is
one observation, not a percentile. The read left the index absent; its original
bytes were restored outside the timer. Hashes of all 12 fixture files matched
before and after the complete run, and CLI/MCP response schemas matched.

These are pure inventory, new-agent planning and current-file comparison
measurements. They do not measure applying a plan, loading agents in a harness,
or interactive browser latency.

## Bounded opening snapshot preparation

Measured on **25 October 2026**, on Windows 11 AMD64 (build 26200), a Ryzen 5
5600G with 12 logical CPUs, using Rust 1.94.1 and the locked release profile.
The baseline is unmodified `2d263c6`; the candidate is the opening-consistency
working-tree patch on that revision. Both executables report `0.21.0`, so the
version string alone does not identify either build. Their SHA-256 digests are:

- Baseline: `f6e600d116281bdfc6f4def2760cb43f1e6ed5571a18960b1b5aba75fd71d390`.
- Candidate: `c6be512376df7804cb0cd8f79f5e4599589d2bf13cd72948be6e8a8133afcaed`.

`tools/measure-opening-context.py` creates an isolated synthetic project outside
existing trees, with manual custody and no native agent files. It alternates
both executables and their legacy, Codex and Claude-module producer modes on
the **same** fixture, rotating the six-cell order between samples. Each cell
has five warmups and 200 measured fresh processes. The first candidate legacy
opening is recorded separately and excluded from those percentiles; that is a
first-call observation, not a measurement of a cold machine.

Wall times include process startup, tree and index loading, reconciliation,
the opening receipt, serialization and pipe transfer. No startup time is
subtracted. The table uses the median for p50 and nearest rank for p99.

| Nodes / open fronts | Producer mode | Baseline p50 / p99 (ms) | Candidate p50 / p99 (ms) |
|---|---|---:|---:|
| 101 / 100 | Legacy hook | 14.683 / 26.121 | 14.592 / 26.881 |
| 101 / 100 | Codex | 14.702 / 29.755 | 14.578 / 26.811 |
| 101 / 100 | Claude module | 14.678 / 27.189 | 14.564 / 26.669 |
| 10,000 / 100 | Legacy hook | 111.519 / 165.165 | 110.477 / 170.531 |
| 10,000 / 100 | Codex | 113.044 / 158.312 | 111.607 / 152.070 |
| 10,000 / 100 | Claude module | 113.004 / 192.056 | 111.582 / 156.146 |

The CLI fixture is on **H: NTFS**. The 101-node case emitted the same 4,403 UTF-8
bytes in every cell; the 10,000-node case emitted 5,358 bytes. Each case appended
exactly 1,231 opening receipts. The helper verifies exact context equality, the unchanged
synthetic event-log prefix, every appended opening receipt, and both executable
digests after the run. Its JSON report retains every sample, maxima, fixture
hashes, executable ages and checkout metadata. Random identifiers from `init`
can change the complete fixture hash between runs; the graph shape and text
generation are fixed. Checkout metadata is recorded separately from the
executable digest and does not by itself attest a build's source.

These runs serialize managed Rust work and leave other user applications
running. They do not establish idle-host, other-filesystem or cross-platform
performance, nor do percentile differences identify their cause. The complete
opening has no separately established aggregate latency ceiling here; these
numbers do not certify the node-write or resident-read budgets by subtracting
startup or summing independently measured percentiles. The fixtures do not
measure custody imports, materialization, a second preparation attempt, or
native harness reception.

To reproduce, build the baseline and candidate separately with the same
toolchain and locked dependencies. Pass their actual executable paths through
`--binary` and `--baseline-binary`, with `--samples 200 --warmup 5`, a fixture
size through `--nodes` and `--fronts`, and `--temp-dir` outside existing trees.
Use `--output` to preserve the complete JSON report. The helper refuses
executables whose file modification times are less than 300 seconds old;
that safeguard does not prove when they were built or that the host is idle.

### Instrumented phase diagnostic

The opt-in release test
`session::tests::measure_opening_preparation_and_receipt` separately measures
context loading, real preparation and receipt recording after another context
appends a note. It uses independent fixtures on **C: NTFS**, with 100 open
root tasks, five warmups and 200 samples per size. The 10,000-node fixture closes
the remaining tasks. Both emit 4,292 context bytes. These are different shapes
from the CLI fixtures above.

| Nodes / open fronts | Load p50 / p99 (ms) | Prepare p50 / p99 (ms) | Receipt with refresh p50 / p99 (ms) |
|---|---:|---:|---:|
| 100 / 100 | 0.871 / 1.563 | 2.166 / 3.294 | 7.996 / 10.047 |
| 10,000 / 100 | 48.542 / 67.720 | 51.676 / 72.255 | 56.388 / 76.416 |

Both quantiles use nearest rank. Setup, the intervening append and assertions
are outside their phase timers. Preparation excludes the native input parser
and output envelope; receipt recording includes a real lock and refresh.
The test harness's `cfg(test)` counting allocator remains active, including in
release mode. These diagnostic timings neither certify production latency nor
establish a production budget violation. Do not subtract or add these phases to
the fresh-process measurements, which use production executables and different
fixtures. Neither experiment measures native model reception.

Build the release test executable with `cargo test --locked --release --bin vivac
--no-run`, then run that executable with the exact test name and
`--ignored --exact --nocapture --test-threads=1` after the same aging and workload
controls. A debug build refuses this diagnostic instead of reporting timings.

### Opening phases with the normal allocator

Measured on **25 October 2026**, on the Windows host above, with Rust 1.94.1,
the locked release profile and fixtures on **H: NTFS**. The reference already
contains the bounded-snapshot consistency changes; the candidate also reuses
the snapshot for custody, retains a validated receipt cursor and avoids
temporary metadata and lane-key copies. Both are patches on `2d263c6` and
report version `0.21.0`. Executable SHA-256 digests identify the builds:

- Reference: `681640359873381f49a9e2488aae7540d0c095b8a2f63c307f5d05b35ead0ec1`.
- Candidate: `1d4573db7fe220decd2450f32f5bc46920dd01a1152c0565fa7f2d54b7a4b1cd`.

These measurements identify that historical candidate. They do not cover the
subsequent full-prefix and physical-identity checks described in
[Instruction Delivery](INSTRUCTION-DELIVERY.md#shared-preparation-and-harness-adapters).

Two independent 10,000-node candidate runs stayed below **50 ms p99** for
combined loading and preparation and **5 ms p99** for receipts in every cell.
Their slowest combined p99 was **48.0521 ms**, leaving **1.9479 ms** below
50 ms. This is observed headroom in these fixtures, not a guarantee for other
workloads or a measurement of complete CLI latency.

`tools/measure-opening-phases.py` prepares a separate copy of the public crate
and injects `tools/opening-phases.rs` there. An ordinary locked release build
calls the real opening functions without `cfg(test)` or the counting allocator.
The distributed executable gains no profiling option. The helper records the
source inventory, copy and driver hashes, injection recipe, executable digest,
declared toolchain and filesystem, and every sample. It verifies the frozen
copy and executable before and after measurement. Checkout metadata at run time
is recorded separately from the source inventory used to prepare the copy.

Each run has four independent synthetic fixtures: index absent or available,
and a stable receipt or a receipt after one concurrent node note. The available
index is generated before warmup and can accumulate a tail during the run;
the absent-index cases do not generate one. Each fixture has 100 open root
tasks; the larger shape closes the remaining tasks. Every case has five warmups
and 200 measured iterations. Both quantiles use nearest rank.

The combined load-and-preparation timer measures both phases directly; it is
not a sum of their percentiles. Receipt timing includes acquiring the lock,
refreshing, appending and unlocking. Fixture creation, index generation, the
external note and result validation are outside those timers. Validation checks
the prepared focus, byte ceiling, broken-line count, receipt count, sequence
and shown focus and stop. Process startup, initial project discovery, native
input parsing and output envelopes are outside the phase timers. These phase
measurements do not replace the complete-process benchmark above or establish
native reception.

That measured candidate reused a valid index and folded the log in a streaming
pass when no index could be used. Event metadata could borrow the input line;
escaped strings retained owned decoding. Both views shared the required fields
and JSON value types. It reused the loaded tree for the preliminary
custody query, with separate policy checks under the lock before applying any
authorized changes. A retained reader and checkpoint let an ordinary appended
tail refresh the receipt without folding the history again. A changed file
identity, incompatible checkpoint or tail outside the fast path caused a full
authoritative reload. This fallback preserved supported input rather than
imposing the strict artifact-capture limits on session opening.

The first reference and candidate runs produced these **p99 times in ms**:

| Nodes / index / receipt | Reference load + prepare | Candidate load + prepare | Reference receipt | Candidate receipt |
|---|---:|---:|---:|---:|
| 100 / absent / stable | 2.3701 | 1.7607 | 0.2788 | 0.3240 |
| 100 / absent / concurrent | 3.0407 | 2.4327 | 9.7660 | 0.3734 |
| 100 / available / stable | 2.1652 | 2.0476 | 0.2788 | 0.3582 |
| 100 / available / concurrent | 2.7377 | 2.0951 | 8.7506 | 0.3505 |
| 10,000 / absent / stable | 107.7031 | 46.4430 | 0.3508 | 0.4044 |
| 10,000 / absent / concurrent | 108.8979 | 46.2401 | 63.8698 | 0.4709 |
| 10,000 / available / stable | 16.2331 | 11.4059 | 0.3068 | 0.3814 |
| 10,000 / available / concurrent | 19.0359 | 9.8989 | 15.4090 | 0.4846 |

A second candidate run used fresh independent fixtures of 10,000 nodes,
again with five warmups and 200 samples per cell. These are separate p99s;
no percentiles were averaged or pooled:

| Index / receipt | Load + prepare, first / second (ms) | Receipt, first / second (ms) |
|---|---:|---:|
| Absent / stable | 46.4430 / 48.0521 | 0.4044 / 0.3980 |
| Absent / concurrent | 46.2401 / 46.2715 | 0.4709 / 0.4891 |
| Available / stable | 11.4059 / 9.8940 | 0.3814 / 0.3826 |
| Available / concurrent | 9.8989 / 10.8004 | 0.4846 / 0.4503 |

All shapes emitted 4,235 context bytes in stable cases and 4,305 after the
concurrent-note warmup. Custody was manual, with no native agent files. Runs
were sequential rather than interleaved; managed Rust jobs were serialized
while other user applications remained running. Quantile differences alone
do not identify their cause. The reports retain every sample and maximum.

These runs do not time automatic imports, materialization, a second preparation
attempt or the full authoritative fallback for an incompatible cursor or large
tail. They do not update the earlier complete-process measurements to this
candidate or accredit other platforms and native model reception.

To reproduce, run `python tools/measure-opening-phases.py prepare --build-dir
<new-external-directory>` from the checkout, then run `cargo clean -p vivac`
and `cargo build --locked --release --bin vivac` in that copy, using the
required Rust coordinator where applicable. After the executable is at least
300 seconds old, run the helper's `run` command with `--build-dir`, a new
`--fixture-root`, `--output`, `--nodes`, `--fronts`, `--samples`, `--warmups`,
`--filesystem` and `--rust-build`. The last two describe the verified environment;
they do not discover it. File age does not prove build age or an idle host.

### Opening with full prefix validation

Measured on **28 October 2026**, on the same Windows host, with Rust/Cargo
1.94.1, the locked release profile, the system allocator and fixtures on
**H: NTFS**. This candidate folds the authoritative log without reusing its
derived index. It hashes the bytes during that read and validates the complete
prior prefix before every incremental opening receipt, including a receipt
with no intervening append. File timestamps or change-journal records do not
replace that comparison. An incompatible cursor falls back to a complete
authoritative fold. The opening reader uses 256 KiB blocks and obtains the last
line checkpoint from the offset already observed during parsing.

The source inventory SHA-256 is
`b0595127b4630c7e1a3661a66c6c85512e4abd8c7d90d89eb915daf76d5cd9bc`.
The measured executable SHA-256 is
`b4ae3cfe05ff8d8794d8d7fb080ad8499cdf8ffa1ecce16355b3234052f9aa7c`.
The copied build used the same helper and injected driver described above,
without `cfg(test)` or a counting allocator. Its dependency cache excluded all
`vivac` outputs and fingerprints; the build compiled `vivac` afresh. The helper
verified the frozen copy and executable before and after each run.

Each cell has five warmups and 200 measured iterations. Both quantiles use
nearest rank. All three runs used the same executable; the two 10,000-node runs
used fresh independent fixtures. They are separate series, not pooled samples
or averaged percentiles. An available index is created before warmup and left
unchanged by opening; this candidate does not use it as a source of tree data.

| 100 nodes / index / receipt | Load + prepare p50 / p99 (ms) | Receipt p50 / p99 (ms) |
|---|---:|---:|
| Absent / stable | 1.5957 / 2.2634 | 0.3012 / 0.4941 |
| Absent / concurrent | 1.6702 / 2.3786 | 0.3266 / 0.4723 |
| Available / stable | 1.5139 / 1.9392 | 0.2896 / 0.4310 |
| Available / concurrent | 1.6384 / 2.4345 | 0.3177 / 0.4595 |

| 10,000 nodes / series / index / receipt | Load + prepare p50 / p99 (ms) | Receipt p50 / p99 (ms) |
|---|---:|---:|
| A / absent / stable | 32.6760 / 48.6537 | 3.7590 / 4.8180 |
| A / absent / concurrent | 32.0927 / 35.9122 | 3.7724 / 4.1964 |
| A / available / stable | 31.6502 / 35.3146 | 3.7013 / 4.1759 |
| A / available / concurrent | 32.0020 / **52.4156** | 3.8937 / **5.9444** |
| B / absent / stable | 32.0010 / 36.5075 | 3.6498 / 4.2457 |
| B / absent / concurrent | 32.2910 / 43.0368 | 3.8071 / 4.2426 |
| B / available / stable | 32.8823 / 40.4664 | 3.7624 / 4.6603 |
| B / available / concurrent | 32.0169 / 36.6962 | 3.8603 / 4.4397 |

The 100-node run and series B meet both nominal targets in every cell. Series A
exceeds 50 ms for loading plus preparation and 5 ms for the receipt in one cell.
These results do not establish that every cell meets the nominal targets.
The exceeding cell is retained; percentile differences alone do not establish
their cause.

Under the [latency tolerance and repeatability policy](PILLARS.md#latency-tolerance-and-repeatability),
the 52.4156 ms reading falls within the 55 ms tolerance band. The 5.9444 ms
receipt exceeds the 5.5 ms band in series A, while the corresponding cell in
independent series B takes 4.4397 ms p99. That unreproduced exceedance is a
warning to retain and investigate, not an integration blocker. This
classification does not change any recorded time or waive a security check.

Each fixture has 100 open root tasks; the larger shape closes the remaining
tasks. Stable contexts contain 4,235 bytes and concurrent contexts 4,305 bytes
after warmup. Custody is manual, without native agent files. The helper checks
receipt count, sequence, shown focus and stop, complete-context size and broken
lines outside the timers. It preserves all samples and maxima in its JSON
reports. Managed Rust work is serialized while other user applications remain
running; the experiment does not establish an idle host.

The combined timer covers loading and preparation directly. Receipt timing
includes locking, prefix validation, refresh, append and unlock. Startup,
initial discovery, native input and envelopes are excluded. These runs do not
measure automatic imports, materialization, a second preparation attempt,
authoritative fallback latency, other platforms or native model reception.
Reproduce with the same `prepare` and `run` commands and aging controls above;
identify the actual source and executable instead of relying on their shared
`0.21.0` version string.

## Private sanitation planner: measurement scope

The opt-in release tests in `src/sanitation/measurements.rs` measure the real
private planning core. They do not expose a sanitation command. Each case
uses two series of three independently prepared synthetic fixtures. Capture
and `verify_unchanged` are timed separately; fixture generation, source
snapshots, envelope inspection and cleanup are outside those timers. Source
snapshots also warm filesystem reads, and verification follows capture. These
are neither cold-cache nor contended-writer measurements.

Each sample emits JSON with its case, series, phase, input and expanded-plan
counts, elapsed time and peak additional Rust heap measured on the calling
thread. That heap counter is not process RSS, total resident memory or a
measurement of stack and operating-system allocations. A rejected capture has
no plan: its verification is reported as `not_run`, with null measurements.
Envelope boundary cases distinguish predicted serialized size from the size
of a successfully captured plan and check that the two agree on acceptance.

The cases separate 10,000 distinct nodes, a 256 MiB log with selected notes,
the same volume of unselected notes, encoded event lines at and above 1 MiB,
and serialized plans near and above 512 KiB. A separate oversized-log case
checks the size refusal. Three samples per series do not establish p99;
retain the raw observations and report medians and maxima instead.

These measurements do not establish a sanitation-specific latency or memory
budget. Artifact-retention targets concern another operation. Custody member
counts and bytes, dense selections, deep histories, simultaneous limits and
other platforms remain outside this initial matrix. The complete exposure
requirements in [Sanitation design](SANITATION-DESIGN.md) still apply.

Run an individual case with its full test name, replacing the suffix for
another case:

```text
cargo test --locked --release --bin vivac sanitation::measurements::event_line_at_limit -- --ignored --exact --nocapture --test-threads=1
```

The other suffixes are `distinct_nodes_10000`, `selected_notes_log_limit`,
`retained_notes_log_limit`, `log_limit_plus_one`, `event_line_limit_plus_one`,
`envelope_near_limit` and `envelope_over_limit`. Preserve the emitted JSON for
both series, including refusals; fixture-generation time is not operation
latency. A 64 KiB copy buffer does not bound the complete history fold's heap.

### Windows observations, 4 November 2026

All eight opt-in cases completed successfully. The [96 phase observations](measurements/sanitation-planner-windows-2026-11-04.jsonl)
retain both series: 60 accepted phases, 18 refused captures and 18 `not_run`
verification records. Console line wrapping in the first case was rejoined to
recover its JSON objects; their field values were preserved. This completes
the initial matrix, not the exposure requirements or a performance budget.

The instrument was compiled from commit `500850a` with Rust 1.94.1 and the
repository's release profile: optimization level 3, LTO and one codegen unit.
The host ran Windows 11 Enterprise 10.0.26200 on a Ryzen 5 5600G with six cores,
twelve logical processors and approximately 31.9 GiB visible RAM. Fixtures
used the fixed NTFS system volume. Managed Rust work was serialized; no
idle-host or cold-cache control was imposed. Queueing and compilation are
outside the measured operations.

For the accepted 1 MiB event-line case, series A capture took 8.532 ms median
and 9.519 ms maximum; series B took 8.185 / 8.250 ms. Verification took
4.522 / 4.615 ms in A and 4.417 / 4.688 ms in B. Peak additional Rust heap
was 4,270,853 bytes for capture and 4,261,982 bytes for verification in both
series. Each fixture contained three events and three required source files.
This single shape does not characterize complete-history or custody costs.

Adding one byte to that encoded event line caused all six captures to refuse.
Refusal took 3.370 / 3.404 ms median / maximum in A and 3.200 / 4.504 ms in B,
with 2,162,736 bytes peak additional Rust heap. Verification was not run and
has no measured cost in this case.

The remaining cases produced the following median / maximum times in
milliseconds. Each row contains three samples; A and B remain separate.

| Case | Series | Capture median / maximum (ms) | Verify median / maximum (ms) |
|---|---|---:|---:|
| 10,000 nodes | A | 376.793 / 630.157 | 193.665 / 194.962 |
| 10,000 nodes | B | 389.976 / 396.205 | 175.558 / 183.823 |
| 256 MiB, selected notes | A | 1,172.484 / 1,707.519 | 584.741 / 666.643 |
| 256 MiB, selected notes | B | 1,140.528 / 1,160.840 | 579.560 / 584.087 |
| 256 MiB, retained notes | A | 16,344.784 / 17,550.164 | 8,507.977 / 8,531.821 |
| 256 MiB, retained notes | B | 16,727.899 / 17,813.754 | 8,352.091 / 8,542.619 |
| Envelope near limit | A | 145.934 / 146.482 | 75.071 / 76.378 |
| Envelope near limit | B | 147.563 / 151.219 | 79.046 / 79.607 |
| Envelope over limit: refused | A | 144.753 / 906.775 | Not run |
| Envelope over limit: refused | B | 145.269 / 146.063 | Not run |
| Log over limit: refused | A | 0.868 / 57.332 | Not run |
| Log over limit: refused | B | 1.093 / 1.319 | Not run |

Both volume cases contain exactly 268,435,456 log bytes, 258 events and one
node. Selecting all 256 notes yielded 4,403,636 bytes peak additional Rust heap
for capture and 4,338,244 for verification. Selecting only the title, leaving
those notes in the fold, yielded 544,258,430 and 275,837,926 bytes respectively.
The latter capture exceeds 519 MiB of additional Rust heap; its median takes
over 16 seconds in both series. These results warrant investigation before
exposure, rather than a claim that the 64 KiB buffer bounds planning memory.
They do not identify the cause of every allocation or establish a budget miss.

The 10,000-node fixture has 10,001 events and a 2,497,998-byte log. Its capture
and verification peaks were 17,214,854 and 10,592,782 additional Rust heap bytes.
The near-limit envelope contains 5,819 locations and measures 524,266 bytes,
22 bytes below 512 KiB; predicted and actual sizes agree in all six captures.
One more selected occurrence projects to 524,356 bytes and is refused; there
is no captured envelope to measure for that case. A log file of 268,435,457
bytes is also refused. The envelope-over and log-over peaks were 3,512,142 and
392 bytes respectively. All maxima, including the 906.775 ms and 57.3315 ms
refusal outliers, remain in the raw observations; they were not discarded.

Every accepted fixture in this matrix has only the log, configuration and lock
as its three required files. No custody workload is represented. Completing
these eight cases does not complete the pending custody and dense-selection
measurements, validate other platforms or authorize a user-facing command.

### Sanitation fold lifetime comparison

The fold-lifetime experiment uses the same release instrument and synthetic
fixtures as the initial matrix. The change releases the first folded tree
after its final custody use, before source verification builds another tree.
Both full history folds, the source lock, privacy inspection, file commitments and
source rechecks remain in place.

For the retained-note fixture of exactly 256 MiB, 258 events, one node and
three files, the experiment targets at most 384 MiB of additional Rust heap
during capture. This leaves 128 MiB above the input size for parsing and model
overhead while rejecting the observed overlapping-fold peak. This is a
regression goal for this fixture, not a global input limit, runtime refusal
or planning budget for other histories. The measurement excludes process
RSS, stack and operating-system allocations.

The experimental checker first requires all twelve observations for that
fixture, with accepted capture and verification in both three-sample series.
Only then does it compare the six capture peaks with the fixed target.
Refusing the input or returning incomplete observations cannot satisfy the
memory check. Timing samples remain observations; this comparison does not
add a latency deadline or establish p99.

The [fresh before controls](measurements/sanitation-fold-before-500850a-partial.jsonl)
contain 24 observations for the selected-note and retained-note volume cases.
They use the preserved release binary from `500850a`; its Rust inputs are
unchanged in `66ae940`. The [after matrix](measurements/sanitation-fold-after-8a5b5db.jsonl)
uses `8a5b5db` and contains all eight cases. Both binaries use Rust 1.94.1,
the same release profile, instrument and host described above. The before
controls are a two-case comparison, not another complete eight-case matrix.
All 96 after observations preserve the expected outcomes: 60 accepted phases,
18 refused captures and 18 verification phases not run. Near-limit envelope
predictions equal actual sizes; over-limit envelopes, event lines and logs
remain refused. These boundary fixtures still contain no custody workload.

| 256 MiB case | Phase | Before peak additional Rust heap (bytes) | After peak additional Rust heap (bytes) |
|---|---|---:|---:|
| Selected notes | Capture | 4,403,636 | 4,368,163 |
| Selected notes | Verify | 4,338,244 | 4,338,244 |
| Retained notes | Capture | 544,258,430 | 275,841,773 |
| Retained notes | Verify | 275,837,926 | 275,837,926 |

Each peak is the maximum of six observations across the two series. Retained-note
capture uses 49.3% less peak additional Rust heap, from 519.05 to 263.06 MiB.
The fixed experimental checker first failed on the fresh before controls and
then passed on the after observations, with all twelve required phases accepted.
This removes the overlapping-fold peak; a single fold still retains substantial
memory. Capture still takes roughly 16–17 seconds median for this fixture;
these samples do not establish a latency improvement or a process-RSS reduction.

### Custody and selection density

These opt-in cases separate selected-field density from retained-file costs.
They use the same two-series, three-sample method, phase timers and additional
Rust heap counter as the initial matrix. Fixture generation and source snapshots
remain outside the timers; snapshots warm filesystem reads. Every capture must
succeed, verification must accept the unchanged source, the source snapshot must
remain identical and the destination must remain absent.

| Case suffix | Controlled input | Expected expansion |
|---|---|---|
| `dense_notes_selectors_1` | 1 selected node among 1,024 nodes, four notes each | 1 selector, 4 locations |
| `dense_notes_selectors_128` | Same history, 128 selected nodes | 128 selectors, 512 locations |
| `dense_notes_selectors_512` | Same history, 512 selected nodes | 512 selectors, 2,048 locations |
| `dense_notes_selectors_1024` | Same history, all nodes selected | 1,024 selectors, 4,096 locations |
| `custody_objects_1` | One retained object, 1 MiB payload | 5 required files |
| `custody_objects_64` | 64 retained objects, 1 MiB total payload | 68 required files |
| `custody_objects_512` | 512 retained objects, 1 MiB total payload | 516 required files |
| `custody_object_hundred_mib` | One retained object, 100 MiB payload | 5 required files |

The dense cases keep the same 1,024 nodes and 4,096 small notes while varying
the selected set. Each selector covers every note on its node. Custody cases
retain actual simple artifact revisions under strict inspection in the main
lane. Required files include the objects, their site marker and the three
source files. `custody_object_count` counts retained objects;
`custody_payload_bytes` sums their bytes and excludes the site marker and source
files. `custody_site_bytes` records the marker separately: 26 bytes in these
custody fixtures and zero in cases without custody. Payloads are generated in
bounded chunks from safe synthetic text.

These cases do not establish a general selector limit or a sanitation budget.
They do not cover native agent custody, bundles, multiple lanes, deep ancestry,
simultaneous limits, cold caches or other platforms. The 100 MiB case uses the
artifact per-file limit; it does not characterize the separate 256 MiB backup
member limit. Raw observations, source attribution and measured results are
required before drawing conclusions from this matrix.

The [Windows phase observations](measurements/sanitation-custody-density-b5212d9.jsonl)
are measured from source commit `b5212d9`, with Rust 1.94.1 and the repository's
unchanged release profile on the host described above. Each reported case must
retain all twelve accepted observations; source and destination assertions are
also required for a successful measurement test.

All eight cases completed with exit 0: 96 accepted phase observations, with
the expected counts, unchanged sources and absent destinations. The dense
fixtures all contain exactly 1,257,515 log bytes and 5,121 events. Times below
are median / maximum milliseconds within each three-sample series; heap peaks
are additional Rust heap bytes measured separately for capture / verification.

| Case | Series | Capture median / maximum (ms) | Verify median / maximum (ms) | Capture / verify heap peaks (bytes) |
|---|---|---:|---:|---:|
| 1 selector, 4 locations | A | 146.665 / 280.028 | 88.223 / 113.632 | 1,924,162 / 1,920,303 |
| 1 selector, 4 locations | B | 137.900 / 141.060 | 76.085 / 88.544 | 1,924,066 / 1,920,399 |
| 128 selectors, 512 locations | A | 142.688 / 148.032 | 72.567 / 77.737 | 2,063,750 / 2,007,463 |
| 128 selectors, 512 locations | B | 136.334 / 147.420 | 70.764 / 80.393 | 2,063,750 / 2,007,463 |
| 512 selectors, 2,048 locations | A | 144.154 / 148.617 | 71.200 / 76.547 | 2,536,022 / 2,321,527 |
| 512 selectors, 2,048 locations | B | 140.535 / 141.721 | 68.395 / 69.626 | 2,536,022 / 2,321,527 |
| 1,024 selectors, 4,096 locations | A | 171.077 / 174.853 | 85.525 / 147.650 | 3,166,646 / 2,741,207 |
| 1,024 selectors, 4,096 locations | B | 187.864 / 191.030 | 90.336 / 114.338 | 3,166,646 / 2,741,207 |
| 1 object, 1 MiB total payload | A | 71.535 / 72.014 | 36.460 / 37.931 | 147,162 / 142,149 |
| 1 object, 1 MiB total payload | B | 70.936 / 71.227 | 36.073 / 36.124 | 147,162 / 142,149 |
| 64 objects, 1 MiB total payload | A | 165.584 / 170.424 | 89.413 / 89.558 | 258,063 / 227,262 |
| 64 objects, 1 MiB total payload | B | 216.254 / 229.435 | 118.308 / 119.906 | 258,063 / 227,262 |
| 512 objects, 1 MiB total payload | A | 1,055.331 / 1,134.443 | 534.034 / 673.833 | 1,072,111 / 855,550 |
| 512 objects, 1 MiB total payload | B | 947.259 / 1,030.581 | 545.587 / 679.283 | 1,072,111 / 855,550 |
| 1 object, 100 MiB payload | A | 7,092.298 / 7,375.111 | 3,808.197 / 4,712.812 | 147,162 / 142,149 |
| 1 object, 100 MiB payload | B | 9,474.469 / 9,514.750 | 3,966.794 / 4,006.374 | 147,162 / 142,149 |

The largest dense case produces a 424,518-byte envelope, within 512 KiB.
These samples do not establish the worst-case cost of checking each location
against the selection with a linear search. At fixed custody payload, increasing object count also
adds receipts, file identities and open handles; the times include those costs.
The 512-object fixture has a 311,652-byte log and 516 required files, compared
with 1,069 log bytes and five required files for one object.

The 100 MiB object and the 1 MiB object have the same observed heap peaks in
both phases. The larger file still costs seconds to capture and verify; these
observations do not identify the share spent hashing, inspecting or opening
files. They neither bound process RSS nor justify a general constant-memory
claim. Both series and all outliers remain in the raw data.

## Experimental OpenSSH bcrypt work on Ubuntu 24.04 — 2026-11-12

Two prepared-fixture series reproduced package-creation latency above the declared
2,000 ms target and its 2,200 ms tolerance band. All fourteen creation samples
exceeded the band. The other measured operations stayed within their nominal
targets. After reviewing the results, the maintainer accepted this observed
creation latency for pilot progression under practical tolerance. That explicit
acceptance preserves the original target, band, samples and `above-band`
classification; it does not lower the KDF work or compound tolerance bands.

The [fixture contract](../tools/native-key-store-probe/review/KDF.md) defines the
windows, input mutations and reproduction commands. Each series used a fresh
container and private persistent package store, one CPU, 512 MiB RAM, no extra
swap, 128 PIDs, 128 MiB tmpfs and an independent 240-second deadline. Provider
admission and baseline preparation preceded timing and could warm caches. All
initial timed samples are included. These seven-sample series describe observed
maxima and medians; they do not estimate p99 or cold-start latency.

The provider was Ubuntu 24.04 amd64, `openssh-client 1:9.6p1-3ubuntu13.20`.
Both series used the same image, provider objects and recorded helper/source
hashes. Helpers came from `/build/writer189/debug/ssh-writer-graph-audit` and
`/build/writer189/debug/openssh-proof-verifier-probe`. The figures characterize
these debug helpers. The later integrated release measurements use separate
intervals. No Rust source or compiled helper was changed for this run.

| Operation | Nominal / band (ms) | A median / max (ms) | B median / max (ms) |
|---|---:|---:|---:|
| `package-create` | 2000 / 2200 | 2290.297 / 2432.523 | 2284.221 / 2300.433 |
| `activation-proof` | 2000 / 2200 | 526.569 / 569.163 | 529.544 / 537.132 |
| `unlocked-sign-proof` | 50 / 55 | 16.275 / 16.873 | 18.372 / 19.196 |
| `reject-rounds-0` | 50 / 55 | 2.942 / 3.421 | 3.475 / 5.368 |
| `reject-rounds-1` | 50 / 55 | 2.688 / 2.923 | 3.092 / 3.346 |
| `reject-rounds-16` | 50 / 55 | 2.820 / 3.074 | 3.149 / 3.733 |
| `reject-rounds-63` | 50 / 55 | 2.879 / 3.188 | 2.924 / 3.344 |
| `reject-rounds-65` | 50 / 55 | 3.169 / 3.638 | 3.072 / 3.513 |
| `reject-rounds-4294967295` | 50 / 55 | 3.134 / 3.919 | 3.114 / 3.663 |
| `reject-large-field` | 50 / 55 | 3.146 / 3.321 | 3.127 / 3.232 |

Each of the 98 malformed-input checks returned exit code 1 with zero native-load
dispatches. Inputs had recomputed manifest size/digest and an accepted baseline;
they covered rounds 0, 1, 16, 63, 65 and 4,294,967,295 plus the large-field prefix.
All measured activation and unlocked proofs were verified; each sentinel still
signed afterward. Both runs completed with exit 0 and no container OOM. That
completion records measurement controls, including the repeated latency miss.

Child-process RSS high-water marks were 25,108 KiB for A and 25,204 KiB for B.
These values include child process/mapping overhead and do not measure auxiliary
parser heap or simultaneous aggregate memory. The separate 2 MiB parser heap
ceiling, password/KDF policy and production Rust launcher were open gates at the
time of this work measurement. The following requested-heap measurement addresses
the heap component separately.
The KDF reports keep `performance_admitted=false`, `kdf_policy_adopted=false`,
`parser_auxiliary_heap_measured=false` and `p99_estimated=false`.

Raw milliseconds below retain all 140 completed samples in their original order.
The retained reports additionally reconcile 70 ordered attempts per series.

| Case | Series | Ordered samples (ms) |
|---|---|---|
| `package-create` | A | 2288.765656, 2279.866375, 2432.522969, 2290.297106, 2287.011857, 2371.130490, 2378.019411 |
| `activation-proof` | A | 533.045766, 515.356842, 526.569334, 527.460505, 524.350419, 569.163369, 525.826575 |
| `unlocked-sign-proof` | A | 16.872773, 16.329008, 16.217510, 16.275261, 15.926839, 16.068620, 16.355859 |
| `reject-rounds-0` | A | 3.004210, 3.030342, 2.941533, 3.421098, 2.884361, 2.798230, 2.676328 |
| `reject-rounds-1` | A | 2.923323, 2.566300, 2.685736, 2.770348, 2.794847, 2.688213, 2.662800 |
| `reject-rounds-16` | A | 3.073952, 2.820495, 2.730604, 2.743818, 3.041968, 2.760167, 2.996621 |
| `reject-rounds-63` | A | 3.147387, 3.069252, 3.188254, 2.742935, 2.878735, 2.756359, 2.770386 |
| `reject-rounds-65` | A | 3.102845, 3.637634, 3.144596, 2.972630, 3.169008, 3.548669, 3.571213 |
| `reject-rounds-4294967295` | A | 3.134146, 2.984739, 3.163877, 3.918743, 3.530306, 3.094079, 2.981761 |
| `reject-large-field` | A | 3.146207, 2.954055, 2.778092, 2.729528, 3.276969, 3.175646, 3.321381 |
| `package-create` | B | 2272.883455, 2289.742978, 2284.221488, 2253.867432, 2300.432548, 2266.530556, 2294.815762 |
| `activation-proof` | B | 529.543630, 516.753030, 514.737555, 530.809592, 536.255257, 516.333943, 537.132459 |
| `unlocked-sign-proof` | B | 17.660988, 19.196371, 18.920534, 18.371760, 17.907471, 18.499325, 16.679989 |
| `reject-rounds-0` | B | 3.475158, 3.297637, 5.368190, 3.736355, 4.651685, 3.122141, 3.287270 |
| `reject-rounds-1` | B | 3.346460, 3.085600, 3.092427, 3.048979, 2.974249, 3.151213, 3.266041 |
| `reject-rounds-16` | B | 3.732634, 2.930273, 2.828553, 3.446086, 3.233865, 3.149008, 3.016302 |
| `reject-rounds-63` | B | 3.343763, 2.891744, 2.923736, 2.740161, 3.161637, 2.992441, 2.864071 |
| `reject-rounds-65` | B | 3.512686, 3.010052, 3.057384, 3.251107, 3.071583, 3.060151, 3.209418 |
| `reject-rounds-4294967295` | B | 3.403123, 3.662650, 3.114087, 2.997000, 3.284502, 2.774027, 2.650678 |
| `reject-large-field` | B | 3.186015, 3.127030, 2.713975, 3.129557, 3.231927, 2.522518, 2.633655 |

Recorded identities (identical between A and B):

| Object | SHA-256 |
|---|---|
| Container image | `806ce46ffb8f4775988f53d68d65182c8b53b540e56c19e581313303c48d33d4` |
| Helper `verifier` | `26cc96f01ad84315e6548698bdab06e914d649401dfaf71f39ec9220a9943c55` |
| Helper `writer` | `3c8c671cb2be8b7ea07a2b55fbcf4c83c31f764b9884f3671f211f9e4b0e3774` |
| Source `kdf_probe` | `d564e51c038ee35e9187bc701dd9d5737e2b09e91ed5228712e6c2fb1117a55a` |
| Source `package_probe` | `3f4a8f63c66fc39f783a3eb0206837d0aaf37c2b1875377eed4bfabae06e05cf` |
| Source `provider_probe` | `3c405eea3d4eba34743feea18b61b68744da52d8993c9afdfa931f9f5d1e9ae8` |

## Requested Rust heap of the package checker — 2026-11-12

The complete instrumented run observed a maximum of **1,114,816 additional
requested Rust bytes** (about 1.063 MiB), within the 2,097,152-byte budget for
these cases. This is the requested-byte counter defined by the
[heap fixture](../tools/native-key-store-probe/review/HEAP.md). It includes the
complete package check's reads and buffers and conservatively counts possible
old/new allocation coexistence during realloc. Stack, native allocator overhead,
reserved arenas and process mappings remain outside this measure.

The standalone helper was compiled offline with Rust 1.89.0 in a separate build
volume, using the optional `heap-probe` feature and the existing locked graph.
Default tests (7) and feature tests (14, including the same default cases), fmt,
and Clippy default/feature debug and release passed in the bounded Linux builder.
The builder image was `rust:1.95-bookworm` with the installed 1.89 toolchain
selected explicitly; the measurement runtime was the separate Ubuntu 24.04 image.
Cargo-deny was unavailable in the offline cache and was not run. These checks
concern the experimental helper, rather than the complete product CI matrix.

The runtime used one CPU, 512 MiB RAM, no extra swap, 128 PIDs, 128 MiB tmpfs,
a read-only root and separate read-only original/instrumented helper mounts.
It observed the same Ubuntu 24.04 amd64 OpenSSH package and runtime image as the
work measurement above. No native key loading or KDF was performed by the
instrumented checks. A separate original helper created the synthetic control
package before measurement.

| Case | Checker accepts | Three additional requested peaks (bytes) |
|---|---|---|
| `valid-package` | yes | 3738, 3738, 3738 |
| `rounds-u32max` | no | 2642, 2642, 2642 |
| `key-count-u32max` | no | 2645, 2645, 2645 |
| `declared-body-1048575` | no | 1049893, 1049893, 1049893 |
| `declared-body-1048576` | no | 2650, 2650, 2650 |
| `declared-body-4294967295` | no | 2653, 2653, 2653 |
| `declared-salt-1048575` | no | 1050157, 1050157, 1050157 |
| `near-cap-large-field` | no | 1114816, 1114816, 1114816 |
| `file-65536-malformed` | no | 66293, 66293, 66293 |
| `file-65537-oversize` | no | 2648, 2648, 2648 |

The valid input was 565 bytes. Each observation reported valid accounting and a
conservative realloc policy, with live bytes after the check equal to its retained
baseline. The final valid-package control was accepted and recorded another
3,738-byte peak. Summary maxima reconcile with all 30 case observations.

The 1,048,575-byte field exercises the decoder's permitted length boundary and
allocates close to one MiB before refusing missing bytes. The next value is
refused with a much smaller peak. The near-file-limit case combines input near
64 KiB with the large declared field; this produced the measured maximum.
These cases do not exhaust every possible input or establish physical RSS.

Earlier attempts A and B each retained 27 observations from the first nine
cases, with the same requested peaks shown above. They stopped during publication
of the intentionally oversized 65,537-byte fixture. That preparation incorrectly
used normal semantic admission checks and a key-readback bound that were
unsuitable for a deliberately malformed input. The corrected
publication path checks bytes and object identity while preserving the normal
admission limits; a real POSIX integration check covered that separation.
Those two attempts remain incomplete. Complete run C then observed all ten
cases, the three fresh processes per case and the final retained control, with
exit 0 and no container OOM.

The original writer and verifier hashes remained unchanged from the work
measurement above. The instrumented helper SHA-256 was
`2be4750789b4f43ffbf87bbda1c953c43a308d9fd5573c91e271725ac6c7aeda`. Recorded source identities were:

| Source | SHA-256 |
|---|---|
| `openssh-writer/Cargo.toml` | `7ddc4327a59e2eff536ddfdd18cd6bcce8d6b033444c472aa9a6b5d42c485c69` |
| `openssh-writer/Cargo.lock` | `5f59ccdfa48aadc74cc5c49d7d5ef671e7da71c6860008291b326aa8f469b140` |
| `openssh-writer/src/main.rs` | `b3a556bc2b9121128fdadf889f658f858fd76030e7e344d8f344fa026e36cfbc` |
| `openssh-writer/src/package.rs` | `60213831ef8f35c4da66339b02abe9a8680ebf7268e83d2249a7da8d01016911` |
| `openssh-writer/src/heap_probe.rs` | `df07d4b825457a5a297597f64836fcbe24349321a7b1edfeaa8af82893014705` |
| Driver `heap_probe` | `9e9bbcdadd1a257b88477baa250dbc18f6008c804b1acc8a31aa66a1c2f15ca2` |
| Driver `package_probe` | `3f4a8f63c66fc39f783a3eb0206837d0aaf37c2b1875377eed4bfabae06e05cf` |
| Driver `provider_probe` | `3c405eea3d4eba34743feea18b61b68744da52d8993c9afdfa931f9f5d1e9ae8` |

This result measures the locked checker and adversarial fixtures described here.
It does not adopt a passphrase/KDF policy or accredit the production Rust process
launcher; Linux custody remains disabled.

## Generated-secret delivery and real deadlines — 2026-11-12

The [generated-secret fixture](../tools/native-key-store-probe/review/GENERATED.md)
exercised the standalone writer's new attended creation mode on Ubuntu 24.04
amd64 with administrator-trusted local OpenSSH package metadata for
`1:9.6p1-3ubuntu13.20`. Two complete ten-case runs observed the positive creation
and nine refusals. Both retained a structurally accepted positive package after
the negative cases. Native `ssh-keygen -y` decrypted that package using the
transiently observed secret over its private PTY and returned the expected public
key. This is decryption/public extraction, not agent loading or signing.

Separate runs each repeated the positive control and withheld input at one
interaction seam. The reader's real deadlines were unchanged:

| Withheld input | Configured deadline | Observed interval from prompt visibility to exit | Result |
|---|---|---|---|
| Initial acknowledgement | 60 s | 60,085.828 ms | Refused; no disclosure or publication; terminal restored |
| Generated-secret confirmation | 180 s | 180,087.367 ms | Refused; one intended disclosure, no publication; terminal restored |

These are functional deadline observations, not percentiles or creation-latency
benchmarks. The new complete production flow has not been measured. The older
2.3-second creation observations above belong to their historical password-input
fixture and must not be relabeled as this flow's performance.

All four runs completed with exit 0 at the experiment level, without container
OOM or selected RDP error events. The observer retained metadata, never the
secret, its hash or the terminal transcript. Foreground and echo checks were
observed where a PTY existed; unavailable terminal states remain null. Cleanup
followed observed process exit. This does not establish remote click
responsiveness or protection from a recorded/compromised terminal.

The helper used Rust 1.89.0 offline in a fresh build volume, with the same Debian
builder and Ubuntu runtime distinction described above. Its default tests (11),
heap-feature tests (18, including the default cases), fmt and default/feature
Clippy debug/release passed. Python unit checks passed with 228 tests and one
POSIX-only case skipped on Windows; that case passed separately on Linux using
real metadata. A prior Windows fixture-adapter ctime mismatch was corrected in
the test alone; explicit ctime-change refusal tests cover each readback seam.
The Linux identity guard was not modified. Cargo-deny and the complete product
CI matrix were not run for this increment.

The writer SHA-256 was
`afff4caaf78956fb981ffc40b41014fac6b5300c9740028c7248613e1e6827fa`.
The final driver revision was
`63e55193ebf75508bc5732dc9b5111bb07b4467da4fbe2b3c65530010c9a4e80`;
the first ten-case run retained the earlier driver revision before the deadline
mode was added. Original measurement builds and the public verifier were
unchanged. Selected writer source identities were:

| Source | SHA-256 |
|---|---|
| `src/main.rs` | `a696db9e5b9fc99c09541713f56f4fca211a13b38d5fd69199d74ccd31f7ab91` |
| `src/terminal.rs` | `8cb94c0376f694e7e87b4a3a3226da31311c13cf506a2d6dcd25a96b1d5d90d9` |
| `src/generated_secret.rs` | `74d2fe2ad955453fa41f15648d381dde510d96e9b168c809cee6e881a3c3791c` |

This evidence does not admit Linux custody. Safe launching from the production
multithreaded Rust runtime, complete native activation/signing, remaining failure
seams and complete-flow measurement still gate adoption of the selected policy.

## Generated-secret terminal drift and publication interruptions — 2026-11-12

The separate [failure matrix](../tools/native-key-store-probe/review/GENERATED.md)
completed seven cases on the same Ubuntu 24.04 runtime and unchanged generated
writer identified above. The experiment exited 0, without container OOM or
selected RDP errors. Its outcomes were:

| Case | Writer exit | Published package | Remaining preparations | Terminal restored |
|---|---|---|---|---|
| Valid generated package | 0 | Accepted and decrypted | Not measured | Yes |
| Input-attribute drift before acknowledgement | 1 | Absent | Not measured | Yes |
| Input-attribute drift before confirmation | 1 | Absent | Not measured | Yes |
| Interrupted after key sync | 77 | Absent | 1 | Yes |
| Interrupted after manifest sync | 77 | Absent | 1 | Yes |
| Interrupted before publish | 77 | Absent | 1 | Yes |
| Interrupted after publish | 77 | Accepted and decrypted | 0 | Yes |

The drift fixture toggled `ICRNL` on its own PTY while keeping echo disabled.
The acknowledgement drift case disclosed no secret. Each later case observed
one intended disclosure, with no echoed confirmation or standard-stream leak.
Bounded inspection of each interrupted case's own files found no literal unlock
secret; it retained no inspected bytes or secret hashes. After the successful
exclusive rename, the expected package remained present and passed both the
checker and native public extraction. This is the commit boundary, not a failed
creation that permits overwriting the destination.

These were the existing abrupt exit hooks, not real filesystem errors or the
writer's error-return cleanup path. The report explicitly kept
`fsync_failure_tested=false` and `terminal_replacement_tested=false`. It does not
accredit terminal reassociation, multithreaded Rust launching, native agent
activation/signing or the complete product backend.

The driver SHA-256 for this run was
`b62d556c4140cabad7ee7f98eb9244e0402690a1918e3cc547eb2247c4da2640`.
The writer and verifier hashes remained unchanged. Python unit verification for
this increment passed 236 tests, with the unchanged POSIX-only integration case
skipped on Windows. No Rust compilation or complete product CI run was performed
for these Python fixture changes.

## Generated-secret publication error returns — 2026-11-12

The [publication-error matrix](../tools/native-key-store-probe/review/GENERATED.md)
exercised the standalone writer's ordinary return paths on the same Ubuntu 24.04
runtime. Its positive control created and decrypted a valid package. The four
controlled error returns had these outcomes:

| Error seam | Writer exit | Published package | Remaining preparations | Exact uncertainty diagnostic |
|---|---|---|---|---|
| After key sync | 1 | Absent | 0 | No |
| After manifest sync | 1 | Absent | 0 | No |
| Before rename | 1 | Absent | 0 | No |
| After rename | 2 | Accepted and decrypted | 0 | Yes |

The precommit returns ran the owned preparation's normal cleanup. The postcommit
return preserved the package, passed the checker and native public extraction,
and emitted the constant indeterminate-durability diagnostic with empty stdout.
Bounded inspection of the cases' own files found no literal unlock secret;
neither inspected bytes nor a secret hash entered reports. These seams occur
after the preparation guard is established. An earlier failure to obtain or
validate the newly created directory handle can leave a preparation for
inspection; no universal precommit cleanup claim follows from these cases.

The exclusive rename remains the commit point. Every error in the subsequent
sync/name/integrity checks now maps to the typed `CommittedIndeterminate` result
in the generated mode. Historical modes retain their generic exit-1 behavior.
Empty fault-channel values still mean no selection; two nonempty selections
refuse. This fixture injects returned errors, not a real failed sync syscall, and
keeps `fsync_failure_tested=false`.

The five-case experiment completed with exit 0. The new binary also passed the
existing ten-case generated interaction and seven-case abrupt-interruption/drift
matrix. All three runtime reports observed no container OOM or selected RDP
errors. They establish no creation-latency percentile or production admission.

The helper was rebuilt offline with Rust 1.89.0 in another fresh volume, preserving
earlier measurement binaries. Default tests (15), heap-feature tests (22,
including the default cases), fmt and default/feature Clippy debug/release passed.
Python verification passed 243 tests with one unchanged POSIX-only case skipped
on Windows. Cargo-deny and the complete product CI matrix were not run.

The writer SHA-256 was
`6d4b954401eb1396db3622eb0326d8aa5372c93ab80efb0341cece6689406a03`;
the driver revision was
`d6778bf98423f898a32f16b6df39a79c80e905d63c8758fec64bb561b710f729`.
The verifier, dependency manifest and lockfile were unchanged. Updated writer
source identities were:

| Source | SHA-256 |
|---|---|
| `src/main.rs` | `08a3e122d1249fb8c30e2fcaab5968692b42b0f133c6a9ede482311a2a07a10a` |
| `src/package.rs` | `2d6dfeb2af440062891ee96b8cd8a5f8423e4cd79bb4ccb3ceafaaa3834b4098` |

These results do not accredit the production multithreaded Rust launcher, native
agent activation/signing or complete-flow performance. Linux custody remains
disabled.

## Candidate Rust spawn boundary — 2026-11-12

The [Rust launcher fixture](../tools/native-key-store-probe/review/RUST-LAUNCHER.md)
compiled the candidate `src/device_keys/linux_spawn.rs` source into a disposable
Rust helper. The product declaration is Linux-test-only; this is not integration
of the operational credential backend. Rust 1.89.0 and libc 0.2.189 match the
product's declared MSRV and locked binding. The helper's other external versions
were already present in the reviewed writer graph. The missing Linux-cache copy
of the public libc crate came from the local host cache and matched the product
lock checksum; no dependency download was required.

The final seven-case run completed on Ubuntu 24.04 amd64 using the same retained
OpenSSH package gate and bounded runtime image as above:

| Case | Observed result |
|---|---|
| Stable creator thread | Four native children, four returned caller threads, four allocator/mutex pressure threads; clean environment/mask and exact inherited FD allowlist; non-CLOEXEC canary at FD >=4096 excluded |
| Malformed executable ELF | ENOEXEC propagated to spawn after both barriers; no receipt program ran; owner remained reusable |
| Creator-thread exit control | Child SIGKILL with parent process/control channel still alive; deliberately forgotten test owner did not run Drop |
| Parent death before arming | B reached; changed-parent receipt P from the same child PID after release; no exec; observed SIGABRT |
| Parent death after arming | A reached; child SIGKILL while observer retained the gate |
| Native empty agent | Credentials/empty identities checked after exec; SIGKILL shutdown observed; retained/new connections refused |
| Native agent parent death | Owner SIGKILL followed by actual agent SIGKILL; retained/new connections refused |

The two pressure counters recorded 87,953 and 37,327 iterations, respectively.
These are work-presence observations, not a throughput benchmark or latency
percentile. The container inherited NNP=1; post-exec NNP was observed without
claiming a transition from NNP=0. The first native run preserved its prearm
SIGABRT observation without the P receipt; only the later source/run supports
the explicit changed-parent cause. All observed cases and source revisions were
retained rather than relabeling the first run.

The final helper passed seven unit tests, fmt, Clippy debug/release and build in
the bounded offline builder. An earlier compile overlapped the harness's final
edit and was retained without attributing it to the frozen source; package clean
and verification were repeated afterward. A further clean/test/Clippy/build
followed the P-receipt change. Python unit verification passed 250 tests with one
unchanged POSIX-only case skipped on Windows. The complete product crate/CI
matrix and Cargo-deny were not executed for this increment.

The final native run exited 0 without container OOM or selected RDP error events.
Its helper SHA-256 was
`8cc008b4ade5b003fbdace3f50bac19ee45aad39acf5200f099d73b00d14b062`;
the Python observer revision was
`8232684d6e8be7108dfd73c506b6f5515e292e79ed541ffbd7d7723d2e8ae8a7`.
Selected source identities were:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_spawn.rs` | `098794696000eed3c25ea9dba3c266a7cedbbbf8e775daa357a6483c94d1c033` |
| `rust-launcher/src/main.rs` | `bb6b8b2b915b8a6be52ef96abf4ea0d03f619ab2c7d9246ca357ad8b87c84d74` |
| `rust-launcher/Cargo.lock` | `a8fe3455ab816f902bbed92f84827d098c68e4ecc841f6a138cddf3a19f1ab1f` |

Loader PTY/secret delivery, cancellation across loading/readiness/signing, package
leases, full-flow performance and operational product integration remain open.
Failure to acquire a pidfd after spawn preserves ownership but can leave Drop
waiting indefinitely; this matrix does not exercise or accredit automatic
recovery from that case. Linux custody remains unavailable in normal commands.

## Candidate Rust loader lifecycle — 2026-11-12

The [Rust loader fixture](../tools/native-key-store-probe/review/RUST-LOADER.md)
extended the shared candidate with a private PTY, sealed ciphertext descriptor,
stopped secret handoff, native loader completion and independently verified
proof before test READY. Eight cases passed: normal completion, cancellation at
prompt/stopped handoff/before READY, supervisor SIGKILL at those three points,
and loader SIGKILL at prompt. In every case the external observer retained its
master and package lease through observed native exit, checked slave mode `0600`
and UID, and rejected retained/new connections after closure. An independent loaded sentinel
signed successfully after every case. No runtime or memory percentile was
measured by this matrix.

The same helper passed the preceding seven-case spawn matrix. Rust 1.89 offline
verification passed 17 unit tests, fmt, Clippy debug/release and build after clean.
Python verification passed 260 tests with one POSIX-only Windows skip. The full
product CI matrix and Cargo-deny were not run. The Debian builder and Ubuntu
24.04 native runtime stayed separate; the native runs used the same bounded,
offline, unprivileged image and OpenSSH package as above. Both exited 0 without
OOM or selected RDP error events.

The helper SHA-256 was
`117ae80a36505358ec391db824060d8078f6eb2b7e42729f2b2a55a773a52900`;
the loader observer was
`31daca061592fb21d7ffdccb6a05b7939e09ca9ee4acf2c7c7569f32b5b64e37`.
Selected source identities were:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_spawn.rs` | `b76d33f0e4a03dae3169d5f3c160a8e70f32df342f06c46f4fca3a021a1c8502` |
| `src/device_keys/linux_loader.rs` | `942286bd81e324acd163b30bcafbd3b296c20119f0ec8f4c5505e3111e56d3f8` |
| `rust-launcher/src/main.rs` | `67353369317fd95f629886510df01a42ba46274f8825e3593386c8bb07c52f56` |
| `rust-launcher/src/loader_probe.rs` | `1696baf13e59dc247390d7981a9f1159463e8798b34d8191e3c6297048ad1366` |
| `rust-launcher/Cargo.lock` | `d690e6ef209c234cf18da1b171f9703e33433236c2c3f4f86845290d114f7e96` |

Terminal chmod does not revoke an already opened descriptor; this observation
does not establish atomic private permissions at terminal creation or privileged
concurrent isolation. Real SSH disconnect, complete shared-session integration,
full-flow performance and operational product custody remain unaccredited.

## Candidate Rust loader over SSH — 2026-11-12

The [Rust SSH loader extension](../tools/native-key-store-probe/review/RUST-SSH-LOADER.md)
ran ten cases through an actual isolated SSH connection: normal close and
disconnect at prompt, stopped handoff, before READY and after READY, each with
and without a remote PTY. A bootstrap transferred retained capabilities and
execed Rust in the same PID; the observer checked start identity, executable
inode, SSH ancestry and actual stdin kind. Only the owned SSH client was killed.
The Rust helper observed SIGHUP in all four PTY disconnects and EOF in all four
pipe disconnects. EIO handling passed a unit classification check, without a
native EIO observation in this run.

All ten cases observed native closure before its acknowledgement, then
supervisor exit and refusal of retained/new connections. The external observer
kept its master and package lease through native exit. Loaded cases required an
independent proof before test READY; the pre-READY disconnect withheld READY.
An independent loaded sentinel signed after every case. The updated helper also
passed the preceding eight-case local loader matrix, including supervisor death.
Both native runs exited 0 without OOM or selected RDP error events. Neither
matrix measures a latency percentile, heap bound or silent network partition.

The helper passed 18 Rust unit tests, fmt, Clippy development/release and build
offline using Rust 1.89 after clean. Python verification passed 270 tests with
one POSIX-only case skipped on Windows. No dependency versions changed in this
increment. The full product CI matrix and Cargo-deny were not run. The native
SSH image was Ubuntu 24.04 amd64 with OpenSSH `1:9.6p1-3ubuntu13.20`, retaining
CPU1, RAM512 MiB without extra swap, PID128, NNP, unprivileged execution, bounded
tmpfs, read-only helpers and no external network. The Debian builder remained
separate from the Ubuntu runtime.

The updated helper SHA-256 was
`c39240a51d4ce63ef00b0ea930909c4d072e2b7f8cb5567a3fbdf4e664ca6409`;
the SSH observer was
`0ff4b46076d6ce947e821801212e302e1003cfb958dfe4ad0cc4a70a5d283124`.
The changed `rust-launcher/src/loader_probe.rs` source SHA-256 was
`8d0dc374f88effdbe9fc6c8653eba5055cd8e7a4e307f214fa9922005a67a14c`.
The shared `linux_spawn.rs`, `linux_loader.rs`, auxiliary `main.rs` and lockfile
identities remained those recorded in the preceding loader section.

This credits the selected real transport disconnect seams in the disposable
Rust fixture. It does not make SSH detection atomic against every signal
interleaving, accredit silent partitions, establish complete shared-session
integration or enable operational credential custody. Earlier pidfd-failure and
PTY permission-transition limitations remain applicable.

## Candidate Rust signing gate — 2026-11-12

The [Rust signing gate fixture](../tools/native-key-store-probe/review/RUST-CONCURRENT.md)
compiled `src/device_keys/linux_gate.rs` with the previously observed native
owner and loader. Seven cases completed with the expected causal counters:
published-before-lock control; lock before send, after complete send/before
reply consumption, after reply, and before publication; two lock callers; and
encrypted-package lease mutation after verification. Only the completed control
published. The lease-negative control independently detected its mutation and
reported lease invalidation at the selected seam. Each case observed one joined
worker, native agent SIGKILL and loader exit 0 before close acknowledgement,
retained/new connection refusal, and a sentinel signature after closure.

The runtime exited 0 on Ubuntu 24.04 amd64, OpenSSH `1:9.6p1-3ubuntu13.20`,
without OOM or selected RDP error events. It retained CPU1, RAM512 MiB without
additional swap, PID128, bounded tmpfs, NNP, unprivileged execution, read-only
helpers and no network. This is an ordering/ownership observation, not a latency
percentile, memory measurement or proof of the instant OpenSSH executed signing.

The offline Debian builder used Rust 1.89 and unchanged auxiliary dependency
versions. After clean, 36 unit tests, fmt, Clippy development/release and build
passed. The initial Clippy attempt failed on coordinator argument count; grouping
native identity into a struct removed that failure without suppressing the lint,
and verification was repeated after another clean. Python checks passed 277
tests with one POSIX-only Windows skip. An additional Windows product nextest
attempt was stopped during compilation after exceeding the read-only review
scope. Its exec exited 1, its recorded launcher processes and Windows Job were
subsequently absent, and it supplies no product-CI verdict. The complete product
CI matrix was not completed; Cargo-deny was not run. Earlier SSH/cancellation
runtime matrices were not repeated in this increment.

The helper SHA-256 was
`e84065616f0cf7f6193532550ddc280f0e0d87b4ff5d008b3195db376b593a3b`;
the Python observer was
`a0e44cbcacc43151735910a5f4f02bb238e1cbbab1263f1ccf3d82450699f236`.
Selected source identities were:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_gate.rs` | `0beb5b90ff50cbdf51e4d340302a862ef87013e0c32b074d66be4ea32fbd7520` |
| `rust-launcher/src/concurrent_probe.rs` | `ffbd8e84768f6b78b26a989905e7f2cb6e859bd2c648b2866234690957179671` |
| `rust-launcher/src/main.rs` | `44b0b60504311b5374657cbcfaec7998075d02b714b951b443a43e28df226cac` |
| `rust-launcher/src/loader_probe.rs` | `937637a110b859cb17b47fadc8fe58d10564768949a3ffd4420cdeb70e3457ef` |

The gate retains custody from creation until explicit joined closure; abandoning
it or reaching permanent uncertainty can retain resources until process exit.
The fixture does not accredit recovery or permit accumulating abandoned product
sessions. Deadlines prevent late acknowledgement, but callbacks must cooperate
and joining is not preemptive thread cancellation. Complete product integration,
client authorization/freshness, every interleaving, full-flow performance and
operational Linux custody remain unaccredited.

## Shared candidate Rust session adapter — 2026-11-12

The [shared session adapter](../tools/native-key-store-probe/review/RUST-SESSION.md)
moved package leases, signing work and the native stop/release bridge from the
fixture to `src/device_keys/linux_session.rs`, included only for Linux product
tests and imported by the auxiliary helper. The fixture keeps orchestration;
its duplicate implementations were removed. The adapter fixes the socket to
the owned agent and checks SO_PEERCRED at construction and on each signing
connection. Release is linked to gate Closing, observed join, a unique lease
owner and matching native exit receipts.

The same seven native cases completed on Ubuntu 24.04 amd64 with OpenSSH
`1:9.6p1-3ubuntu13.20`. Comparing the case objects with the earlier gate run
matched all causal fields, excluding only PID/TID and the pressure counter.
Only the completed control published. Cancellation and lease invalidation
withheld publication; every case observed one joined worker, expected native
exits and connection refusal, and an unaffected sentinel signature. The runtime
exited 0 without OOM or selected RDP error events, retaining the earlier CPU1,
RAM512 MiB/no-extra-swap, PID128, tmpfs, NNP, unprivileged, read-only and external
network-isolation controls. These are ownership/order observations, not latency
or memory-budget measurements.

After another clean, the final offline Rust 1.89 auxiliary build passed 44 unit
tests, fmt, Clippy development/release and build. The earlier 43-test intermediate
revision was retained before adding a guard-rejection test. That unit test uses
the real gate and lease with the same authorization function called by release;
it refuses cleanup/ACK with an extra loan and permits closure with a unique
owner. It is not an additional native case. Python checks passed 277 tests with
one POSIX-only Windows skip. Auxiliary dependency versions were unchanged.
The complete product CI matrix, Cargo-deny, earlier SSH/cancellation runtime
matrices and full-flow performance were not executed in this increment.

The helper SHA-256 was
`d26191fb5d7d4829f00afdcef53e7a1fa047886431e25ac718a6de4b17087ffd`;
the unchanged observer was
`a0e44cbcacc43151735910a5f4f02bb238e1cbbab1263f1ccf3d82450699f236`.
Selected source identities were:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_session.rs` | `c77e7733f4257376ca542f2b9e18a3c58b5b46431d8498c62846a7f62ca7afed` |
| `src/device_keys/linux_spawn.rs` | `30021286cacc7a471cb04e1a3aa243130e87f7bd1be68fe0c21755f281cb95c5` |
| `rust-launcher/src/concurrent_probe.rs` | `d639b0148f4f8a69a2b2e157e86eafa945e128143285fc8cb98cceb3cf2b5abd` |
| `rust-launcher/src/main.rs` | `ed154b529dde71295e722ba22267688ba923fde08d5e2be8306051d00406d0ec` |

Dropping an unreleased adapter attempts only owned-token signals and retains
the bundle without an exit acknowledgement; abandonment can retain resources
until process termination. This does not establish recovery, complete product
admission, client authorization/freshness or every race. Native provider/profile
and exact loaded-identity admission remain caller obligations. Operational Linux
custody remains unavailable.

## Shared signing session over SSH — 2026-11-12

The [SSH/signing session composition](../tools/native-key-store-probe/review/RUST-SSH-CONCURRENT.md)
combined actual transport loss with the shared Rust gate and native adapter.
The lifetime detector moved to `src/device_keys/linux_ssh_lifetime.rs`, also
used by the existing loader fixture. It owns a CLOEXEC stdin duplicate, records
EOF/EIO/SIGHUP without clearing a cause, and restores the prior signal
disposition on Drop. Its irreversible singleton permits one SSH lifetime per
supervisor process; it is not a multi-client daemon implementation.

Twelve cases passed: normal close, disconnect before native send, with reply
pending, after reply, before publication and after publication, each with PTY
and pipe. All eight uncommitted requests were cancelled without publication.
The observer held the signing barrier until Closing with active work and no
ACK was observed, then released it on a private control channel. The four
published controls retained publication. The ten disconnects produced one
observed SIGHUP and nine EOF causes; EIO remained unit-only in this run. Each
case observed one joined worker, one close acknowledgement, expected native
exits and connection refusal, supervisor exit, and an unaffected sentinel.

The same helper passed the ten existing loader-over-SSH cases and seven local
concurrency cases after the shared control/parser changes. All three runtime
runs exited 0 on Ubuntu 24.04 amd64, OpenSSH `1:9.6p1-3ubuntu13.20`, without OOM
or selected RDP error events. CPU1, RAM512 MiB without additional swap, PID128,
bounded tmpfs, NNP, unprivileged execution, read-only helpers and external
network isolation were preserved. SSH used container loopback only. These
observations establish selected lifecycle/order behavior, not a latency
percentile, heap bound or instantaneous network-loss detection.

The offline Rust 1.89 auxiliary build passed clean, fmt, 52 unit tests, Clippy
development/release and build. Python checks passed 286 tests with one
POSIX-only Windows skip. No dependency version changed. The complete product
CI matrix, Cargo-deny and full-flow performance measurements were not run.

The helper SHA-256 was
`3bd2c80765b099b97205601026542b09b3e93da0f14e7df1ea871e0959e8addf`;
the new Python observer was
`aa1045e5deb82b69672a33fc89d63f98bfef01c8e5ff9af1cb50914b94932b52`.
Selected source identities were:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_ssh_lifetime.rs` | `9631e20aff9574bdf2cb25d9f801642e5da60cbdf0d9e4fa2d3f776bf42f98e5` |
| `rust-launcher/src/loader_probe.rs` | `b85f5c82dcd1bf70dc82a4e0801f77151f6f095357032b3fe3fae444acc6c29c` |
| `rust-launcher/src/concurrent_probe.rs` | `35938bfd8ddad5a45d22cfc85118d1de2d81462d375ba6c3d9b474708c4c011f` |
| `rust-launcher/src/main.rs` | `7a8e494b4373f844c75ab904a15026ab062bce7e5c8e8d3c521ff47f0ff5b471` |

This remains candidate evidence for the self-hosted team service. It does not
enable Linux custody, establish authenticated/durable shared admission, prove
every interleaving or silent partition, or accredit recovery from uncertain
pidfd acquisition/abandoned custody. Previous terminal and cooperative-deadline
limits remain applicable before adoption into an operational backend.

### Bounded native abandonment, 2026-11-12

The [candidate abandonment contract](../tools/native-key-store-probe/review/RUST-ABANDON.md)
replaces the spawn owner's indefinite destructor wait with one stop/poll pass,
retention of uncertain ownership and irreversible process-wide custody refusal.
Four reserved owners, each with at most four children, bound retained entries;
this is not a process-memory budget or automatic recovery. NativeSession marks
uncertainty before retaining its complete bundle and permit. Explicit lock and
its publication boundary remain separate.

A fresh auxiliary target, after package cleaning, passed 56 Rust tests, format
checking and Clippy with warnings denied in debug and release, offline on Rust
1.89. The final Python run passed 297 tests with one POSIX-only test skipped on
Windows. The public product package was cleaned before verification. No new
dependency version was introduced. Full product CI, Cargo-deny and full-flow
performance were not run.

Six isolated Ubuntu 24.04 amd64 runs observed 48 native cases with exit 0:

| Experiment | Cases | Result |
|---|---:|---|
| Abandonment | 3 | Four-owner admission limit and clean reuse; injected post-spawn EMFILE with responsive live child and retained marker; valid-pidfd abandonment |
| Local signing session | 8 | Seven previous lock/lease controls plus loaded-session abandonment with a paused signer, five retained descriptor identities, actual worker join and callback uncertainty without ACK |
| SSH signing session | 12 | Previous PTY/pipe normal-close and disconnect boundaries |
| Spawn primitive | 7 | Previous stable-creator, exec-failure and parent-death controls |
| Private loader | 8 | Previous startup, cancellation, lease and owner-death controls |
| SSH loader | 10 | Previous PTY/pipe transport and startup boundaries |

The injected error tests the actual post-spawn failure branch, not descriptor
table exhaustion. In that case shutdown returned ETIMEDOUT, Drop returned while
the child still answered its own channel and a second helper command confirmed
responsiveness. The observer checked the unique helper-side marker capability
before requesting normal helper exit, then collected the child's SIGKILL status.
The loaded-session case likewise required a response after Drop, retained all
five checked descriptors through worker join and refused new owners three times.
Its selected before-send request failed without publication; subsequent lock
returned uncertainty with no accredited release. This does not prove that Drop
generally revokes unpublished work.

Single observed Drop durations were 4,509 ns for missing pidfd, 4,068 ns for
explicit owner abandonment and 22,664 ns for loaded-session abandonment. These
are individual observations under fixture limits, not latency distributions,
budgets or hard real-time syscall guarantees.

All successful runners checked CPU1, 512 MiB RAM, no extra swap, 128 PIDs,
bounded tmpfs, no network, unprivileged credentials and NNP. They reported no
OOM kill or selected RDP error events; independent loaded sentinels survived
the loader/signing experiments. All fourteen owned build/runtime containers
and seven temporary stores were absent after verification. Report volumes and
the auxiliary build volume remain retained as evidence. RDP event selection
does not diagnose or exclude every possible remote-desktop failure.

Local evidence directories under the workspace `.scratch/`:

- `rust-abandon-build-20261112-a`: seven build phases, final Python output,
  nine-source manifest and the checked evidence index.
- `rust-abandon-native-20261112-b`: three successful abandonment cases.
- `rust-abandon-session-native-20261112-a`: eight local session cases.
- `rust-abandon-ssh-concurrent-20261112-a`: twelve SSH signing-session cases.
- `rust-abandon-launcher-regression-20261112-a`: seven spawn controls.
- `rust-abandon-loader-regression-20261112-a`: eight loader controls.
- `rust-abandon-ssh-loader-regression-20261112-a`: ten SSH-loader controls.

The separate `rust-abandon-native-20261112-a` report preserves an initial
fixture refusal before any cases: the new observer was missing from the exact
script allowlist. Adding its name left executable/deadline/credential/cgroup
validation intact. That attempt exited 1, with no OOM or selected RDP errors;
it is not counted as a successful native observation.

All six successful runs used helper SHA-256
`eb740afa072b93d80de11b0c51e656e4b299bd6b979ae00dfe79b82985d45a0b`.
The nine Rust source hashes remained unchanged after build/runtime verification.
Selected changed source/observer identities relative to the preceding SSH composition:

| Source | SHA-256 |
|---|---|
| `src/device_keys/linux_spawn.rs` | `2bb4475052ef16bd38a3f3f6adfa3c8872990192ad8599aaf6f3af9409d47fad` |
| `src/device_keys/linux_session.rs` | `fe0ea3fa44bc43679e94319212b72983be5518de25c77a6cc62b4b65606c3a8a` |
| `rust-launcher/src/abandon_probe.rs` | `b7e85083416683a1f4e554b78347059e48362be09d2735d8a7f4dd85ab555bee` |
| `rust-launcher/src/concurrent_probe.rs` | `a577cc93e074f89b50313be5b9fcf34858fe08afa995354ab2ce6fc776cd1f4c` |
| `rust_abandon_probe.py` | `c296b8481dc532ec630a8dc2bf1bb7f4a39c3d3e0bf75015bbbe258415245f18` |
| `rust_concurrent_probe.py` | `81e2cfb887ab6722fe9d47e6ed58bacabda834ea494b68d5ac5fa14ac75e121f` |

This candidate remains unavailable to normal Linux device-key commands. Service
isolation, recovery and product integration require separate design and proof;
these results do not make the self-hosted central server operational.

### Candidate custody process boundary, 2026-11-12

The [isolation contract](LINUX-CUSTODY-ISOLATION.md) places each explicit Linux
custody session in its own foreground process, separate from the resident
client and central synchronization service. The new
[Rust process-boundary fixture](../tools/native-key-store-probe/review/RUST-ISOLATION.md)
tests containment of process-wide custody refusal. It does not implement MCP,
authenticated IPC, same-store replacement or operational recovery.

On Ubuntu 24.04 amd64, a persistent Rust host ran four allocator/mutex pressure
threads and launched two disposable executors through the shared spawn source.
Worker A injected a missing pidfd after its public self-exec child was stored,
returned from Drop, retained its marker and refused three new owners. While A
and that child remained live and independently observable, B reserved four
owners, refused a fifth, released them, readmitted and exited 0. The host
answered two health requests and actually admitted an empty owner. It observed
both worker pidfds and exact exit token/PID pairs before resource release. The
outer observer checked A/C lineage, credentials, NNP, capabilities, executable
identity and stable start times through retained live pidfds. It collected the
child's SIGKILL status before requesting final host cleanup; host exit was 0.
No loaded key, passphrase or OpenSSH agent participates in this new case.

The final clean auxiliary build passed 59 Rust tests, formatting and Clippy with
warnings denied in debug and release, offline on Rust 1.89. Final Python source
checks passed 306 tests with one POSIX-only test skipped on Windows. The Python
checks were retained with build revision A; only the Rust fixture's exit-identity
assertions changed for revision B. Its ten-source manifest matched current Rust
sources after verification. Shared spawn, gate, loader, session and SSH-lifetime
sources were unchanged from the preceding abandonment increment. No dependency
or MSRV change was introduced.

Three final native runs shared one helper and observed 12 cases with exit 0:

| Experiment | Cases | Scope |
|---|---:|---|
| Process isolation | 1 | Host and B keep working while A remains uncertain and its child remains responsive |
| Abandonment regression | 3 | Admission limit, missing-pidfd containment and valid-pidfd abandonment |
| Local signing-session regression | 8 | Existing loaded-session lock/lease/abandonment controls, including the independent loaded sentinel |

These runs preserved CPU1, 512 MiB RAM, no extra swap, 128 PIDs, bounded tmpfs,
no network, unprivileged credentials and NNP. No runner reported an OOM kill or
selected RDP error event. A final selected-log query from 21:08:53 through
21:48:47 UTC returned no events. That observation does not diagnose or exclude
all RDP failure modes. All ten owned containers from the final seven build
phases and three runtime runs, and their three temporary stores, were absent
after verification. Report volumes and the final build volume remain retained.

Final local evidence under workspace `.scratch/`:

- `rust-isolation-build-20261112-b`: seven build phases, ten-source manifest,
  evidence index and final RDP query.
- `rust-isolation-native-20261112-b`: process-boundary observation.
- `rust-isolation-abandon-regression-20261112-b`: three abandonment controls.
- `rust-isolation-session-regression-20261112-b`: eight loaded-session controls.
- `rust-isolation-build-20261112-a/python-tests-final.log`: unchanged final
  Python source checks.

Revision A reports/build remain separate. Review strengthened independent
process-lineage checks before its runtime, then added explicit token/PID and
pidfd exit assertions for both workers in revision B. The final build and all
three selected native matrices were rerun for that new artifact; only revision
B contributes to the final 12-case count above.

| Identity | SHA-256 |
|---|---|
| Final helper | `a2f0d4b4b76e0e95678bf0c8c5e32589471f7a0dd384efcc549f675f842bddb9` |
| `rust-launcher/src/main.rs` | `dd5e11b26b96b5842ff47c717da913b58e1d2eb889cf44fd33b4f2526650cb97` |
| `rust-launcher/src/isolation_probe.rs` | `ba31e6540f242a63df9ee1607d9ab071e2c0fecf3f018c1cd747571b0f3a6a48` |
| `rust_isolation_probe.py` | `fa693964a85b486068cfa3fdde516cf540b596c6a681019ed9231e29eb662569` |

Product CI, Cargo-deny, silent-partition behavior and full-flow performance were
not run. This experiment does not establish an organization-wide process count,
latency budget, hostile same-UID isolation, authenticated client transport,
same-store exclusion, fresh unlock after failure or central server availability.
Normal Linux custody remains unavailable. Authenticated IPC, one-session-per-store
enforcement and explicit same-store recovery evidence remain integration gates;
the profile continues to trust the endpoint administrator and OS account.

### Inherited custody control channel, 2026-11-12

The [candidate IPC contract](LINUX-CUSTODY-IPC.md) defines a private inherited
SEQPACKET channel between already known processes. It matches each message's
kernel credentials and a retained live peer pidfd, a validated local object
selection, a fresh bootstrap generation and strict sequences in a fixed
112-byte control frame. It does not discover sessions or expose credential
signing, unlock, descriptor transfer or same-store replacement.

The [native IPC fixture](../tools/native-key-store-probe/review/RUST-IPC.md)
observed ten cases on Ubuntu 24.04 amd64. Roundtrip accepted two probes and
replay accepted its first probe before refusing the repeated sequence. Wrong
generation/store, a forwarded descriptor's different sender, received rights,
dead peer, live-peer timeout, short frame and oversized frame accepted no probes.
The responder's FD count stayed 11 before and after rejecting SCM_RIGHTS. Owned
host/responder/injector exits were observed before final cleanup. Raw injection
tests require responder refusal; they do not attribute an error to a client
Channel object bypassed by the injector.

The new Linux-test-only source and validated PackageLease identity getter passed
66 auxiliary Rust tests, formatting and Clippy with warnings denied in debug and
release, offline on Rust 1.89. Final Python checks passed 312 tests with one
POSIX-only test skipped on Windows. No dependency or MSRV change was introduced.
The getter does not retain another Arc loan or replace native signing/lease
validation. Full product CI, Cargo-deny and full-flow performance were not run.

Three native runs shared one helper and observed 19 cases with exit 0:

| Experiment | Cases | Scope |
|---|---:|---|
| Inherited IPC | 10 | Known-process control handshake, message identity/binding, sequence and refusal cases |
| Loaded-session regression | 8 | Previous lock, lease and abandonment cases; independent loaded sentinel survives |
| Process-boundary regression | 1 | Uncertain executor remains isolated from its host and independent executor |

Each runner checked CPU1, 512 MiB RAM, no extra swap, 128 PIDs, bounded tmpfs,
no network, unprivileged execution and NNP. None reported OOM or selected RDP
error events. A final selected-log query from 21:48:47 through 22:37:54 UTC
returned no events; this does not diagnose or exclude all RDP failures. Fourteen
owned containers, including the expected red-test phases, and three temporary
stores were absent after verification. Report volumes and the build volumes
remain retained.

Two expected test-first failures remain separate: a valid frame rejected by the
initial stub, and an installed pidfd not closed by the initial ancillary parser.
They exited 101 and are not counted as successful validation. The final parser
closes and rejects both visible SCM_RIGHTS and SCM_PIDFD records. SCM_PIDFD and
truncated ancillary cleanup are synthetic parser tests with real descriptors;
the native descriptor-receive case is SCM_RIGHTS. The factory does not enable
SO_PASSPIDFD, and no native SCM_PIDFD receive is claimed. Review also removed an
unbounded EINTR retry in the zero-time peer-liveness query before final testing.

Local evidence under workspace `.scratch/`:

- `rust-ipc-build-20261112-a`: seven final build phases, Python output,
  twelve-source manifest, checked evidence index and final RDP query.
- `rust-ipc-native-20261112-a`: ten native IPC observations.
- `rust-ipc-session-regression-20261112-a`: eight loaded-session controls.
- `rust-ipc-isolation-regression-20261112-a`: one process-boundary control.
- `rust-ipc-red-20261112-a` and `rust-ipc-pidfd-red-20261112-a`: expected failures
  retained independently of the final green build.

All twelve selected shared/fixture Rust source hashes matched after runtime
verification. Selected identities for this increment:

| Identity | SHA-256 |
|---|---|
| Helper | `a2f5892bc947b2f45e4e79c9ce611e0846c43d1a3f27f3623af8e2b4cd190a4c` |
| `src/device_keys/linux_ipc.rs` | `c2f2c6f9b609a9b33f91fcee1b14a81e36d9db2c5768c748e0c31d1bb6c11596` |
| `src/device_keys/linux_session.rs` | `1e5b651ceb200b629c53495711ac8355af1c2c729f36445ad595d5d34b124ae9` |
| `rust-launcher/src/ipc_probe.rs` | `966ae5e3e3c7d0c5a573ef405d361bc87826e4fba841afba5a46d65334cda744` |
| `rust_ipc_probe.py` | `2acf159dfb1caa0479f4711961231dd6e9a6cfe2a37270a3200cf269c7652158` |

Kernel identity within a trusted endpoint is not organizational authorization
or fresh human approval. Pathname-based discovery, per-store cross-process
exclusion, preserved exit evidence and explicit same-store recovery remain
integration gates. Normal Linux custody and the central server are not enabled
by these results.

## Cooperative Linux directory reservation observation (2026-11-13)

The [reservation candidate](LINUX-CUSTODY-RESERVATION.md) was exercised with
public disposable markers on Ubuntu 24.04 amd64. It uses one nonblocking flock
on a fresh directory open file description, limited to the ext filesystem
family. A retained independent PackageLease snapshot binds the reservation to
the selected objects. SpawnOwner validates it before admission and before each
native spawn, retains it through uncertain abandonment and releases it after
observed native exits on successful shutdown.

The six new cases observed same-directory contention despite shared original
descriptors, independence of another directory, normal release with a live
owner, reservation retention after uncertain abandonment, anchored pathname
replacement and in-place invalidation. The explicit death control observed
successful readmission after the owner died while its unlinked public child
remained alive and answered a probe. The observer then independently killed
and reaped that exact child using its retained pidfd. This proves why lock
release cannot authorize session replacement or stand in for native exit
evidence. It is not a production unlinked-child option.

| Experiment | Cases |
|---|---:|
| Directory reservation | 6 |
| Loaded signing-session regression | 8 |
| SSH signing-session regression | 12 |
| Native launcher regression | 7 |
| Loader regression | 8 |
| SSH loader regression | 10 |
| Bounded abandonment regression | 3 |
| Inherited IPC regression | 10 |
| Process-boundary regression | 1 |

All 65 native cases completed with observed exit conditions and runner exit 0.
The auxiliary crate, rebuilt after cleaning, passed 71 Rust tests, formatting
and Clippy in debug and release offline with Rust 1.89. Python checks passed
325 tests with one POSIX-only test skipped on Windows. These checks do not
constitute the product's complete CI matrix: full cross-platform nextest,
product MSRV/lints, Cargo-deny and full-flow performance were not run for this
increment. The changed candidate remains Linux-test-only in the product.

Each runtime retained CPU1, 512 MiB RAM with no extra swap, 128 PIDs, bounded
tmpfs, no network, unprivileged execution and NNP. No runner reported OOM or
selected RDP errors. The final selected-log query from 04:53:00 through 05:13:47
UTC returned no events; this does not diagnose the earlier RDP failure or
exclude failures outside those logs. All twenty owned containers and nine
temporary store volumes were absent after verification. Nine report volumes
and the separate build volumes remain retained.

Local evidence under workspace `.scratch/`:

- `rust-reservation-build-20261113-b`: seven final build phases, fourteen-source
  manifest, checked evidence index and final selected RDP query.
- `reservation-python-20261113-a.log`: Python verification output.
- `rust-reservation-native-20261113-a`: six reservation observations.
- `rust-reservation-session-regression-20261113-a` and
  `rust-reservation-ssh-session-regression-20261113-a`: loaded session controls.
- `rust-reservation-rust-EXPERIMENT-regression-20261113-a`, where EXPERIMENT is
  `launcher`, `loader`, `ssh-loader`, `abandon`, `ipc` or `isolation`: remaining
  native controls.
- `rust-reservation-red-20261112-a`: initial stub failure, exit 101. After an
  interrupted host turn, the exact exited container's state and output were
  recovered into `recovery.json`; no normal phase report was fabricated.
- `rust-reservation-build-20261113-a`: a subsequent helper type error, exit 101,
  retained separately from the corrected clean build.

All fourteen selected shared/fixture Rust hashes matched after verification.
Every native report used helper SHA-256
`bb73bbbf7c1844fd712f630dc1ee70c611133633ddb1ba93a92c9eec166d1768`.
The reservation observer SHA-256 was
`e81edaf56663349a35e4e42b130dd76e1685b55f72708be5f7a30210f2c1f466`.

Cooperative physical-directory exclusion does not establish organizational
permission, uniqueness of copied credentials, endpoint discovery or preserved
exit evidence. Operational replacement, fresh unlock and central-server
integration remain separate gates. Normal Linux custody stays disabled.

## Connected-peer identity: incomplete native observation (2026-11-13)

The [connected-peer candidate](LINUX-CUSTODY-CONNECTED-PEER.md) adds a kernel
identity acquisition path for a connected AF_UNIX SOCK_SEQPACKET socket. It
uses SO_PEERCRED and SO_PEERPIDFD from the socket without selecting a process
from an advertised PID. It retains existing per-message credential and peer
liveness checks and does not grant endpoint authority.

The clean auxiliary build passed 74 Rust tests, formatting and Clippy in debug
and release offline with Rust 1.89. Python checks passed 342 tests with one
existing POSIX-only test skipped on Windows. All ten inherited IPC regression
cases were observed with runner exit 0. Full product CI, cross-platform tests,
product MSRV/lints, Cargo-deny and full-flow performance were not run.

**The seven new native cases remain unaccredited.** Both attempts exited 1,
with zero completed cases, no OOM and all authority/custody/recovery/server/ACK
flags false. The Ubuntu 24.04 image shares kernel
`5.15.167.4-microsoft-standard-WSL2`; changing the container image did not change
that kernel. The explicit preflight returned errno 92 (ENOPROTOOPT) for
SO_PEERPIDFD. Unsupported capability is an incomplete failure, never a skip,
fallback or credited refusal case. A supported kernel is required to test
normal exchanges, bindings, forwarded senders, peer exit and historical
listener behavior. This observation establishes no such runtime success.

Local evidence under workspace `.scratch/`:

- `rust-connected-build-20261113-a`: seven successful auxiliary build phases,
  fifteen-source manifest, checked evidence index and selected RDP query.
- `connected-python-20261113-b.log`: final Python checks.
- `rust-connected-ipc-regression-20261113-a`: ten observed legacy control cases.
- `rust-connected-native-20261113-a`: incomplete first attempt, generic
  ValueError before the observer received its expected readiness report.
- `rust-connected-native-20261113-b`: incomplete explicit capability diagnostic,
  original errno preserved as KernelCapabilityUnavailable.

All fifteen selected shared/fixture Rust hashes matched after verification.
All reports used helper SHA-256
`1675891688728a1c12228ac8bd6a9b1a4c93ac7b58440207add9a10ed748afe0`.
The final connected observer SHA-256 was
`02d93b215db7957698ca6111867713a856bbcc46b41533554598b34896d6cad5`.

The isolated runs retained CPU1, 512 MiB RAM with no extra swap, 128 PIDs,
bounded tmpfs, no network, unprivileged execution and NNP. The selected RDP
query from 11:00:00 through 11:21:01 UTC returned no events; it does not diagnose
the earlier RDP failure. Ten owned containers and three temporary store volumes
were absent after verification. Three report volumes and the build volume
remain retained. No global WSL/RDP configuration or installed product changed.

Identity of a socket peer is still distinct from selecting an authorized
custodian. Endpoint bootstrap, seven-case native accreditation, preserved exit
evidence and operational replacement remain integration gates. Linux custody
and central-server operation are not enabled by this increment.

## Connected-peer identity on a separate guest kernel (2026-11-13)

The subsequent [connected-peer experiment](../tools/native-key-store-probe/review/RUST-CONNECTED.md)
observed all seven native cases on Ubuntu kernel 6.8.0-149-generic, using the
same Rust helper as the earlier WSL2 attempt. Two normal control probes were
accepted; wrong generation/store/roles, forwarded senders, independently
observed peer exit and historical listener identity produced their required
refusals. A timeout was not credited as a valid rejection. Endpoint authority,
custody, recovery, MCP/server and lock acknowledgements remained false.

QEMU TCG ran in an unprivileged, capability-free, NNP, read-only-root container
with no network, CPU1, 512 MiB RAM with no extra swap and 128 PIDs. Its explicitly
configured writable temporary mount was a 128 MiB tmpfs at `/var/tmp` for the disposable disk
snapshot. The guest used 256 MiB RAM, one vCPU, no NIC and a 16 MiB TCG translation
cache. The unchanged observer required its original private PID-1 deadline,
UID/GID 65534, NNP, no effective capabilities, read-only root, loopback-only
network, finite cgroups and dedicated ext-family store. The observed inner
cgroup limits were CPU1, 128 MiB RAM, no extra swap and 128 PIDs; `/tmp` was a
128 MiB tmpfs. The ext4 store was a fresh 32 MiB snapshot.

The final attempt exited 0 in both guest and outer container, without OOM.
Six outer memory samples ranged from 167.9 to 261.3 MiB under a preventive
448 MiB sample threshold. This does not establish an absolute peak or a product
latency budget. The initial TCG configuration had reached a 511.8 MiB sample
and recorded OOM/exit 137; that attempt is retained as a failure. It is not
attributed to the earlier RDP incident. After bounding the translation cache,
the same guest payload completed within the original deadlines.

Preparation initially encountered APT-directory permissions, absent `ldd` and
minimal-image path assumptions. Those failures were resolved in auxiliary
scripts without adding host capabilities or bypassing package verification.
An initial boot reported PermissionError; the bootstrap stopped running mdev
over devtmpfs nodes, and the final boot observed `/dev/null` mode 0666. Python
failure diagnostics were extended with errno, checkpoint and bounded code-frame
identifiers, without messages, full paths or changed acceptance conditions.

The earlier clean Rust 1.89 auxiliary build still supplies the exact helper:
74 tests, formatting and Clippy in debug/release. All fifteen selected shared
and fixture Rust source hashes still matched. Final Python checks passed 344
tests with one existing POSIX-only omission on Windows; two auxiliary asset
integrity tests passed separately. The ten earlier inherited-IPC regressions
used the same Rust artifact on the WSL2 kernel; they are separate from these
seven guest-kernel cases. Full product CI and full-flow performance were not run.

Local evidence under workspace `.scratch/`:

- `kernel-probe-20261113-h`: final seven-case report, outer configuration,
  memory samples, source/cleanup evidence and final selected RDP query.
- `kernel-prepare-20261113-b`: successful bounded package preparation and
  local image identity; `kernel-prepare-20261113-a` retains its initial failure.
- `kernel-assemble-20261113-e`: selected guest manifest. The prior successful
  bundle remains byte-exact in the retained asset volume, alongside failed
  assembly stages and retention receipts.
- `kernel-probe-20261113-a` through `g`: incomplete auxiliary attempts,
  including the PermissionError boot and the OOM attempt. None accredits cases.
- `kernel-python-20261113-a.log`: final Python suite output.
- `kernel_driver_20261113.py`, `kernel_exec_20261113.py`,
  `kernel-prepare-20261113.sh`, `kernel-assemble-20261113.py` and
  `kernel-guest-init-20261113.sh`: retained local preparation/runtime sources.
  `test_kernel_exec_20261113.py` contains the separate integrity controls.

| Selected identity | SHA-256 |
|---|---|
| Helper | `1675891688728a1c12228ac8bd6a9b1a4c93ac7b58440207add9a10ed748afe0` |
| Observer | `0c6395d782a434334668841421dffb115d2692650029a43b41f0669ef6ca14e8` |
| Guest manifest | `5f89bce28229759c4cb120a8c71e013e475946e85b36a2aefae2db579db89add` |
| Ubuntu kernel | `9e9bb539fbf5e25c3fa737180e3586dacd0922257cdffe319daf8e5ceecaeaf0` |

The selected prepared image was
`sha256:348f601bf71a5cd855cd4afd19eae631d6b3b5def45b8d26c0a876e8da493d32`.
The dedicated retained asset volume is
`vivac-kernel-review-bf76cb957fd043ec986090b6203c8ea3`. Kernel and module
identities are indexed in its package manifest, copied into the final evidence.
Before execution the driver bound the selected preparation and assembly
receipts to that image/volume and checked the explicit manifest digest; an
in-container checker verified the three guest artifacts before starting QEMU.
This assumes a trusted host and no concurrent writer to the selected volume.
It is neither remote attestation nor a hostile-host confidentiality claim.

Sixteen owned containers, including the never-started metadata-export
container, were absent after verification. The asset volume and auxiliary
image remain retained. The selected RDP query from 12:00:00 through 12:45:06 UTC
returned no events; this does not exclude every RDP failure or diagnose the
previous incident. Global WSL/RDP settings and the installed product were
unchanged. Connected identity is now observed under the stated guest profile;
trusted endpoint bootstrap, native-exit evidence across supervisor failure and
operational replacement remain separate gates. Normal Linux custody remains
disabled.

## Generated creation and resident reopening in the Ubuntu guest — 2026-11-19

Two runs used separate disposable stores in Ubuntu 24.04 with kernel
`6.8.0-149-generic`, OpenSSH 9.6p1, QEMU TCG and one virtual CPU. Both runs
used the same selected writer and reader binaries. The observer measured
monotonic elapsed time around its existing checks. It sent synthetic
confirmation and unlock input through the corresponding terminals.

| Observed interval | A (s) | B (s) |
|---|---:|---:|
| Producer spawn to acknowledgment prompt | 2.345 | 1.924 |
| Acknowledgment prompt to synthetic confirmation sent | 0.065 | 0.060 |
| Confirmation sent to verified publication and producer exit | 28.678 | 27.265 |
| Verified publication to independent admission read | 11.461 | 11.242 |
| SSH transport setup to first verified channel | 15.448 | 14.642 |
| First channel to unlock prompt | 9.371 | 8.765 |
| First unlock prompt to synthetic input sent | 0.162 | 0.132 |
| First input sent to proof endpoint available | 4.964 | 4.530 |
| First endpoint available to independently verified signature | 6.670 | 6.750 |
| First release command to verified close permit | 4.232 | 4.060 |
| Verified close permit to first SSH client and keeper exit | 0.261 | 0.218 |
| Second SSH connection to verified channel | 10.293 | 10.837 |
| Second channel to unlock prompt | 5.025 | 5.001 |
| Second unlock prompt to synthetic input sent | 0.053 | 0.055 |
| Second input sent to proof endpoint available | 3.971 | 4.094 |
| Second endpoint available to independently verified signature | 5.853 | 6.016 |
| Second release command to SSH client and keeper exit | 0.279 | 0.278 |
| Third SSH connection to verified channel | 10.444 | 11.058 |
| Third channel to refused opening and exit | 1.731 | 1.733 |

Publication includes native loading, proof and closure checks as well as
storage checks. Independent admission includes reading the initial permit and
the durable record. SSH transport setup includes generating synthetic keys and
configuring the loopback server. Each proof interval includes a separate verifier process.
The third connection was refused before an unlock prompt in both runs.

The guest fixture cgroup reported memory peaks of 68,325,376 and 68,263,936
bytes against a 134,217,728-byte limit, with zero OOM events. Its CPU usage was
155.334 and 148.977 CPU-seconds. Those counters cover setup and the full
observer process group. The enclosing containers were removed.

These timings characterize the emulated fixture, including its observer
checks. Performance admission requires measurements of the integrated flow
on the pilot runtime. The raw nanosecond intervals, resource counters and
validation receipts are retained in the workspace under
`.scratch/keeper_ssh_probe_measure_20261119` and
`.scratch/keeper_ssh_probe_measure_retry1_20261119`. The isolated runner
selects this case with `keeper_ssh_probe.ps1 -Trial measure` and a fresh run
suffix for each additional repetition.

## Integrated Linux binary in the Ubuntu guest — 2026-11-21

One complete local run validated a monotonic-clock observer against the Vivac
release binary in an isolated Ubuntu 24.04 guest with kernel
`6.8.0-149-generic`, QEMU TCG and one virtual CPU. The binary orchestrated
attended creation, a new unlock through SSH loopback, independent endpoint
verification and clean close. The observer checked the opening permit and
admission before and after the test. An earlier attempt ended before the
unlock prompt and yielded no complete sample.

| Observed interval | Elapsed (s) |
|---|---:|
| Confirmation sent to creation process exit | 22.594 |
| SSH client spawn to authenticated channel marker | 3.516 |
| Channel marker to unlock prompt | 8.296 |
| Unlock sent to active prompt | 9.280 |
| Independent proof process | 6.795 |
| Close sent to SSH client exit | 2.830 |

The fixture cgroup used 57.864 CPU-seconds between the recorded snapshots.
Its memory peak was 49,344,512 bytes (47.1 MiB) against a 128 MiB limit, with
zero OOM events. These counters include the observer and loopback SSH. The
complete record contains 12 intervals, binary identity and before/after
counters. Synthetic input writes are timed separately and do not measure
human response time. The run validates the measuring fixture under TCG.

## Integrated Linux binary with hardware acceleration — 2026-11-21

Two fresh runs used the same Vivac ELF and Ubuntu 24.04 amd64 guest image with
kernel `6.8.0-149-generic`. QEMU 11.1.0 used WHPX exclusively with one guest
vCPU. The host runner observed Windows debug events for its own QEMU child
inside a bounded Job and validated the final-exception path with a synthetic
process before launching the guest. The guest waited for kernel random
readiness before provisioning, outside the measured intervals.

| Observed interval | A (s) | B (s) |
|---|---:|---:|
| Confirmation sent to creation process exit | 2.818 | 2.413 |
| SSH client spawn to authenticated channel marker | 0.188 | 0.198 |
| Channel marker to unlock prompt | 0.330 | 0.336 |
| Unlock sent to active prompt | 0.595 | 0.645 |
| Independent proof process | 0.135 | 0.150 |
| Close sent to SSH client exit | 0.207 | 0.251 |

The fixture group used 3.574 and 3.345 CPU-seconds between snapshots. Its
memory peaked at 42,733,568 and 42,741,760 bytes against a 128 MiB limit, with
zero OOM or OOM-kill events. Both runs independently verified the endpoint,
preserved the admission and restored the clean opening permit. The complete
receipts are in `.scratch/t4056-whpx/series-{6,7}/receipt.json`.

These are local measurements of the integrated Ubuntu profile. The proof
interval includes process launch and reading the retained admission. The
creation interval includes loading, verification, closure and publication.
Each interval has one observation per run. The results describe this
workstation and guest profile. Deployment on the pilot server still requires
validation in its own environment.
