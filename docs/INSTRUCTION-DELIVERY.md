# Checking instruction delivery

Before replacing an instruction file with the tree, check how each instruction
will reach the agent and when. A configured hook, a successful command and a
model receiving its output are different evidence.

| Evidence | What it establishes | What it does not establish |
|---|---|---|
| Configuration | The opening folder has the expected hooks, event selection and server | The harness trusted or ran them |
| Emission | The hook ran and its stdout contains the expected instructions | The harness included the complete output in model context |
| Reception | Harness context evidence contains the expected instructions for this opening | The agent consulted rule bodies or followed them |
| Consultation | Before a relevant action, the agent retrieved the applicable bodies and assessed the action against them | Correct behavior in every future session |

`vivac doctor claude-code` and `vivac doctor codex` separate configuration from
observed hook execution. Opening receipts are shared evidence in a lane's
tree log: they identify neither the harness nor the machine and do not prove
reception by a model.
A successful agent load check likewise does not establish its effective model,
effort or compliance without corresponding runtime evidence.

## What the opening carries

The opening hook emits every active pillar's complete title, the applicable
invariant titles, and the requirement to run `vivac rules` before deciding or
reviewing. It also carries agent custody guidance and the work capture
protocol. Pillar, rule and invariant bodies remain available through
`vivac rules`; they are not all injected into each opening. An obligation that
depends on a detail in a body needs that consultation before the action.

`SessionStart` covers `startup`, `resume`, `clear` and `compact` in both setup
integrations. Claude Code's entry has no source matcher; Codex's entry names
those four sources. `UserPromptSubmit` handles custody reconciliation and the
capture reminder, rather than reinjecting the opening brief on every message.
Calling `vivac brief` or its MCP equivalent mid-session retrieves orientation;
it is not evidence that the opening hook ran, and does not include the hook's
custody and capture sections.

Tree discovery starts from the hook process's working directory. A `cwd` field
in the input payload does not redirect it. Configure each folder where a
harness is opened, including a nested repository that shares its parent's
tree, as described in [Setup](SETUP.md).

## Bounded Codex opening

The Codex integration uses one synchronous invocation of
`vivac session start --hook --context codex`. Its JSON `additionalContext`
contains the existing opening text and operational notices. The handler has
`additionalContextLimit: 0`; vivac imposes its own hard ceiling of 60,000 UTF-8
bytes on the complete context instead. This avoids the default hook preview
without permitting unbounded output or coordinating independent renderers.
It does not change which rule bodies the brief includes.

If the initial brief exceeds the ceiling, vivac returns `continue: false` and
an explanation without reconciling custody or recording `session.started`.
If a custody notice makes the final context exceed the ceiling, the opening is
likewise rejected and no opening receipt is recorded; already authorized
custody changes remain applied. A busy-store notice is also subject to the
final ceiling. Rejection never disguises a shortened payload as a complete
one. Review the oversized opening rather than retiring its source instructions.

This is a transport budget, not a verified allowance in every model's context
window. Doctor checks the command, synchronous execution and handler limit;
reception remains separate evidence. Re-running `vivac setup codex` offers the
upgrade for the recognized original opening handler, preserving foreign hooks.
Approve the changed handler in Codex before relying on it.

If locating, loading or rendering the context fails, the bounded Codex mode
also returns `continue: false`, with a fixed explanation rather than error
details in the context. The diagnostic stays on stderr. The hook exits
successfully so Codex can process the JSON refusal; malformed commands retain
their usage error. Outside a vivac project, the hook remains silent.

Claude Code retains its plain-text opening hook. Its oversized-context problem
remains unresolved: the multi-handler consumer tests below establish transport
capacity, but not safe coordination of a dynamic vivac opening.
Do not assume that the Codex refusal mechanism also protects Claude Code:
the isolated Claude checks below found that `SessionStart` did not prevent a
provider request after receiving `continue: false`.

## Repeatable producer checks

