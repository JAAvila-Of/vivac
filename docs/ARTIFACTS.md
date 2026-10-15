# Local artifact custody

An artifact delivery connects a producer node to a selected file or complete
frontend bundle revision, optionally with a contained SQLite database.
For preview tasks, prefer self-contained HTML or an HTML/CSS/JS frontend without
a backend. Reports, Markdown documentation, JSON results, SQL scripts, ordinary
source and configuration are not artifact deliveries by default. Retain those
files only when the person explicitly selects them as deliveries. UTF-8 support
is a format capability, not a reason to capture every file.
Vivac retains that revision locally so it can be recovered after the source
changes or disappears. The provenance tree holds identities, paths, sizes and
SHA-256 digests and brief descriptions; file contents stay in a separate
`.vivac-artifacts/` directory.

Custody does not imply approval. Review the retained revision, then record a
decision that references that exact revision. Later deliveries keep the same
artifact identity and receive new revision identities.

## Retain, review and recover a delivery

Capture a project-relative UTF-8 file with an explicit producer:

```sh
vivac artifacts capture design.html --from t12 --description "Preview the checkout layout." --json
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
vivac artifacts capture design.html --from t12 --description "Preview checkout with delivery options." --artifact <artifact-id> --previous <revision-id> --json
vivac artifacts show <artifact-id> --json
vivac artifacts restore <artifact-id> --revision <revision-id> --to recovered-r1.html
```

The decision still references R1. Deleting `design.html` does not delete the
retained revisions. Deleting the local artifact store does: this feature is
local retention, not a backup service.

Every capture requires `--description` (MCP `description`): a nonempty, single
line of up to 280 Unicode characters explaining what the delivery does and what
it is for. Surrounding whitespace is trimmed; control characters and sensitive
values are refused before retaining bytes. The description belongs to the exact
revision, appears in list/show and the web, and is included in retry matching.
A retry with the same operation identity and a different description is refused.
Use the same description when retrying the same capture.

Historical receipts without descriptions remain readable and recoverable. The
web shows **No description recorded.** for those revisions; it does not derive
a description from a filename or copy a later revision's text. New file and
bundle captures use versioned receipt types, `artifact.captured_v2` and
`artifact.bundle_captured_v2`. Readers that do not recognize them refuse the log
rather than silently discard the descriptions. Use a compatible engine in every
harness before creating these receipts in a shared project.

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

## Preview an exact HTML revision

```sh
vivac artifacts preview <artifact-id> --revision <revision-id>
vivac artifacts preview <artifact-id> --revision <revision-id> --no-open --json
```

Preview verifies and serves the selected retained `.html` or `.htm` revision,
including an earlier revision whose source has changed or disappeared. It
does not serve the current source or inherit an approval. The CLI prints the
preview URL and stays running until the preview is closed or stops accepting
requests. `--no-open` leaves opening that URL to the caller.

On the Artifacts page, **Preview latest revision** opens the revision named by
that table row. **Preview selected revision** opens the exact revision shown
in the detail, including an earlier revision. Both open a separate preview
page; **Run HTML** starts its JavaScript. Checking integrity and downloading
an attachment remain separate actions.

The authenticated web interface accepts only its canonical `127.0.0.1` host;
the preview uses `localhost` on a separate loopback port. The distinct hostnames
keep Vivac's host-only session cookie out of preview requests. The preview
exposes the verified delivery rather
than project-file or tree APIs. The HTML runs in a sandboxed frame without
access to Vivac's session or storage. Browser policies restrict outgoing
connections, external resources, form submissions, nested frames, popups and
artifact-initiated downloads. Inline JavaScript and styles, and embedded
images, fonts and media can support a self-contained interactive mockup.

These browser restrictions do not guarantee complete network isolation.
Preview is not a virtual machine, a general application runtime or a proof
that the HTML is safe. Dependencies on external files, APIs, persistent
storage or a backend can prevent the delivery from functioning completely.
Vivac does not install those dependencies.

At most four preview servers can be active in one Vivac process, each handling
one connection at a time. Each server expires after fifteen minutes. Requests
have a two-second deadline and responses a ten-second deadline, both shortened by
the preview's expiry. **Close preview** ends the view and releases its verified
reader. Closing can wait for the current request or response deadline.
Closing a browser tab alone does not stop its preview server; stopping the
owning Vivac process ends it.
The server's expiry does not stop JavaScript already loaded in a browser tab.
Close that tab to stop the loaded HTML.

## Complete static bundles

