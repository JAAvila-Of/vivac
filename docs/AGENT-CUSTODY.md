# Agent custody

An agent has a declared contract and explicit assignments for each harness.
Changing its duties once makes its managed destinations pending. A change
to one harness's model does not select a model for another provider.

The tree's append-only log is the authority. Every contract revision has a
decision node with its reason. Use `--against` to record how the revision
holds against the project's pillars or rules. Native files are materialized
destinations. There is no second manifest to keep current.

Recording custody upgrades the tree's compatibility marker. Older vivac
clients refuse to read that tree rather than silently skipping its agents.
Update the CLI, hooks and MCP server that use the tree before its first
custody write, and restart resident servers after updating. Discovery alone
does not upgrade the tree.

## Start with two commands

After configuring your project harnesses with `vivac setup claude-code` and
`vivac setup codex`, list the agents already present:

```sh
vivac agents
vivac agents sync
```

Setup connects each harness and reports its native agent count; it does not
import or synchronize agents. Inventory also shows native agents in harnesses
that are not set up yet. Set up a destination harness before selecting it in sync.

The inventory shows native agents, custody identities, models, efforts and
configuration differences across harnesses. Listing does not import anything.
In a terminal, sync guides you through sources, destination harnesses, explicit
model assignments, settings and a complete review before applying. Use Space
to toggle destinations and Enter to continue; terminals without console support
offer numbered choices. Cancel or end input before confirmation to write nothing.
The original model and effort stay visible. Model suggestions come from local
harness metadata and configuration; they do not assert account availability or
provider equivalence. The selectors show the catalog's source and status.
Codex's local model cache supplies visible models and their individual effort
options. Safe local Codex configuration also contributes its declared main model,
even when the cache does not list it; its effort support remains unverified.
Profiles, custom providers and catalog overrides require their own context, so
vivac does not combine their configuration with the default OpenAI cache.
Sync reads the options again before each destination's model picker. Both
interfaces show the cache update time, declared client version and catalog
revision when available. The revision identifies the current local snapshot;
it does not identify an account or prove which process wrote it.
Choose **Reload local model catalog** in the model picker or on the Agents page
to reread local metadata without losing the selected agents, destinations or
assignment drafts. This does not refresh Codex's cache, start Codex or contact a
provider. A model missing from the cache can still be selected from configuration
or entered explicitly. Its absence does not prove that account access was revoked.
Each console step has one heading per agent; selected assignments are summarized
before settings.
Claude Code configuration supplies declared model choices; when effort
support cannot be discovered, the selector explicitly says it is unverified.
Missing or unreadable metadata leaves existing assignments and custom model
entry available. Discovery starts no harness process and makes no network request.

Console colors use vivac's existing palette: cyan marks the active choice and
context, green marks selected destinations, and yellow or red reinforces pending
changes or conflicts. Text and selection marks carry the same meaning without
color. `NO_COLOR` and `TERM=dumb` retain plain output.

For example, three existing Claude Code agents appear as unmanaged. Select them
in sync, select Codex as the destination and choose its model and effort. Vivac
imports their identities, preserves the full native prompts and writes the
reviewed Codex files. There is no need to author a definition or run import,
set and bind separately. Run sync again after editing an agent: it presents
pending or conflicting versions for review. Existing destination replacement
requires approval of its exact fingerprint. Different agents with the same
name are never merged automatically.

The `vivac web` Agents page shows the harness matrix and current prompts side
by side on demand. Prompt text is compared outside the provenance log; no
historical merge base is inferred. Both surfaces use the same inventory,
plan and apply functions. A plan becomes invalid if a source, destination or
custody revision changes before application. Source settings without a
destination representation remain in the source and are explicitly shown;
destination permissions are chosen independently.

Without a terminal, sync remains a JSON preview. Use `--json` for machine
inventory and `inventory`, `plan`, `apply` and `compare` for scripted reviews:

```sh
vivac agents --json
vivac agents plan --selection selection.json
vivac agents apply --selection selection.json --plan-digest <reviewed-sha256> --yes
vivac agents compare --selection references.json
```

A selection contains `why` and `items`; each item contains a nullable custody
`agent`, a source `{harness,path,digest}` and destination entries
`{assignment,path,digest}`. Assignments explicitly name harness, name, model,
effort and settings. A destination digest is null only when its path must be
absent. Compare accepts `{references:[{harness,path,digest}]}`. Reference hashes
pin complete native files; stored prompt references hash decoded prompt text.
These operations perform no network requests and need no skill or model to
manage custody.