From a checkout of vivac, run:

```sh
cargo clean -p vivac
cargo test --locked --test instruction_delivery
```

These tests generate both setup integrations in temporary projects and check
the configured opening command and event selection. They invoke that command
with synthetic event input from the workspace root and a directory inside a
nested repository, whose opening folder is declared as a lane of the same
tree. Its local `.vivac/lane` connection must point to that tree without
creating another event log. New pillar and invariant sentinels between
openings check freshness; a different tree in the input's `cwd` checks
discovery isolation.
Stdout is checked separately from stderr. The tests also distinguish titles
emitted by the brief from bodies retrieved through `rules`.

They do not launch Claude Code or Codex and cannot prove model reception.
Passing synthetic `source: compact` proves producer behavior for that input,
not that a real compaction triggered the hook.

## Observed reception on Windows

Local checks on 2026-10-09 used a development checkout reporting Vivac 0.19.3,
Claude Code 2.1.294 and Codex 0.162.0. Disposable projects and harness homes
contained invented instructions. A loopback provider received each request and
returned an intentional HTTP 400 before inference. These checks establish
context constructed by the harness, not understanding or compliance by a model.

| Harness and operation | Observed request context |
|---|---|
| Claude Code, small startup and resume | Opening sentinels arrived; resume included an instruction added after startup |
| Codex `exec`, small startup and resume | Opening sentinels arrived; resume included an instruction added after startup |
| Codex app-server, `thread/start` with `sessionStartSource: clear` followed by a turn | A fresh sentinel arrived; the opening receipt recorded `source: clear` |
| Claude Code, extensive opening | The hook emitted all 63 numbered title sentinels; the request contained only 2 |
| Codex `exec`, extensive opening | The request contained 11 of 63 numbered title sentinels, from the beginning and end; a saved hook output contained all 63 |
| Codex `exec`, bounded JSON opening, startup and resume | The complete producer context arrived unchanged; resume included a fresh instruction |
| Claude Code mod, synthetic `prompt.submit` context, startup and resume | The complete 60,000-character test string appeared as contiguous text in request context |

The bounded product check validated the setup-generated handler before using a
temporary wrapper to capture the producer's actual JSON stdout in memory and
re-emit its identical bytes. The receiver compared `additionalContext` with
the complete `developer/input_text`: 36,581 UTF-8 bytes at startup and 36,638
at resume, including 32,800 ASCII characters in synthetic titles. Each opening
recorded one receipt with the corresponding source. A separate oversized
product opening returned `continue: false`, with no provider request, custody
change or opening receipt. This demonstrates reception and rejection in the
tested harness, not actual compaction or model understanding.

An unsupported future event in a separate fixture also produced the fixed
preparation-failure JSON and a diagnostic on stderr. Codex processed that
refusal without a provider request, opening receipt or change to the fixture's
tree. This covers the observed loading failure, not every possible system error.

A separate Claude control check used a synthetic `SessionStart` command that
returned `{"continue":false,"stopReason":"Synthetic fixed stop reason."}`.
Claude reported the hook's successful execution, but still made two requests
to the local provider, which returned HTTP 400 before inference. Startup and
`--continue` behaved this way. Returning the same JSON with exit code 7 also
did not prevent either opening's requests. These checks used a temporary
harness home and a synthetic credential, with no tools or timeouts. They
establish that these `SessionStart` responses did not block the tested sends;
they do not establish behavior for other hook events or versions.

In the same isolated harness, `UserPromptSubmit` did prevent requests when its
command returned either `{"decision":"block","reason":"Synthetic fixed stop reason."}`
or `{"continue":false,"stopReason":"Synthetic fixed stop reason."}`, each with
exit code 0. Startup and `--continue` both finished without provider requests,
tools or timeouts. A local synthetic counter confirmed one command execution
per invocation; the stream output did not expose hook-response records for
this event. This establishes an observed send guard, not complete context
delivery or behavior after actual clearing or compaction.

