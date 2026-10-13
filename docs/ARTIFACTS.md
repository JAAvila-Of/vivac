# Local artifact custody

An artifact delivery connects a producer node to a selected revision of a file.
Vivac retains that revision locally so it can be recovered after the source
changes or disappears. The provenance tree holds identities, paths, sizes and
SHA-256 digests; file contents stay in a separate `.vivac-artifacts/` directory.

Custody does not imply approval. Review the retained revision, then record a
decision that references that exact revision. Later deliveries keep the same
artifact identity and receive new revision identities.

## Retain, review and recover a delivery

Capture a project-relative UTF-8 file with an explicit producer:

```sh
vivac artifacts capture design.html --from t12 --json
```

The reply includes `artifact`, `revision` and a reference of the form
`artifact:<artifact-id>@<revision-id>`. Substitute the returned identities in
the following commands:

```sh
vivac artifacts restore <artifact-id> --revision <revision-id> --to review-r1.html
vivac decide "Accept the reviewed layout" --reason "The person reviewed R1" --ref artifact:<artifact-id>@<revision-id>
```

Restore verifies the retained bytes and creates a new destination. It refuses
an occupied destination. Opening the restored file is a separate, deliberate
action; capture and restore never execute it.

After editing the source, retain the next delivery under the same identity:

```sh
vivac artifacts capture design.html --from t12 --artifact <artifact-id> --previous <revision-id> --json
vivac artifacts show <artifact-id> --json
vivac artifacts restore <artifact-id> --revision <revision-id> --to recovered-r1.html
```

The decision still references R1. Deleting `design.html` does not delete the
retained revisions. Deleting the local artifact store does: this feature is
local retention, not a backup service.

The first retained delivery marks the tree as requiring artifact-aware Vivac.
Older releases refuse to read that tree rather than silently omit its receipts.
Use a compatible release in every harness that shares the tree. The separate
local store must also be preserved to recover the retained bytes.

## Inspect custody

```sh
vivac artifacts
vivac artifacts list --all --limit 20 --offset 0 --json
vivac artifacts show <artifact-id> --revision <revision-id> --json
vivac artifacts check <artifact-id> --revision <revision-id> --json
vivac artifacts check --store --json
vivac artifacts check --store --limit 20 --offset 20 --json
```

The default list follows the current focus and its ancestors. `--from` selects
another node's lineage; `--all` selects the project. List and show return
metadata without reading file bodies. Their default page size is 20, with a
maximum of 100; follow `next_offset` for later pages.

Explicit checks compare actual bytes with the recorded digest and report when
they ran. A matching digest proves those bytes match the receipt; it does not
prove that a person approved them or that a harness used them. Source paths
recorded at another site are not checked against an unrelated local file.

The map marks nodes with compact note and artifact counts. Their icons have
distinct shapes and accessible labels. A node's artifact detail separates
revisions produced there from exact revisions referenced by the node; a later
delivery does not replace an earlier reference.

The Artifacts page keeps the last explicit check in the current browser tab's
session, including its date, retained-copy result and source result. Refreshing
the page can show that previous observation; it does not check bytes again.
Observations belong to the selected project, site and exact revision, and are
not recorded as approvals or written to the provenance log. If browser session
storage is unavailable, observations remain available only in the current page.

Store checks inspect every receipt and object even when their detail lists are
paged. A damaged revision outside the displayed page still makes the check
fail. Each detail category reports its total; `next_offset` continues the lists.
Unrecorded-object and temporary-file pages follow filesystem enumeration order.
Changing the store between requests can change those pages; this is an inventory
check, not a directory snapshot.

## Limits and refusals

The first implementation accepts UTF-8 text up to 10 MiB. Invalid UTF-8,
oversized input and content recognized by the sensitive-data guard are
refused. Binary formats are unsupported. The guard examines the complete
stream without truncating inspection; an oversized token is refused.
No automated detector can certify that an arbitrary document contains no
sensitive information. Select files deliberately under the project's rules.

The CLI capture loader folds the event log incrementally. It refuses a log
line larger than 512 KiB, including its newline, or more than 1,024 JSON keys
and values combined in one line. Containers count as values. These bounds apply
to loading the log for CLI capture, separately from the selected file's 10 MiB
limit. They do not truncate events or change ordinary indexed reads. Resident
MCP uses the tree its server already loaded.

If another writer appends while CLI capture retains the file, confirmation
incorporates the complete new log tail under the write lock. This refresh
accepts at most 64 KiB and 64 events. It checks the log's physical identity
and the digest of its last previously observed line before accepting new
events. A replaced or truncated log, a changed checkpoint, a timestamp change
without log growth, an incomplete or broken new line, an unknown event, or an
exceeded refresh limit refuses the
capture without a receipt. Run the capture command again to load fresh
context; bytes retained before the refusal may appear as unrecorded objects
in a store check. The refresh never applies only part of the new tail.
These checks do not make the log or source a filesystem snapshot.