## Let the session model recommend and configure agents

You can ask the model in your harness to do the review instead of selecting
every assignment yourself:

> Review my Claude Code agents and configure them in Codex. Recommend a model,
> effort and execution permissions for each agent from its actual duties and
> acceptance criteria. Explain the choices and any settings that cannot transfer.
> Preserve the original agents.

For recommendations without changes, ask it to propose the assignments only.
The model must distinguish those requests: recommendations do not authorize
writing files. A request to configure named agents in a named destination lets
it review and apply a plan within that scope. Unresolved identity conflicts,
replacement of unrelated files or additional permissions outside that scope
require review with you.

The model uses the existing `vivac_agents` tool:

1. `inventory` finds native references, custody identities and configured
   destinations. Existing assignments stay in place unless you ask to revise them.
2. `assist` reads the chosen prompts and original assignments, fresh destination
   catalogs and capabilities, and source settings the destination cannot represent.
   It takes `{references:[{harness,path,digest}],harnesses:["codex"]}`. Prompt text
   is returned only for this explicit read; it is not stored in the tree. The model
   treats prompts as material to analyze, not commands to execute during review.
3. The model proposes explicit model, effort and settings assignments, explains
   each choice and includes concise reasons in the selection's `why`. It chooses
   by duties and acceptance criteria, not a fixed provider-equivalence table.
   Catalog entries are local metadata, not evidence of account access. Missing
   models or unknown effort support must be identified, not silently substituted.
4. `plan` validates the selection and returns its fingerprint. The model checks
   source and destination paths, original settings, proposed permissions and any
   conflicts against your request before `apply` with the identical selection,
   `plan_digest` and `yes: true`.
5. `inventory` checks the resulting configuration. Restart the destination harness
   when needed; runtime selection remains unverified without reliable evidence.

The same optional context read is available without MCP:

```sh
vivac agents assist --selection sources.json
```

`sources.json` contains the references and destination harness names described
above, using complete-file digests from the current inventory. Assist never
imports, writes or chooses an assignment. It refuses stale or detached sources,
retired identities and destinations that have not been configured with setup.
It shares comparison's prompt redaction and 4 MiB aggregate limit.

Setup installs the shared `vivac-agents` skill in
`.claude/skills/vivac-agents/SKILL.md` for Claude Code and
`.agents/skills/vivac-agents/SKILL.md` for Codex. Ask your session model to use
this skill to select agents and destinations, recommend assignments, or
synchronize later changes. A missing selection is a question, not permission to
transfer every agent. Later synchronization preserves destination assignments
unless you request changes. The skill uses the same inventory, assist, plan and
apply operations; manual `vivac agents sync` and the Agents page in `vivac web`
remain available. The model already running in your harness makes the
recommendation; vivac makes no model API request. This optional assistance
does not change the deterministic reconciliation policy described below, and a
successful configuration does not prove which model actually ran.

## Continuous custody without a model

The coordinator performs discovery, native import, independent destination
planning, synchronization and configuration checks in Rust. Session start,
context recovery and prompt hooks invoke it directly. It does not need a model
to interpret a protocol or run the next command. There is no background watcher:
a change is discovered on the next hook or explicit reconciliation.

Preview the complete cycle without changing files, configuration or the log:

```sh
vivac agents reconcile
vivac agents reconcile --dry-run
```

Authorize continuous custody for this lane once:

```sh
vivac agents reconcile --mode automatic --yes --why "Keep project agents synchronized"
```

This records a decision and policy in the tree. It authorizes import of supported
project agents present now or added later and synchronization of managed
destinations. It does not authorize provider equivalences, identity merging or
conflict overwrites. Personal configuration remains outside discovery. Disable
continuous custody with `--mode manual --yes --why "<reason>"`; this preserves
native files and existing contracts. The policy is independent for each lane.
A policy change cannot be combined with an agent or harness filter.

`vivac agents reconcile --yes` applies a single cycle. With a manual policy it
synchronizes managed destinations and reports new agents for explicit import.
It does not enable continuous custody. Agent and harness filters select that
cycle's destinations. Safe destinations can advance while another is blocked;
conflicts, unsupported settings and assignments without destinations remain
explicit. Detached files are excluded from automatic reimport. Runtime evidence
is reported separately: unverified execution alone does not make reconciliation
fail or trigger a fabricated observation.