An isolated local Claude plugin also passed native validation and supplied a
60,000-character synthetic string through `prompt.submit`. Every local
request contained the complete string at startup and `--continue`, using
either `--plugin-dir` or the `CLAUDE_CODE_PLUGIN_DIRS` environment variable.
A separate `prompt.context` check delivered the complete string at startup.
A `prompt.submit` handler returning `drop`, and a handler that threw with a
`.catch` returning `drop`, produced no provider requests. These are consumer
tests with synthetic modules, not a vivac setup integration. A subsequent
resume check also delivered a unique marker added only to its context.
Compaction, project-local configuration and the Rust producer required the
separate checks below.
The [mods reference](https://code.claude.com/docs/en/plugins/mods/reference)
describes the interfaces; the [event guide](https://code.claude.com/docs/en/plugins/mods/events#handle-a-hook-that-fails)
explains that a handler which fails without a refusal-producing `.catch` is
skipped, allowing the next handler to proceed.

Another synthetic-module check submitted two messages, a native `/compact`
command and a followup through `--input-format stream-json` in one Claude
process. A local provider returned invented successful streaming responses,
without inference. Claude emitted `system:compact_boundary` and the module's
`session.compact` marker. The followup received a fresh `prompt.context`
sentinel absent from earlier requests and test prompts. The handler did not
run again during the compact command itself, but did run on the followup in
the same process. This proves refresh on that path, not automatic compaction
in the middle of a turn or a vivac integration.

A further synthetic-module check registered `classic.SessionStart` without a
settings command for that event. In a single stream-json process, startup and
two real `/clear` commands produced the corresponding native opening events;
`session.end` reported both clears. The receiver never received `/clear` as a
literal user message. Complete synthetic `prompt.context` blocks exceeding
12,000 characters arrived in subsequent requests. A separate `--continue`
process reported `source: resume`. The same fixture also expanded a
fixture-owned `SessionStart` marker through `prompt.attachment`: the receiver
compared the entire replacement text, exceeding 12,000 characters, with the
requests for startup, both clears and resume. Attachments owned by the engine
were left unchanged. This establishes native opening metadata and intact
synthetic transport on those paths, not cancellation of an abandoned
preparation: the delayed hook did not observe its abort signal during clear.

A separate automatic-compaction fixture used synthetic provider usage to
trigger `session.compact` with `trigger: auto`, and observed a native
`system:compact_boundary`. No `precompute` trigger was observed in that run.
The baseline did not observe another `prompt.context` after the boundary in
its three submitted entries; it therefore does not establish fresh context
delivery after automatic compaction. These lifecycle checks use invented
responses and do not establish a working vivac integration.

A subsequent four-prompt automatic-compaction check registered
`classic.SessionStart` in the module, without a settings command. It returned
a small fixture-owned marker. `prompt.attachment` recognized only that marker
with hook origin and event `SessionStart`, and emitted 12,385 bytes
of synthetic context. The provider request immediately after the native
boundary, within the same turn, contained the new marker once. After a
second automatic compaction, the request contained the new marker
and no occurrence of the previous marker. That receiver counted markers;
it did not establish intact reception of the expanded text. `prompt.context`
did not reopen during that compaction; it reopened before the fourth
submitted prompt.

The receiver was then corrected to reconstruct each entire expected ASCII
snapshot in memory and compare it with the request's text fields. Repeated
12,385-byte and 60,000-byte cases both matched the complete snapshot in the
request immediately after the boundary. Another request, between the next
`session.compact(auto)` and `SessionStart(compact)`, did not contain that
snapshot intact; the fixture does not classify that request, which may have
been for summarization. After the new opening, the request contained the new
complete snapshot and not the old one. Do not infer loss from a main response
based on the intermediate request.

A separate refusal check called `turn.abort` after a simulated failure in
`session.compact` with trigger `auto`, and returned `skip` with a string
reason. No provider request followed the failure. Claude exited with
`error_during_execution` and no compaction boundary in that variant. This
establishes rejection before compaction, not rejection of a failed context
preparation after the new opening or safe loading through vivac setup.

A further failure variant simulated failed preparation in
`classic.SessionStart` with source `compact`, after the native boundary. A
`turn.step` guard called `turn.abort` and returned without calling the next
handler. Three provider requests preceded the failure; none followed it.
The fixture does not independently classify the preceding requests by their
purpose. Claude exited with
`error_during_execution`, without a timeout. This establishes refusal of the
next send in that variant, not refusal when the module fails to load, handler
exceptions or timeouts, or cancellation of a request already in flight.

A separate bridge check used a local module to call the stable vivac producer
through `$.process.run` and attach its decoded `additionalContext` through
`prompt.submit`. The complete startup context, 18,921 UTF-8 bytes, appeared
contiguously in both local startup requests. After a new synthetic constraint was
added, both resume requests contained the complete updated context, 19,931
bytes, including the new instruction. Each opening recorded one receipt in
the fixture, and the original workspace's event log remained unchanged. The
fixture used the existing bounded Codex JSON mode to test the bridge; this is
not a Claude setup integration or a Claude opening-lifecycle contract.

A further bridge used the same stable producer from native
`classic.SessionStart`, without a settings command. The module prepared one
snapshot per startup or resume event, returned its own small marker, and
expanded only that marker through `prompt.attachment`. The receiver compared
the complete producer text with both requests per opening: 18,921 UTF-8 bytes
at startup and 19,931 at resume, including the fresh resume instruction.
Each event invoked the producer once and recorded one `session.started` with
the native source and session identifier. The fixture's own event log was
verified before mutations; the original workspace log's hash was identical
before and after. Native plugin validation succeeded and `--plugin-dir`
loaded the module. This bridge still uses the bounded Codex producer mode
only as a fixture; it does not establish clear or compaction of that producer,
or installation and loading by vivac setup.

Plugin declaration alone also did not establish loading. In two isolated
non-interactive runs, project settings containing a local directory marketplace
and an enabled plugin did not activate the module. Registering the marketplace
and installing the plugin with `--scope project` in the isolated harness home
did activate it and delivered a complete 12,066-character test string. Project
settings with `env.CLAUDE_CODE_PLUGIN_DIRS` likewise did not activate a separate
module, while the process environment did. Workspace trust was not confirmed
in these runs; do not generalize their configuration behavior to trusted
interactive sessions or treat native validation as proof of loading.

The extensive Claude fixture also included four synthetic pillars. Their
titles and the instruction to consult `vivac rules` arrived, while the custody
and work capture guidance did not. The extensive Codex fixture had one
synthetic pillar and 62 constraints, with no active focus. Neither case
establishes delivery for a deeply nested focus or the original project's
instructions. No check established that a model read the saved output or
consulted rule bodies. The clear check exercised the app-server API, not the
interactive `/clear` command. Actual compaction of the vivac setup integrations
remains unverified; the synthetic Claude module check above has narrower scope.

A separate Claude consumer check used a deterministic test hook with three
markers in ASCII text. Both plain stdout and JSON `additionalContext` delivered
all three at 9,900 characters; at 10,100 characters, both delivered only the
first marker. The local receiver validated complete HTTP bodies in every case.
This demonstrates loss under those conditions and that JSON alone did not
avoid it; it does not establish the internal cause or an exact universal cutoff.

A subsequent consumer check configured two synchronous command handlers, each
returning 9,900 ASCII characters in JSON `additionalContext`. The receiver
compared each complete expected string against the request context, rather
than checking only its markers. Both complete strings arrived in both
harnesses. Claude delivered them in the reverse of configured order; Codex
delivered them in configured order in that run. Neither observation establishes
an ordering guarantee. Codex used `additionalContextLimit: 0` with strictly
bounded test producers. A separate Codex Unicode check also arrived intact:
9,900 code points occupying 9,950 UTF-16 code units. Claude also received both
complete blocks at startup and resume when one contained Unicode: that block
occupied 9,900 UTF-16 code units and contained 9,899 code points.

These checks demonstrate bounded transport, not a working multipart vivac
integration. Independent invocations of the current opening command each
render the tree, reconcile custody and attempt to record an opening. Simply
registering that command several times would repeat those effects and could
mix tree revisions. The consumed input fields, `session_id` and `source`, do
not uniquely identify repeated openings of the same kind within one session.
A multipart implementation therefore needs a reliable shared opening identity,
one immutable payload, explicit part numbering and limits, and handling for
missing parts and growth after setup. Time windows alone do not establish that
two handlers belong to the same opening.
In the isolated Claude startup and resume checks, the input had no separate
opening identifier, and `CLAUDE_ENV_FILE` differed between handlers. The
isolated Codex startup input likewise supplied no separate opening identifier.
These observations do not rule out future harness capabilities, but the
tested inputs provide no verified correlation mechanism for this design.

Codex's child processes needed the normal Windows `COMSPEC` and `PATHEXT`
values. Omitting them from the isolated environment made trusted handlers
fail. With that environment corrected, `exec` received the opening using
`--dangerously-bypass-hook-trust`; app-server received it after the fixture's
handlers were reviewed through `/hooks`. In this version, accepting the
bypass flag before `app-server` did not make its untrusted handlers execute.
Do not infer execution from argument acceptance or change personal trust
settings to reproduce an isolated check.

## Check a real harness separately

Create writable fixtures outside existing vivac trees. `VIVAC_HOME` isolates
the global registry, but does not override discovery of `.vivac` in an ancestor
of the working directory. Inside an existing tree, `vivac init` can declare a
lane of that tree rather than create an independent log. Before adding test
nodes, verify that the fixture has its own `.vivac/events`; do not treat a
successful initialization alone as proof of isolation.

Use a disposable project containing only invented sentinels. Keep instruction
files in the original project throughout this check. Set up and approve the
integration in every opening folder being tested. Record harness and vivac
versions, opening folder, event, expected sentinel and the evidence available.

For each harness and folder, check a new session, a resumed session, a cleared
session and an actual compaction. Add a fresh sentinel before each event so
old conversation context cannot masquerade as a new delivery. Do not repeat
the expected sentinel in the test prompt. Check the
opening hook's stdout and the context the harness includes for the next model
request, using its available inspection facilities. If those facilities do
not expose reception, label it unverified; an agent's unsupported assertion
that it received everything is insufficient.

Codex can replace oversized hook context with a head-and-tail preview and a
path to the full output. Its documented default threshold is approximately
2,500 tokens per handler. Legacy or customized handlers using that default
remain subject to this behavior; the bounded integration explicitly overrides
it. Vivac's soft brief budget alone cannot exclude a preview. Check whether required
instructions reached the model; a path alone does not show that it read the
saved text. See the [Codex hook reference](https://learn.chatgpt.com/docs/hooks#large-hook-output).
For Claude Code, see its [SessionStart reference](https://code.claude.com/docs/en/hooks#sessionstart).
Its [JSON output reference](https://code.claude.com/docs/en/hooks#json-output)
also documents a 10,000-character limit for plain stdout and individual
context fields, including `hookSpecificOutput.additionalContext`. Switching
to JSON alone does not establish complete delivery of oversized context.

Finally, request a harmless decision whose acceptance depends on a sentinel
in a rule body. Check that the agent retrieves `vivac rules` before deciding
and uses that body in its assessment. This tests consultation as well as
opening orientation. Retain only the minimal evidence needed; do not copy
credentials, private transcripts or native prompt bodies into the tree.

Retiring a source requires an inventory of its instructions, each destination
and delivery route, and the person's review of the result. These checks do
not themselves authorize retirement. See [Migrating](MIGRATING.md).