A static bundle retains its explicitly selected HTML, CSS and JavaScript files
as one complete revision. Its HTML entry point and every member belong to that
revision; changing, adding, deleting or renaming a member produces another
complete revision of the same artifact. A decision naming an earlier revision
does not approve the new one.

Create a project-relative selection file, for example `card.selection.json`:

```json
{
  "root": "designs/card",
  "entrypoint": "pages/index.html",
  "members": ["pages/index.html", "assets/style.css", "assets/app.js"],
  "requirements": []
}
```

Members are relative to `root`; `.` selects the project root without selecting
other files automatically. The selector does not supply trusted hashes, artifact
identities or a producer. Unknown selector fields, globs and implicit directory
capture are refused. Selection does not prove that an application has all its
dependencies or that it works.

```sh
vivac artifacts capture --bundle card.selection.json --from t12 --description "Preview the interactive card." --json
vivac artifacts capture --bundle card.selection.json --from t18 --description "Preview the card with keyboard controls." --artifact <artifact-id> --previous <revision-id> --json
vivac artifacts show <artifact-id> --revision <revision-id> --limit 20 --offset 0 --json
vivac artifacts check <artifact-id> --revision <revision-id> --json
vivac artifacts restore <artifact-id> --revision <revision-id> --to recovered-card --json
```

`show` pages member metadata and revision history separately, using
`members_next_offset` and `next_offset`. Listing metadata does not check every
member. `check` verifies the exact manifest and all members, including members
outside a displayed page. Missing, changed or unexpected members invalidate the
complete revision. The source comparison is a separate observation.

Restore requires a new directory. It verifies the bytes it copies, prepares the
complete delivery and publishes it exclusively; it never merges into an existing
directory or overwrites a file. It rechecks the visible destination parent's
identity before publication and refuses a mismatch. The restored
`.vivac-delivery.json` is verified
revision metadata, not an application member. Restoration does not create a new
artifact revision or execute the delivery.

A bundle accepts at most 64 explicitly selected UTF-8 `.html`, `.css` and `.js`
members. The entry point must be an HTML member. Paths use portable ASCII
components; ambiguous names, case-insensitive collisions, traversal, Windows
reserved names and links are refused. The complete revision, including its
manifest, is limited to 10 MiB; the manifest is limited to 64 KiB. The operation's
auxiliary memory limit remains 2 MiB. Every member and its metadata pass the
complete sensitive-value guards.

The preview serves a verified private copy of this exact revision from an
independent loopback origin. **Run HTML** enables the entry point and declared
local resources; resource URLs do not carry the preview control key or vivac's
session token. **Close preview** retains its existing meaning. Custody, integrity,
human approval and observed execution remain distinct. A single-file download
does not represent a complete bundle; use directory restoration to recover it.

Local custody is not isolation from another application with the owner's
authority that directly changes vivac's private preparation, store or process.
Vivac coordinates its own operations and rejects observed damage and conflicts;
it does not freeze editable source files or claim a coherent filesystem snapshot.
Unsupported filesystem operations are refused rather than replaced by an
overwriting fallback.

## Frontend applications with SQLite

Check `vivac artifacts capabilities --json` first. The `sqlite_application`
delivery type must report `supported: true`; `sqlite_snapshot` does not enable
standalone database capture. The application supplies its own JavaScript
runtime. Vivac does not download a database engine or provide an application
backend, WebAssembly or dynamic code evaluation in the preview.

Select the complete frontend and one database explicitly:

```json
{
  "root": "previews/counter",
  "entrypoint": "index.html",
  "members": ["index.html", "style.css", "app.js", "sql-asm.js", "data.sqlite"],
  "database": "data.sqlite",
  "requirements": ["JavaScript enabled; changes stay in browser memory."]
}
```

Use `capture --bundle` and the same check, preview and directory restore
operations as static bundles. The `database` path must name exactly one selected
`.sqlite`, `.sqlite3` or `.db` member; every other member must be HTML, CSS or
JavaScript. A filename alone never selects database capture. Include required
runtime notices in a selected HTML or JavaScript member.

The initial `sqlite-standalone-v1` profile accepts an autonomous, UTF-8 SQLite
database in rollback mode. WAL mode, any WAL/SHM/journal sidecar, multiple
databases and hard-linked sources are refused. Vivac holds a native SQLite read
lock on the authorized source handle throughout inspection and hashing. It
does not checkpoint, compact or write the source.