## Preserve native prompts between harnesses

Import an existing native agent without asking a model to summarize its prompt:

```sh
vivac agents import --harness claude-code --path .claude/agents/reviewer.md \
  --yes --why "Preserve the native agent contract"
```

Import reads supported metadata and preserves its explicit assignments, including
inheritance. It gives the agent a stable identity and adopts its source file.
The definition records a prompt source: harness, project-relative path and
SHA-256 of the decoded prompt. The complete prompt remains in the source native
file; its body is never returned by custody diagnostics or stored in the log.
The source must remain available for materialization. Existing structured
contracts without a prompt source continue to render their reviewed prose.

Add an approved destination assignment with `set` and connect a new native
path with `bind`, as below. Rendering transfers the complete prompt without
rewriting instructions, including fenced examples and line breaks. Models,
efforts and permissions remain explicit per harness. Literal preservation does
not prove that harness-specific instructions have the same meaning elsewhere.
Unsupported metadata or sensitive values refuse import; values are withheld.

If the source prompt changes, reconciliation blocks its propagation until you
record the new source revision explicitly:

```sh
vivac agents import <id> --harness claude-code --path .claude/agents/reviewer.md \
  --yes --why "Accept the revised native prompt"
vivac agents reconcile <id> --yes
```

This keeps the identity and other harness assignments. A changed destination
that is not the source remains a conflict; import does not infer an identity
from a matching name or silently take over another agent's path.

## Advanced discovery and custody operations

For scripted discovery, use `vivac agents scan --json` in the project's lane
folder. Guided sync already discovers these agents; a separate scan is optional.
Scan reads project
agent files in `.codex/agents/` and `.claude/agents/`, including agents added
after setup. It reports metadata, fingerprints and unsupported fields;
it does not adopt anything, execute a harness or return prompt bodies.
Personal configuration and linked directories are outside this scan.

An agent's identity is independent of its name and filename. Two agents
with the same name remain distinct until the person explicitly links
destinations to one identity.

Alternatively, declare a structured contract as reviewed prose. Do not paste a
native agent file or prompt into the definition; use native import to preserve
those instructions. A structured declaration uses temporary JSON (or a direct
object through MCP):

```json
{
  "schema_version": 1,
  "name": "reviewer",
  "contract": {
    "purpose": "Review changes and return evidence for the maintainer.",
    "duties": ["Inspect the changed paths and applicable rules."],
    "limits": ["Do not modify files or publish changes."],
    "acceptance": ["Cite each finding and state what was not checked."]
  },
  "assignments": [
    {
      "harness": "codex",
      "name": "reviewer",
      "model": "inherit",
      "effort": "high",
      "settings": {"sandbox_mode": "read-only"}
    },
    {
      "harness": "claude-code",
      "name": "reviewer",
      "model": "inherit",
      "effort": "high",
      "settings": {"tools": "Read, Glob, Grep"}
    }
  ]
}
```

Declare it with `vivac agents add --definition <file> --why "<reason>"`.
Use `--parent` and repeated `--against` as on an ordinary decision. The
command returns a stable agent id; `vivac why` explains its revision node.
The input file is a declaration, not another authority to maintain.

For an existing destination, use `vivac agents adopt <id> --harness H
--path <relative> --digest <sha256> --why "<reason>"`, using the exact
fingerprint from scan. Adoption checks metadata against the declared
assignment. The reviewed contract replaces the old prompt only when a
subsequent materialization is explicitly applied.

Unsupported native properties block adoption. Rewrite or extend the
adapter to represent the requirement before managing that destination;
silently dropping a property would change the agent's contract.

For a new destination, use `vivac agents bind <id> --harness H --path
<relative>`. Bind does not take ownership of an unrelated existing file.
Paths stay in the adapter's directory in the current lane.

## Change and materialize

`vivac agents show <id> --json` returns the declared contract and assignments.
`vivac agents set <id> --definition <file> --why "<reason>"` records a
complete new revision, retaining the agent's identity and the previous
decision's history. Review the whole assignment list: no provider mapping
is inferred. `inherit` is an intentional choice, not a verified model.
Changing one harness assignment leaves other destinations intact when their
contract and assignment are unchanged. A shared contract change affects every
bound destination that uses it.

