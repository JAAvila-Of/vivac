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
cargo clean -p vivac
cargo test --locked --release measure_one_hundred_complete_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --nocapture --test-threads=1
cargo test --locked --release measure_one_hundred_receipt_commits_after_external_writes -- --ignored --nocapture --test-threads=1
cargo test --locked --release measure_one_hundred_resident_receipt_commits_after_external_writes -- --ignored --nocapture --test-threads=1
```

For complete file capture, `tools/measure-artifact-custody.py` runs the Rust
binary in fresh processes against exact 1 MiB and 10 MiB synthetic UTF-8 files.
It verifies retained sizes and digests after every sample. The default fixture
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
cargo clean -p vivac
cargo test --locked --release --bin vivac artifacts::bundle::tests::retained_bundle_release_benchmark -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --release --bin vivac artifacts::tests::measure_one_hundred_complete_bundle_receipt_commits_with_ten_thousand_prior_artifacts -- --ignored --exact --nocapture --test-threads=1
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
