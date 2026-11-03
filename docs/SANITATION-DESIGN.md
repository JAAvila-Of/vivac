# Local sanitation: implementation design and fixtures

This is the engineering design for the future operation required by
[Sensitive-data incidents](INCIDENT-RESPONSE.md). There is no sanitation
command or apply implementation yet. The fixtures below are independently
written examples consumed by the current engine, not a simulated sanitizer.
They do not prove interruption safety or completion of the incident contract.

## Selection without values

The first supported selection addresses a canonical node ID and a text role.
Aliases may be resolved against the captured source, but the reviewed plan
must bind the canonical identity. There are no search strings, regular
expressions or caller-supplied replacement text. The replacement is fixed:
`[Removed after a sensitive-data incident]`.

| Role | Complete selected scope |
|---|---|
| `title` | The node's `node.created.title` and every frozen title for that node in `vivac.created.stack`. Resolve each alias at its historical position through canonical identities and node numbers, not just the current alias. |
| `why` | The node's creation reason. |
| `notes` | Every `node.noted.note` for that node, preserving the individual events. |
| `outcome` | Every `state.changed.outcome` for that node, including values no longer present in the current fold. |

Planning must enumerate the selected event sequences and structural locations
without displaying their values. Selecting a role means its entire scope, not
only the last event. An unresolved frozen-stack reference is a refusal, not an
invitation to match text or skip the snapshot.

Promotion changes a node's kind and alias without changing its canonical ID or
parent edge. The same node can appear as `t2` and later `g2`; both frozen titles
belong to the selection. Validate the alias against the history at that event.

A saved `next_intent`, label, flag reason or other text field is not implicitly
the title merely because its bytes happen to match. Event-specific selection
for those roles needs its own explicit schema before support is added. If an
unselected value fails the ordinary privacy guards, the entire operation
refuses. Unknown secrets outside the reviewed selection remain outside any
absence claim. Structural identities, paths, commands, declarations and custody
records cannot be rewritten by treating them as arbitrary prose.

Unknown keys need strict validation before typed event decoding. The ordinary
`Event` and `Body` decoder accepts additional keys; successfully decoding an
event is therefore insufficient evidence that a transformer preserved or
inspected all of it. The maintenance reader must reject unknown top-level and
nested keys and unsupported event variants, including benign extensions. It
must distinguish a selected recognized field from an unrecognized field that
the ordinary decoder would ignore.

## Capture and the reviewed plan

Planning opens the explicitly selected source through directory capabilities.
It must not load it through an operation that registers, repairs or writes the
tree as a side effect. Capture the writer lock, the complete log boundary,
configuration and lane markers, and handles for the complete required custody
set. A torn event, conflicting lane root or missing member refuses the plan.

The private plan envelope binds:

- An opaque operation ID and format version.
- The first-event identity used for discovery and `config.project_id` used for
  the project-sharing boundary. These are different identities.
- The captured source file set, byte lengths and whole-file commitments.
  Commitments stay in the private envelope; they are not fingerprints of
  individual removed values and must not become log prose or public output.
- The canonical selection and its expanded structural locations.
- The absent destination and explicitly supplied lane roots.
- The fixed successor identities, generated once for this operation.
- Required custody members, recorded policies and outstanding external-copy
  actions, expressed without delivered contents or removed values.

Producing a requested private plan file does not authorize mutation of the
source, creation of a successor, registry changes or context delivery. A safe
summary contains structural scope and counts. It must not render node titles,
notes, prompts, excerpts or input parser errors containing source text.

Apply must consume the reviewed envelope with explicit confirmation of its
canonical plan digest, not just its operation ID, and the source-retirement
declaration. That digest binds the complete plan, including private source
commitments; it is not a digest of an individual removed value. Apply must
first reconcile any previously published result for that operation. If no
verified result exists, it must re-open the same source safely and check the
captured identities and bytes before transforming anything. A modified source,
changed selection or destination invalidates a new transformation. It must not
refresh a stale plan and then apply the new scope under an earlier confirmation.

## Successor representation

For the supported prose transformation, preserve canonical node IDs, node
numbers, event sequences, parent edges, historical states and event ordering.
Keep historical opaque actors, lane identities and repository anchors when
their fields remain valid. Change only selected text and the explicitly
required authority metadata; do not renumber the history to hide removals.

Generate a new first-event ID and a new project ID. Reuse neither the source's
discovery identity nor its project-sharing boundary. Rebind lane markers and
other recognized authority references consistently. Preserve remaining event
IDs; the first event's changed ID must be disclosed as an identity change.

After transforming and validating the historical prefix, append a
`tree.sanitized` event. Its payload records `operation_id`, the source and
successor tree/project identities, the approved plan digest and the selected
canonical nodes, roles and expanded event locations. It contains neither
source paths, removed values nor individual value fingerprints. It describes
a transformed successor, not a byte-preserving recovery.