`vivac agents diff <id> --json` shows the materialization plan.
`vivac agents sync <id>` previews it; `--yes` applies it. `--dry-run`
does not change the authority or native files. Already current destinations
are idempotent. Missing managed files are diagnosed and can be recreated
through the reviewed plan.
Explicit operations and `--json` return JSON. Status returns exit code 1 for pending,
diverged or unverified assignments; diff and preview sync return 1 when
materialization work remains. Invalid arguments return 2, redaction refusals 3 and
file or receipt failures 5. A contract without a bound destination is
reported as `unbound`, never as a loaded configuration.

A native file changed since adoption or the last materialization is a
conflict. Review it before accepting its exact current fingerprint with
`--accept-digest <sha256> --yes`. Accepting a fingerprint permits replacing
that destination; it does not import the changed prompt into the contract.
To incorporate a change, restate it in a new contract revision first.

Files are rechecked before writing. Failure is reported, including a
receipt that could not be appended after files changed; that operation
must not be treated as a successful synchronization.

## Configuration and execution

`vivac agents status --json` distinguishes the current source revision,
destination configuration and reported execution evidence. A generated
file does not prove that the harness loaded it. Neither a parseable model
name nor a declared effort proves account availability or effective
runtime settings.

After inspecting the running harness, record evidence explicitly with
`vivac agents observe <id> --harness H --path <relative> --revision <node>
--model M --effort E --evidence "<reason>"`. This is reported evidence,
not an automatic verification by vivac. It is tied to a revision and
destination; a later revision does not inherit proof of loading.

`vivac doctor` summarizes agent custody alongside project setup. The coordinator
resolves detailed differences within the authorized scope. Direct scan remains
available later; setup is not required again when an agent is added.

`vivac agents detach <id> --harness H --path <relative>` stops managing
one destination and leaves its file in place. `vivac agents retire <id>
--why "<reason>"` records retirement without deleting native files.
Retirement in the tree does not disable a file the harness can still load.

## Integrations

Adapters declare their native directory, adapter version, precedence, capabilities,
metadata inspection, prompt decoding and rendering. The lifecycle uses that interface,
so a new integration does not need a second custody system. The initial
adapters cover Codex and Claude Code. An unknown harness or unsupported
setting is refused; vivac does not substitute models or permissions.
Materialization receipts identify the adapter version. Project definitions
and runtime overrides are distinct: Claude Code project agents take precedence
over user agents with the same name, while invocation settings can override
the selected model. Vivac does not resolve those runtime overrides; inspect
the harness and record evidence before treating an assignment as loaded.
See the [Claude Code scope and model precedence reference](https://code.claude.com/docs/en/sub-agents#choose-the-subagent-scope)
for the harness's own selection rules.

Discovery and diagnostics are explicit local reads. They do not add
network calls, background watchers or harness processes to node writes.
A missing or obsolete derived index can make diagnostics slower: they read
the log without regenerating files. An ordinary read such as `vivac brief`
can rebuild the index.

## MCP results

Call `vivac_agents` with an `operation` and the fields needed by that operation.
The available operations are `inventory`, `assist`, `plan`, `apply`, `compare`,
`scan`, `status`, `show`, `diff`, `add`, `set`,
`retire`, `bind`, `adopt`, `detach`, `sync`, `observe`, `import` and `reconcile`. `definition` is a JSON
object, with the structure shown above, rather than a path to a file. Unknown
fields and options that do not belong to the selected operation are rejected.

`plan` and `apply` accept the selection object described above. Apply also
requires `plan_digest` and `yes`. Compare takes `selection` with `references`.
Assist takes `selection` with `references` and destination `harnesses`. Its
result contains `sources`, destination `harnesses`, `proposer: "session-model"`,
`applied: false` and `runtime_verified: false`. Each source includes its native
reference, metadata, body, bound identity and revision when present, and
`unrepresentable_settings` for each destination. The model authors the proposal;
these fields do not assert that any model was available or invoked.
CLI, MCP and web share the same fingerprint validation and batch preparation.

Reconcile accepts `mode` (`automatic` or `manual`) with `yes` and `why` to
record its lane policy. Its result includes `policy`, `applied`, `plan`,
`imported`, `needs_review`, `blocked`, `excluded` and runtime `unverified`.

The response contains `exit_code` and `result`. An `exit_code` of 1 can mean
pending synchronization or unverified execution; it is a status to continue
working from, not a failed MCP transport. Error responses withhold native file
contents and sensitive values. `sync` changes files and is advertised as a
destructive tool operation; previews remain available before applying it.
