# Sensitive-data incidents

Use this guide when prohibited information has reached the provenance tree,
or when retained files have been exposed beyond their intended audience.
The tree and the separate custody stores have different policies. Explicitly
authorized sensitive artifact custody is not, by itself, a violation of the
tree's prohibition on secrets, personal data and file contents.

Vivac currently has a preventive write guard and verified local backups. It
does not provide a sanitation command. The maintenance contract below describes
requirements for a future implementation, not operations available today.

The [implementation design and fixtures](SANITATION-DESIGN.md) describe selection,
identity, publication boundaries and the limited executable examples used to
prepare that implementation.

## Contain an incident with the current release

1. Revoke or rotate an exposed credential at its issuer. Removing a local value
   cannot invalidate a credential, an already downloaded copy or another
   person's access. For personal data and other confidential information,
   restrict access and involve the owner responsible for it.
2. Stop further use and distribution of the affected Vivac store. Stop its
   writers and its MCP/web endpoints before isolating it. Suspend Vivac's
   project context delivery until a safe authority is available. This does not
   require disabling native harness messages, delegation or other native work.
   An existing session may already hold the information in memory or history.
3. Keep the original under restricted access outside active discovery
   locations. Account for every lane and integration that can still reach it;
   moving one directory does not retire every copy or stop an open handle.
   Do not edit `.vivac/events` by hand or manufacture a replacement index.
4. Inventory the affected copies using safe references, not the sensitive
   values: stores, indexes, backups, recovered workspaces, exports, retained
   files, native agent configurations, generated opening context, session
   histories and any copies shared outside the machine. Include historical
   versions and partial preparations. Do not make another unrestricted copy
   while investigating or paste the value into a command, report or chat.
5. Treat older backups as unverified for this incident. They may contain the
   same historical value. Restrict affected backups and prevent their reuse
   until they have been reviewed; ordinary recovery does not sanitize them.
   Decide separately how each controlled copy will be retained or disposed of.
   Vivac does not enforce retirement or deletion on another machine.
6. Report a guard bypass through the private channel in [Security](../SECURITY.md).
   Describe the affected version and fields. Use a synthetic reproduction;
   do not send a real credential or confidential payload with the report.

Do not resume the affected authority just because the current node text looks
clean. Neither `done` nor `abandon` removes history. Deleting a derived index
does not remove its source. A new safe tree can support new work, but it is not
a repaired copy of the old history, and it does not clean existing sessions.

## What backup verification establishes

[Backup creation, verification and recovery](BACKUP.md) inspect decoded strings
and keys in provenance metadata, including unknown fields. A recognized privacy
violation refuses the operation without echoing the rejected content. These
operations do not repair or rewrite it. Passing the guard is not proof that an
unrecognized secret is absent.

Artifact payloads retain their recorded inspection policy. A verified backup
can therefore contain intentionally retained sensitive artifact files. Changing
the project policy governs later captures; it does not erase earlier revisions.

## Required contract for future local sanitation

The first implementation must be a separate, explicitly requested maintenance
operation. It must not change the meaning of backup, normal writes, node
closure or artifact policy. There is no proposed command spelling in this
contract. The CLI must support the operation before a human interface does.

### Selection and authority

- Select affected records and fields by structural references. Do not require
  the removed value in arguments, environment variables or a selection file.
  Planning and diagnostics must not display old values, excerpts, native
  prompts or fingerprints of individual removed values.
- A reviewed plan must state the selected scope, the new destination, affected
  custody references and known copies requiring separate action. Planning must
  not mutate or activate a tree. Applying must require an explicit selection
  and confirmation bound to that plan; changed source bytes invalidate it.
- The original remains a restricted, retired source. The successor has its own
  tree identity and project-sharing boundary, with an explicit transformation
  record. It must not register as an unchanged recovery of the original or
  silently take over its location.
  Local checks and owner declarations cannot establish remote retirement.
- Preserve the unaffected provenance relationships and disclose every selected
  removal with safe structural references and a fixed removal marker. Do not
  reproduce the removed values in an audit record or inherit readiness claims
  from opening receipts prepared for the old authority.

### Complete transformation

- Cover every historical occurrence within the selected structural scope,
  including superseded text and notes. Editing only the current folded view is
  insufficient. Unselected data must still pass the ordinary privacy guards;
  this is not a new bypass or an automatic claim that every secret was found.
- Inspect configuration, lane metadata and all output members. Rebuild derived
  indexes from the sanitized events. Do not copy old indexes or generated
  opening payloads, or treat a previously delivered context as fresh.
- Resolve custody references consistently with their historical identities,
  digests and policies. No dangling reference, silent omitted revision,
  changed payload under an old digest or implicit sensitive-custody exemption
  is acceptable. If a required transformation is unsupported, refuse the whole
  operation and identify only its safe structural scope.
- Older backups, exports, native files, chat histories and copies outside the
  selected operation remain outstanding. Report that boundary explicitly.
  Do not scan other projects, upload data, delete backups or execute retained
  programs as a side effect.

### Publication and interruption

- Require an absent destination outside Git working trees. Prepare privately,
  apply explicit byte and member limits, and validate the complete result
  before publication or registration. No member may be skipped to fit a limit.
- Stop concurrent source writers before applying. Recheck source identity and
  bytes against the plan. Refuse changed input rather than mixing snapshots.
  An older engine or an already running process is not fenced by a new marker
  it does not understand; the operational retirement must cover those writers.
- Before publication, failure leaves the source intact and no active successor.
  A leftover private preparation is not a usable tree or proof of sanitation.
  After publication, interrupted registration or acknowledgement must be
  distinguishable from an unpublished result without creating another authority.
- Resume Vivac context delivery only against the verified successor. Existing
  harness history, model memory and remote copies cannot be recalled. Report
  unverified context without blocking native harness capabilities.

## Acceptance cases before implementation can ship

Use isolated synthetic fixtures. Test reports must not contain sensitive input.

| Case | Required result |
|---|---|
| Rejected write, no persisted value | Remains a preventive refusal; no incident mutation or secret echo. |
| Value exists only in old text or a note | Selected history is transformed, not just the current view. |
| Unknown field or unsupported transformation | Whole operation refused; no silent omission. |
| Sensitive artifact retained under explicit policy | Policy is preserved; no automatic deletion or reclassification. |
| Referenced revision or managed agent cannot be safely carried | Whole operation refused until its treatment is explicitly supported. |
| Existing destination, Git destination or unsafe member path | Refused before publishing payload bytes. |
| Source changes after planning or a known writer holds the source lock | Refused without mixing or replacing the source; retirement of other writers remains an operational precondition. |
| Interruption during preparation | Original intact; no active partial successor. |
| Interruption at publication or registration | One complete identifiable successor or none; retry cannot create a second authority. |
| Old index, receipt or generated context is present | Rebuilt or excluded; never accepted as evidence of current safe delivery. |
| Older backup or copy on another machine exists | Explicit outstanding action; no global-erasure claim. |
| Plan, refusal, audit and command output are inspected | No removed value, excerpt or value fingerprint appears. |
| Completed successor is read through CLI, MCP, web and search | Relationships survive; selected historical fields contain the removal marker instead of their old text. No absence claim extends beyond the reviewed structural scope. |
| Ordinary writes and native harness tools are exercised | Existing guards and native capabilities remain intact. |

These cases define logical content and authority boundaries. They do not prove
physical erasure from storage media, the absence of undiscovered copies, or
removal from another person's system. Successful maintenance must state those
limits rather than claiming the incident is globally erased.