Paths must stay beneath opened project-directory handles. Symlinks, Windows
reparse points, traversal, credential locations and unsupported file operations
are refused. Vivac preserves the source and never overwrites a retained object.
Recognized sensitive values in source paths are refused before retaining bytes,
including a sensitive value in an individual directory or filename component.
It detects observed changes during capture, but does not freeze a mutable source
or promise a coherent filesystem snapshot. Review the retained copy.

The local store has its own ignore rule. Vivac refuses custody if Git already
tracks store files or effective rules do not exclude the store. It does not
commit files, change global Git configuration or upload anything. Bytes are
not read by ordinary writes or hooks.
Tracking checks stay synchronous. Independent Git prefix and ignore queries
run in parallel with one short-lived thread requesting a 128 KiB stack. Both
queries finish and their bounded outputs are validated before proceeding;
the checks are repeated at every existing capture boundary, without caching
Git results.

## Interrupted captures

Retained bytes are synchronized before their receipt is appended. The event
log does not provide a durable transaction with the separate store. A crash or
concurrent conflict can leave unrecorded objects or temporary files:

```sh
vivac artifacts check --store --json
vivac artifacts restore --orphan <unrecorded-revision-id> --to rescued.txt --json
```

An unrecorded object has unknown provenance; rescue does not invent a producer,
decision or receipt. Store checks remain available with an incomplete log and
label unknown objects as unattributed rather than proving they were never
recorded. Explicit rescue by unrecorded object identity remains available with
an incomplete log and reports unknown provenance. It refuses an identity that
appears in a receipt, including an ambiguous receipt. Normal capture, listing
and restoration by artifact identity require a readable log.
Vivac does not automatically delete objects or repair the log.

For retryable integrations, supply a unique opaque ULID with `--operation`.
Repeating an already recorded operation with the same source and producer
returns its original receipt without capturing the changed source again.
An explicitly supplied artifact identity or previous revision must also match
that original receipt; contradictory retries are refused.

## MCP

`vivac_artifacts` exposes `list`, `show`, `capture`, `check`, `restore` and
`capabilities` with
the same file custody guards and local scope as the CLI. Capture requires
`path` and `from`; restore requires `to`. Replies contain `exit_code` and
structured `result`.
File bodies are never returned through MCP or added to the tree.

## Discover capture support

```sh
vivac artifacts capabilities --json
```

The query reports the installed engine's supported delivery types, effective
size and pagination limits, and execution policy. It works outside a project
and does not load the provenance tree or inspect file bodies. MCP exposes the
same result through `vivac_artifacts` with `operation: "capabilities"`.

The current engine retains one UTF-8 file per revision, up to 10 MiB. A
self-contained HTML file can include JavaScript and remain a single delivery.
Complete multi-file bundles and SQLite snapshots are unsupported. Recognizing
their filenames does not make them eligible for custody. Retaining a supported
code file does not prove that an entire application can be recovered or run.
Vivac does not execute deliveries or install their runtime dependencies.

## Incorporate existing deliveries with a skill

`vivac setup claude-code` and `vivac setup codex` install the optional
`vivac-artifacts` workflow in their native project skill directories. Setup
installs instructions; it does not search for or capture existing deliveries.
Ask your harness to use the skill with selected paths, for example:

> Use vivac-artifacts to review designs/checkout.html and retain it if the
> installed engine supports it. Propose the producer before capturing.

The workflow checks installed capabilities, preserves the person's selection
and uses the artifact engine for capture. If the historical producer is unknown,
it records the incorporation task without inventing the delivery's origin.
It can also retain a reviewed new revision of an existing identity. File
bodies stay outside the log; custody does not execute content or approve it.

For direct operation, use `vivac artifacts` or the project's Artifacts page.
Unchanged skills written by setup can be refreshed by setup and removed by
`vivac setup codex --undo` or `vivac setup claude-code --undo`; local edits and
foreign copies are preserved.

## Manage deliveries in the web

Open a project in `vivac web`, then choose **Artifacts**. The inventory shows
the latest delivery and decisions referencing its revisions separately. An
earlier decision reference does not apply to a newer delivery. The node's
lineage page also links to deliveries in that scope.

Select an artifact to inspect a particular revision, its producer and its
references. Integrity checks are explicit and show when they ran; metadata
alone does not prove the state of the working file or retained copy. Capture
requires a project-relative source and producer node. A new revision continues
the latest revision reported by the engine, preserving the selected older one.
Restore creates a new project-relative destination and refuses occupied paths.

Download recovers the exact selected revision as an attachment. The engine
copies and verifies it incrementally into a private temporary file and removes
its name before sending bytes. The browser streams the attachment without
opening it inside vivac; the server does not buffer the whole delivery. A
failed verification or unsupported unlink operation refuses the download. A
crash during preparation can leave a temporary object for a store check to
report. Download and restore do not create new receipts or imply approval.

Operations require the web session and same-origin authorization. A failed
network response does not prove that a capture was never recorded; inspect
the revisions before retrying an uncertain capture. Supported types and
limits come from the installed engine's capabilities query.