The retained database is a fresh **logical export**, not a byte-for-byte source
copy. The manifest identifies `logical-export-v1` and records the source size
and digest separately from the retained member's size and digest. This removes
freed-page remnants; it does not excuse sensitive live data or metadata. Source
SQL is parsed into a supported schema and never executed as artifact code.
All logical names, definitions, values and relevant internal metadata pass the
existing sensitive-value guards, using actual cell storage classes.

Tables need a verified `INTEGER PRIMARY KEY` alias or an explicit primary key
with `WITHOUT ROWID`. Supported keys, ordinary indexes, foreign keys, literal
defaults, AUTOINCREMENT counters, `user_version` and `application_id` are
preserved. BLOB, non-finite REAL, invalid UTF-8, NUL text, views, triggers,
virtual tables, generated columns, expression indexes and unsupported schema
clauses refuse the complete capture. No table or row is silently omitted.

The complete revision still has the 10 MiB, 64-member and 64 KiB manifest
limits. The SQLite profile additionally bounds schema and data: 16 user tables,
32 columns per table, 256 columns total, 32 indexes including automatic indexes,
64 KiB of schema SQL, 128-byte names, 64 KiB values, 256 KiB serialized rows
and 100,000 rows total. Inspection and export are bounded to 20 million SQLite
VM steps and two seconds of work per operation.
SQLite and Rust share the 2 MiB auxiliary-memory budget. Exceeding a limit
refuses capture rather than truncating inspection.

The new receipt is `artifact.sqlite_application_captured`; its manifest uses
schema version 2. Older engines must refuse that receipt rather than treat the
application as a static bundle. Use compatible engines in every harness sharing
the project. An artifact cannot change between file, static bundle and SQLite
application identities.

Preview serves only verified revision members, including the database as
`application/vnd.sqlite3`. The frontend may fetch resources from its random
revision resource prefix; it cannot use that permission to access vivac's
authenticated web origin. Changes made by the JavaScript runtime stay in browser
memory and do not update the retained revision or source. Restore copies the
exact normalized database bytes with the rest of the selected revision.

## Limits and refusals

File deliveries accept UTF-8 text up to 10 MiB. Invalid UTF-8,
oversized input and content recognized by the sensitive-data guard are
refused. Binary formats require an explicit supported delivery profile; the
single-file operation does not accept them. The guard examines the complete
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
the same custody guards and local scope as the CLI. Capture requires exactly one
of `path` or `bundle`, and `from`; `bundle` names a project-relative selection
file. Restore requires `to`. Replies contain `exit_code` and
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
Its `description` object reports `required_for_capture`, `max_characters` and
the `revision` scope. Older engines without this capability cannot retain the
required description. Do not omit it to get an older capture accepted; report
that limitation and keep the described delivery outside custody until the engine
supports it.

The engine retains a UTF-8 file or a complete selected frontend bundle per
revision, up to 10 MiB including bundle metadata. A self-contained HTML file can
include JavaScript and remain a single delivery. A contained SQLite database
requires the explicit `sqlite_application` capability and profile described
above. Other binary formats are unsupported. Recognizing a filename does not
make it eligible for custody. Retaining a supported
code file does not prove that an entire application can be recovered or run.
The engine offers restricted browser preview for retained HTML revisions.
It does not provide general application execution or install runtime dependencies.

## Deliver new work and incorporate existing artifacts

`vivac setup claude-code` and `vivac setup codex` install the
`vivac-artifacts` workflow in their native project skill directories. The session
orientation asks the agent to use it by default for visual or interactive
previews, unless the person asks otherwise. The agent creates self-contained
HTML or a complete HTML/CSS/JS frontend, adds a brief description, captures the
delivery with its actual producer node and returns the exact retained revision.
Later changes to a known artifact become new revisions of the same identity.
Other file deliveries require the person's explicit selection. Documentation,
reports, code and configuration stay in their ordinary project locations by
default. A small frontend application with a contained SQLite database still
requires explicit installed support for its application profile and a supplied
JavaScript runtime; this workflow does not install either one.

This is agent guidance, not automatic filesystem capture. Hooks do not inspect
or retain delivery bytes. Unsupported formats and capture refusals must be
reported; custody does not imply human approval. Setup installs instructions;
it does not search for or capture existing deliveries.

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
requires a project-relative source, producer node and brief description. The
inventory, page search and selected revision detail include the description.
A new revision continues
the latest revision reported by the engine, preserving the selected older one.
Restore creates a new project-relative destination and refuses occupied paths.

For file deliveries, download recovers the exact selected revision as an
attachment. Complete bundles use directory restoration instead. The engine
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