Add a dedicated `ConfigVersion` upgrade sentence for this capability and
validate it through the owner and linked-lane loading paths. The event and
version gate must ship together; do not rely on a marker an old reader might
ignore. Fixtures using old engines must demonstrate refusal before apply is
exposed. Historical actors remain unchanged, but configuration gets a fresh
opaque actor for subsequent writes and `share: false` initially. Opening
cross-project reads requires the owner's separate review and existing sharing
operation; it is not inherited implicitly during incident recovery.

Rebuild the index from the output events. Do not carry source indexes, opening
payloads or receipts that assert readiness for the old authority. Do not claim
to recall material already delivered to a harness. New context preparation
must target the successor; native harness operations remain available.

Custody files are not transformed by these text selectors. Carry only the
complete supported set with unchanged bytes, identities, digests, notices and
recorded policy, using the source handles and ordinary integrity checks. Native
agent files still need their own privacy checks. An unsupported member, unsafe
metadata field or reference requiring a payload transformation refuses the
whole operation. An authorized sensitive artifact does not become a refusal
merely because its recorded policy permits that content.

Use no larger input and member ceilings than local backup: 512 KiB for plan
metadata, 64 KiB for configuration, 8 KiB per lane marker, 4,096 members,
256 MiB per input file and 64 GiB total. Additionally cap each encoded event
line at 1 MiB before JSON decoding. Copy buffers remain bounded
at 64 KiB; that is not a claim that the fold uses only 64 KiB. Measure parsing,
folding and validation memory and latency at the supported boundaries before
exposing the command. No member or event may be truncated to fit a limit.

## Publication and retry

The source remains restricted and retired. The operation never overwrites it,
an older backup or an existing destination. Prepare a complete successor in an
owner-private sibling directory outside Git working trees. Verify the entire
result and synchronize its files and directories before publishing it.

Before reserving the registry entry, durably record the approved plan digest,
fixed successor identities and complete verified output manifest in a private
operation receipt, separate from the immutable reviewed plan. This receipt is
maintenance state, not an opening receipt or public log content. Failure to
persist it prevents publication. It must survive acknowledgement loss so that
a fresh process can verify exactly the result already produced.

The existing recovery registry reserves an identity and destination under its
lock before the publication callback. Keep that ordering for the successor.
An identity created afresh on every retry would bypass this protection: the
reviewed operation must retain the same successor identities across retries.

| Boundary interrupted | Required observable result and retry |
|---|---|
| Before or during private preparation | No active successor. Original bytes remain intact. A stale private preparation is not a published tree. |
| After complete preparation, before registry reservation | Same operation may resume only after checking the captured source, plan and preparation. Nothing is active yet. |
| After durable reservation, before exclusive publication | Registry may name an absent destination. Reconcile that same operation and identity; do not create another identity or overwrite a destination. |
| After publication, before acknowledgement | A complete successor exists at the reserved location. Verify its operation identity and bytes against the durable private operation receipt and report that result; do not run the transformation again or require the retired source to remain available. |
| After acknowledgement | Repeating the same operation identifies the verified result without another tree, registry entry or transformation record. |

An occupied destination with unrelated or unverifiable contents is always a
refusal. A registry reservation without a matching operation envelope is not
permission to adopt those contents. Reconciliation must keep the registry lock
while deciding whether a published authority already exists.

Retry checks the confirmed plan, receipt, registry location and existing result
before applying preconditions for a new transformation. A matching published
result does not require rereading, restoring or redistributing the old source.
A missing receipt, changed result or conflicting registered location refuses
reconciliation; it does not authorize another destination or fresh identities.

Existing recovery tests exercise errors returned before and after publication;
they do not establish the abrupt process-death guarantees in this table. Before
apply can ship, add controlled interruption tests at each durable boundary,
including a process killed after publication and before acknowledgement, and
then retry from a fresh process. Do not use timing races as the interruption
mechanism. Filesystem guarantees and refusal on unsupported platforms must be
checked separately from an ordinary returned error.

The writer lock covers known cooperating writers; it does not fence a process
on another machine or an old process holding an open handle. Operational
retirement remains a precondition. A new marker alone cannot establish it.

## Executable fixture scope

`tests/data/incident/` holds fixed synthetic source and expected-successor
histories. Their selected node is promoted and then abandoned, has multiple
notes and outcomes, and appears as both `t2` and `g2` in frozen stacks. The
examples contain harmless prose,
not a real credential, personal data or a discovered guard bypass.

`tests/incident_fixtures.rs` checks that the current CLI can read both examples,
that unaffected relationships and history survive, that the identity changes
and selected locations are exact, and that earlier outcomes survive in the
source even when the current fold displays only the latest one. The expected
history is written independently; tests do not generate it with a substitute
sanitizer. The fixture prefix deliberately excludes the future transformation
record until that record's schema and compatibility guard exist.

These tests establish representable historical-prefix oracles. They do not
test selection parsing, sanitation, copying, crash recovery, old-engine refusal,
initial sharing policy, custody transfer, MCP/web delivery or absence in
external copies. Those remain implementation requirements from the incident
contract, not covered claims.
