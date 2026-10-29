# Local backups and recovery

A recovery copy preserves the tree and the files it has custody of. It is not
`tree --json`, a copy of the working tree, a Git backup or a synchronization
service. Nothing is uploaded. Local packages are not encrypted; their directory
is created with private permissions and excluded from Git by default. Keep them
on storage whose access you control. A copy on the same disk does not protect
against loss of that disk.

Backup destinations and recovered workspaces must be outside existing Git
working trees. A fresh directory name alone does not prove that its future
members are absent from a repository's index. Private preparations receive
their ignore file before any payload bytes are written.

## Contract

The package includes the exact complete event-log prefix and configuration,
the owning lane marker where present, every acknowledged artifact revision and
the native files still required by managed agent contracts. Historical identities, lanes,
repository anchors, safepoints and recorded inspection policies remain intact.
Artifact bytes remain separate from the event log. Agent prompt bodies are
copied as native files, never inserted into events or diagnostic output.

Repository contents, the global registry, credentials, harness settings,
derived indexes, locks, temporary files and uncommitted artifact objects are
not recovery inputs. Detached agent files remain outside custody. A missing,
changed, unsupported or inconsistent required input refuses the operation;
there is no successful partial backup. A historical reference to bytes that
were never retained does not acquire new content or invented provenance.

Agent and artifact files can belong to different lanes. The caller supplies
the physical root of each additional lane whose files must be read:

```text
vivac backup create <new-directory> [--lane-root <lane-id>=<directory>] [--json]
vivac backup verify <backup-directory> [--json]
vivac backup recover <backup-directory> --to <new-directory> --source-retired [--json]
```

Creation uses the tree owning the current folder. Additional roots must already
belong to that tree and lane; they do not authorize scanning other projects.
Missing roots are reported instead of guessing or looking through the disk.
The backup stores lane identities and relative member paths, not the global
registry or machine-specific discovery paths.

## Coherence and integrity

Creation takes the tree writer lock long enough to capture input handles and
the log boundary. Validation and event folding run after releasing the lock.
That boundary must end at a complete event. It copies the append-only prefix
through its opened handle and checks every required file against its captured
digest. Later events are not
part of this copy. Corrupt or incomplete input is refused, never repaired as a
side effect of backup or verification.

Creation, verification and recovery apply the provenance privacy guards to
decoded strings and keys in the log, configuration and lane markers, including
unknown fields. Rejection does not rewrite history or echo the rejected content.
Artifact payloads retain their recorded inspection policies; native agent files
use their own guards, which permit prompt formatting.

The versioned manifest lists bounded relative paths, sizes and SHA-256 digests.
Payload files use fixed opaque names. Verification checks framing, identity,
the complete file set, sizes, digests, event folding and custody references.
Links, reparse points, hard links, traversal, duplicate destinations and unknown
formats are refused. Hashes detect inconsistent bytes; they do not authenticate
an untrusted package or encrypt its contents.

Preparation happens in an owner-private sibling directory. Files and directory
metadata are synchronized before an exclusive publication of the destination.
Failure leaves no successful partial destination and never overwrites source
data. An interrupted preparation can leave a private temporary directory;
verification does not mistake it for a published backup.

## Recovering authority

Verification only reads a package. It does not register a copy as a project.
Recovery requires a new, absent workspace directory and `--source-retired`:
the caller declares that the original tree is no longer an active authority.
It still refuses any live copy with the same identity known to the local
registry. A declaration cannot override that refusal. Copies on another machine
or outside local discovery are not a distributed lease; they must be retired
by the owner before recovery.

Here, a known live copy means a tree still present at a recorded location, not
merely a running process. Stop its writers and keep the old store in private
quarantine outside active tree locations before recovering. Verification can
exercise the copy while the original remains healthy; recovery does not move,
delete or overwrite that original for you.

The source tree is registered when creating a recovery package. Recovery
reserves its new registry location under the registry lock before publishing
the new tree. A crash before publication can leave a dead discovery entry, not
an active second tree. The registry remains local and is never imported from
the package.

The owning lane is recovered at the new workspace root. Other included lanes
are placed under `lanes/<lane-id>/` with their own markers and native files.
Their historical identities do not change. Existing repository anchors remain
historical references; repository contents must be recovered separately.
Recovery does not launch agents, install hooks, synchronize harnesses or run
artifact programs. The recovered workspace is excluded from Git by default.

## Bounds

The manifest is at most 512 KiB, with at most 4,096 payload files. A single file
is at most 256 MiB and the complete payload at most 64 GiB. Member paths are at
most 1,024 bytes. Tree configuration is at most 64 KiB and a lane marker at most
8 KiB; these metadata limits apply before parsing or copying them. Streaming
uses at most 64 KiB per copy buffer. Supported artifact and native-agent input
profiles retain their existing inspection
limits; the package does not enlarge them. Exceeding a bound refuses the whole
operation.

## Recovery acceptance

An isolated recovery drill must preserve raw events and configuration, all lane
identities, safepoints, anchors, artifact revision bytes and managed native
files. It must work after the original tree has been made unavailable, leave
the original evidence unchanged, and reject recovery while a known original is
still live. Fault cases include missing custody bytes, stale agents, a partial
event tail, unknown format, modified payload, extra files, path attacks,
existing destinations and failed publication. All checks use synthetic fixtures;
development does not export this workspace's private tree.

An owning tree with no lane marker retains its implicit `main` lane. Recovery
preserves that absence instead of inventing a different lane identity.
