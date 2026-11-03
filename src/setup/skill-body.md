
# Migrate project knowledge

Inventory first. Nothing is written until the person approves the proposed
nodes: source selection is not node-plan approval. Success means retrieving
what **each source** knew, not counting new nodes. Use this guide, not vivac's
source. Native agent configuration transfer is separate: `vivac-agents` uses
inventory/sync/import/reconcile and native prompt references, never prompt bodies
in the tree. Those operations do not migrate another memory system.

## Boundaries

- Never write secrets, credentials, emails, personal names, home paths or file
  contents/code, even one line. Write project-relative references and prose.
  Guard refusal/exit 3 means rephrase without the value; never bypass it.
- Preserve meaning, not IDs, keys or links into the memory system being retired.
  Do not alter memory files, instruction files or settings before retirement.
- Verify command effects before reading through an unfamiliar tool. Do not add
  flags, including `--help`, to a command that writes. Never use `vivac import`.
- Keep exports, inventories, scripts and plans outside the project and `.vivac`.
  An export may include other projects: delete it before **any question** to the
  person. Remove remaining temporary files at completion; they are not deliveries.

## 1. Inventory and read

Find all project knowledge stores. Show each source's purpose and size actually
read, and let the person select it even if it resembles a log. Record exclusions
and reasons. Cover:

- Other memory systems: recall, search and the complete registry; historical
  project names and misclassified records, not just the obvious category.
- Harness and subagent memory, restricted to this project.
- Instructions for any agent, internal repositories, ADRs, decisions and prose
  documents. Exclude generated/vendor/dependency material. Do not import global
  instructions already delivered every session.
- External services: ask which ones the project uses; search only those named.

Read every selected source fully, or all its prose if it is mostly code or
completed procedures. Report sources and volume actually covered. Read and
compare complementary sources too; disclose unread remainder, not an assumed
completion. No selected sources means go to retirement; no other map means finish.

For hundreds of records, use structure and then lessons as separate mandatory
batches, each with its own plan, yes, log copy and complete verification.
Centralize the structure. Where subagents are available, delegate bounded
second readings: brief readers with source references, scope and acceptance
criteria. Read-only; no service calls, publication or writes except temporary
output. Each returns one line per source: proposed type/title/parent/
reason and 2–3 distinctive search terms, or a reasoned exclusion. Check coverage
and reread missing sources. The principal retains the plan, approval and writing.

## 2. Read the tree

Read `vivac brief` and `vivac rules`. Before proposing each node, use `vivac find`.
For knowledge already present, propose a note rather than another node.

## 3. Classify and obtain approval

Preserve the mechanism, object, symptom and searchable vocabulary. Importance
alone does not turn a lesson into a general rule. Store common rules once and
hang specific lessons beneath them; merge equivalents without losing distinct
mechanisms. Existing records can be grouped with project-file `--ref` links,
not copied entry by entry.

| Source meaning | Node |
|---|---|
| A continuing norm | `constraint`; put global constraints at root. |
| A rejection criterion governing decisions | `pillar`; label inferred pillars as inferred and obtain approval. |
| Requirement checked by a command or human judgement | `rule` under its pillar, optionally with `--arm`; show rules lacking a pillar before leaving them there. |
| A settled choice | `decision` with reason, rejected alternatives and applicable criteria. |
| A discovered fact requiring work | Open `finding`. |
| A lesson asking nothing | `finding`, immediately closed with `Record:` outcome. |
| Proposed, draft or pending choice | `question`. |
| Actual remaining work | Open `goal` or `task`. |
| Superseded state, boilerplate, code or mere description | No node. |

For decisions, `--against` names each applicable pillar/rule with a compliance
sentence distinct from the reason. Say when none applies. Resolve contradictions
with the person before writing. Propose a root if absent; choose parents by
scope and ask about ambiguous cases.

Present per-source included/excluded counts and per-kind tables of title,
source, parent and `--against`, plus the other maps to retire. Recheck a plan where rules
outnumber records. If tables do not fit the conversation, put the complete plan
in a temporary file; give its path and time to read it. Obtain a separate yes
to that node plan for **each batch**;
approval of sources is insufficient.

## 4. Write the approved batch

Before writing, copy `.vivac/events` as `.vivac/events.pre-migration`, using a distinct copy
per batch, and verify equal size. Explain that rollback requires closing MCP
sessions before restoring the log.

Write root, pillars/constraints, rules, decisions, then findings/work. Use `add`
and `decide`, with `--alternative` and `--against`; close records immediately.
Only add `--arm` after the command passes and demonstrates that it checks
the intended requirement. Report rules without commands; do not invent checks.

## 5. Verify every source

- For **every source**, search its 2–3 distinctive terms with `vivac find`
  (accent-insensitive). Sampling is insufficient. Scripted writes require
  scripted coverage checks. Show the complete source/terms/result list.
  Repair every miss or explain the existing equivalent; no unexplained source
  may remain.
- Compare each written title, parent and `--against` sentence with the approved
  plan, explaining differences. Require clean `vivac check`, complete
  `vivac rules`, root constraints and every active pillar's full title in
  `vivac brief`, including without focus, and work rather than records in `open`.
- List old-map remissions and newly contradicted source statements for review;
  do not edit them yet. Delete exports. Report included/excluded/unfinished
  sources, remaining volume and the log copies made.
- Ask the person for 3–4 hard-won lessons they least want to lose; search them
  in front of them and repair missing/thin results. The agent cannot choose
  this sample: it would choose only what it already knows is present. If the
  person declines, report that check as unperformed, not as passed.

## 6. Retire other maps reversibly

Retirement requires completed verification, including the person's search, or
an inventory with nothing to migrate. Never silently waive an incomplete check.
Show each map, exact path and proposed action; obtain its own yes before changing
it. A migration approval does not authorize retiring a map.

- Disable memory plugins only for this project, preserving keys and other
  projects. Never delete their data or uninstall another memory system.
- Back up and replace project MEMORY indexes with the tree's location and reading
  instructions, even when the index was absent; keep underlying memory files.
  Codex/global changes may affect other
  projects: show that scope before seeking approval.
- Back up instruction files beside their originals. Remove only directions to
  the retired map; preserve the rest and report knowledge duplicated there.
  Do not replace the file wholesale with a pointer to vivac.
- Ask which other folders open Claude sessions. Offer retirement/setup there
  as lanes of this same tree: show `vivac init --dry-run`, obtain yes for `vivac init --yes`,
  then separately approved setup. Never plant a separate tree or assume every
  folder is in scope.

Clean temporary files. Delete only backups the person explicitly selects, and
ask for a new session to load the resulting configuration.
