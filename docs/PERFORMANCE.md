# What it costs

The [performance pillar](PILLARS.md) gives a read **50 ms** and a write
**5 ms**. What follows is every number behind the summary in the
[README](../README.md#what-it-costs), including the two places the budget is
missed.

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
