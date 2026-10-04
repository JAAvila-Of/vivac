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

## Discover and adopt

Run `vivac agents scan --json` in the project's lane folder. It reads project
agent files in `.codex/agents/` and `.claude/agents/`, including agents added
after setup. It reports metadata, fingerprints and unsupported fields;
it does not adopt anything, execute a harness or return prompt bodies.
Personal configuration and linked directories are outside this scan.

An agent's identity is independent of its name and filename. Two agents
with the same name remain distinct until the person explicitly links
destinations to one identity.

First restate the contract as reviewed prose. Do not paste a native agent
file or prompt into the definition. Use a temporary JSON declaration:

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
All agent commands return JSON. Status returns exit code 1 for pending,
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

`vivac doctor` summarizes agent custody alongside project setup. Use the
agent commands to resolve the detailed differences. Setup's inventory
guidance is also available later through scan; setup is not required
again when an agent is added.

`vivac agents detach <id> --harness H --path <relative>` stops managing
one destination and leaves its file in place. `vivac agents retire <id>
--why "<reason>"` records retirement without deleting native files.
Retirement in the tree does not disable a file the harness can still load.

## Integrations

Adapters declare their native directory, adapter version, precedence, capabilities,
metadata inspection and rendering. The lifecycle uses that interface,
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
