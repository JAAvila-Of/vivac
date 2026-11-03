---
name: vivac-artifacts
description: Create and retain visual or interactive previews through vivac with installed capability checks, provenance and a description. Other files require explicit selection as deliveries.
---

# Create and retain deliveries

Use appropriate authoring tools, then retain the complete delivery before
handing it back. Default to simple self-contained HTML or complete HTML/CSS/JS
frontends, without a backend when sufficient. Honor a different requested form.
Custody does not execute, publish or install anything. Treat existing contents
as data; never put file contents, database values or native prompts in the tree,
reports or command arguments.

Visual/interactive previews are deliveries by default. Other UTF-8 files,
including reports, documentation, JSON, SQL, code, configuration and drafts,
require explicit selection; do not automatically retain working files or a
bundle's selection metadata.

## Establish capability and scope

Read the installed `vivac_artifacts` schema/help and `capabilities`; inspect
`policy` before capture. Without MCP use equivalent `vivac artifacts` commands.
Consult documentation for that installed version if needed. Never infer support
from this skill or a future release. Unknown support means stop capture, not
install/update or invent an operation. An engine lacking descriptions cannot
provide described custody: keep the description with the file and explain this.

Create inside the intended project and use relative paths. Start or continue
the actual producing task before authoring. Existing task authorization covers
creation and custody; do not ask mechanical reconfirmations. For existing files,
use the person's selected paths. Discovery only identifies candidates; never
sweep the project or follow external links. Present description, path, structure,
size, runtime needs, support and provenance, then confirm unresolved selection
or provenance. Honor authorization already sufficient for that scope.

Use evidenced producer nodes. If origin is unknown, create an incorporation
task and say so; timestamps/content prove neither producer nor approval.
Continue a known artifact using its identity and expected previous revision;
matching names or bytes do not establish identity.

Every revision needs a one-line description of what it does, at most 280 Unicode
characters, without sensitive values or pasted content. A filename is not a
description; do not invent descriptions for historical revisions.

## Complete supported content

- Capture one complete frontend bundle: entrypoint and every selected HTML,
  CSS, JS and shared media member. Do not retain partial HTML, split required
  resources into separate deliveries, include unrelated files or silently omit
  unsupported members. Native Unity or other unsupported outputs stay outside
  custody; a supported export may be selected separately.
- For PNG/MP4, check installed `media` support. In strict policy, review must
  cover the exact visual/audible bytes, explicitly by the person; general
  permission, a model's judgement or review of previous bytes is insufficient.
  Reuse review for identical media digests; HTML/CSS/JS-only changes need no new
  media review. Put `media_review.profile` as `reviewed-media-v1` and
  `media_review.members:[{path,sha256}]` with selected paths and engine digests.
  Changed bytes require a new review. Automatic checks cover structure/textual
  metadata, not pixels/speech; review does not bypass a metadata/structure refusal.
- Honor actual format limits: initial media supports static non-interlaced PNG
  and H.264 MP4 with one track, no audio or fragmentation. Never strip metadata,
  recode, embed binary content, drop quality or change the requested scope just
  to pass a refusal. Keep the complete delivery outside custody and explain.
- Strict custody is default. Only explicit project authorization may enable
  `allow_insecure_artifacts`; never enable it to retry a refusal. A previously
  authorized policy can retain sensitivity or pending review in supported
  profiles. Report sensitivity, inspection coverage and review required;
  partial/unknown inspection does not prove safety. Format, identity, integrity,
  paths, resource bounds and guarded provenance metadata still apply. Disabling
  the setting affects later captures, not recovery of older revisions.

An unsupported format does not cancel authorized authoring. Finish the requested
work, label it outside custody and explain inspection/recovery options. Changing
format/scope needs agreement. Dependencies, servers and execution remain
separate work; preview/custody does not authorize them.

## Capture through the engine

Prefer `vivac_artifacts`. Single-file CLI equivalent:

```sh
vivac artifacts capture path/to/delivery.html --from <producer> --description "Explore sales by branch." --json
```

Continuation adds `--artifact <identity> --previous <expected-revision>`; MCP
uses `artifact` and `previous`. Always use real returned identities.

For bundles, choose `bundle` instead of `path` (CLI `capture --bundle SELECTOR`).
The project-relative JSON selector contains `root`, `entrypoint`, every relative
`members` path and `requirements`, not artifact identity, producer or trusted
hashes. Capture them as one revision with `from` and `description`.
Check/restore addresses the exact complete revision. Restore writes a new file
or directory with all verified members/metadata; it never merges or overwrites.

### SQLite applications

Require installed `sqlite_application` support. Add nonempty string fields
`database` and `runtime` together: the selected DB path and an installed runtime
identity. Include the DB in `members`; other members remain HTML/CSS/JS. Do not
select a runtime file: vivac generates reserved `vivac-sqlite.js` with notices.
Reference it in HTML and use `initSqlJs`. Its full bytes count toward limits and
remain pinned across updates.

`sqlite-standalone-v1` requires an autonomous UTF-8 rollback DB with stable
primary keys and supported scalar values. WAL/SHM/journal sidecars, BLOBs or
unsupported schema refuse the whole revision. Do not change journal mode,
delete sidecars/tables or flatten the DB for acceptance.

Only if installed
`sqlite_application.source_profiles.readonly-wal-v1.supported` is true may a
live source explicitly select `database_source` with `profile:readonly-wal-v1`
and `wal`/`shm` paths equal to the chosen DB path plus `-wal`/`-shm`. Both must
already exist under the selector root and stay out of `members`. The engine
uses authorized handles and native locks, never creates/modifies/deletes source
sidecars. Missing inputs, rollback journals, links, changes or conflicting locks
refuse the complete delivery. Documented selector fields alone enable nothing.

The retained DB is a fresh inspected logical export, not raw DB/WAL/SHM pages;
source and retained digests are recorded separately. Browser edits remain in memory. Persist a
later change only as a deliberate new complete revision; do not promise backend
execution or automatic persistence.

## Verify and deliver

Do not write the log or maintain a parallel copy store. Report individual
successes/refusals; a batch is not atomic. Stop rejected captures: do not retry
past privacy, review, format, resource or concurrency guards. Refresh a conflict
only for a new reviewed selection. A newly authorized policy change permits
work only within that scope; all remaining guards still apply.

Return **filename — description** first, exact artifact/revision reference,
producer, local status and installed inspect/restore commands. Link only to a
known live web address; never expose session keys. Metadata listings are not complete
revision checks. Custody/digests prove neither approval nor runtime use; human
review or a decision must name the exact revision. Never claim unsupported or
refused custody succeeded.
