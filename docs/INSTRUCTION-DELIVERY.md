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

The bounded modes also reject a loaded tree containing broken event lines,
before rendering, custody reconciliation or an opening receipt. The ordinary
reader still recovers readable events so `vivac brief` and `vivac check` can
help inspect a damaged store. Recoverable text is useful for repair, but cannot
establish that an omitted line contained no required instruction.

Claude Code retains its plain-text opening hook. Its oversized-context problem
remains unresolved: the multi-handler consumer tests below establish transport
capacity, but not safe coordination of a dynamic vivac opening.
Do not assume that the Codex refusal mechanism also protects Claude Code:
the isolated Claude checks below found that `SessionStart` did not prevent a
provider request after receiving `continue: false`.

## Experimental Claude module producer

`vivac session start --hook --context claude-module` prepares a bounded snapshot
for an experimental module. Setup does not install or activate that module;
the existing Claude integration remains unchanged. This command is not itself
a replacement for the opening hook or proof that Claude received its output.

Every response is one JSON object with `protocol: "vivac.claude.opening.v1"`.
`status: "ready"` includes the complete `context`, limited to 60,000 UTF-8
bytes. `status: "absent"` means no vivac tree was found and has no context.
`status: "rejected"` has no context and names `reason: "too-large"` or
`reason: "preparation-failed"`. Operational failures leave diagnostics on
stderr and exit successfully so the consumer can inspect the rejection;
invalid commands retain their usage error.

The producer shares Codex's single preparation, custody reconciliation and
opening receipt path, including its limits on operational notices. It does
not make the Codex JSON envelope a Claude hook response. A consuming module
must validate the protocol and refuse a failed or incomplete preparation
before any subsequent model send. Native module loading, cancellation,
handler failures and installation still need their own checks.

The consumer artifact is in `src/setup/claude-opening/`. It calls the producer
once per native `SessionStart` and keeps only the current snapshot in memory.
With a vivac tree present, the experimental consumer accepts ready context only
for `startup` and `clear`. It rejects `resume`, `compact` and unknown sources,
even when preparation succeeds: publishing a new context block did not reliably
replace the previous snapshot in the provider request. The preparation
can still record an opening receipt; that receipt does not establish delivery.
Outside a vivac tree, `absent` permits continuation, including resume and compact.
Its `prompt.context` handler preserves other context blocks, checks that the
opening is still current after downstream handlers finish, then adds the
complete snapshot as its own block. It does not send its context through the
classic hook's stdout or prepare it again on each render. New openings and
session endings invalidate the old snapshot; a late result cannot restore it.

`vivac session guard --hook` provides a separate command hook check. It reads
the `vivac_opening` object supplied by the module through the native
`UserPromptSubmit` input. The protocol must match, the status must be `ready`
or `absent`, and the generation must be a positive safe integer. Missing or
invalid input returns `continue: false` with a fixed explanation. Valid input
returns `continue: true`. Both exit successfully, without locating a tree or
changing project files. This receipt checks the module-to-command path; it is
not authentication or evidence of provider reception.

The module also rejects sends while preparation is pending or blocked, and
requires its own complete context block to have been published before
advancing a turn with context. These guards are experimental: configuring the
command alone does not establish their native ordering, execution or failure
behavior. Setup and
doctor do not install, enable or claim support for this artifact yet.

Its behavioral tests import the real artifact with a simulated event runtime:

```sh
node --test tests/claude_opening_module.mjs
```

They check complete UTF-8 boundaries, foreign context, stale openings and
renders, aborted handlers and blocked fallbacks. They do not prove native
module loading or reception. The CLI guard is also covered by `tests/session.rs`.

An initial native consumer check on Claude 2.1.294 exposed an ordering error:
`turn.step` ran before `prompt.attachment`, so requiring attachment expansion
at the turn guard blocked even a valid opening. A diagnostic variant that
relaxed only that requirement observed `prompt.context`, then `turn.step`,
then attachment rendering and a provider request. That variant establishes
the event order, not safe behavior. The consumer therefore uses the earlier
context-block event while retaining its publication requirement.

An isolated missing-module control observed no receipt and no provider
requests, with successful process exit. The native stream did not expose the
command hook's JSON response. Separate checks of the same executable confirmed
that a valid synthetic receipt permits continuation and missing input returns
`continue: false`; do not confuse those CLI checks with captured native hook
stdout.

The unmodified context-block consumer then passed a native startup control
with the real producer and command guard. An isolated twin project produced
the expected 4,303-character snapshot; the receiver compared it literally
with the published block and the provider request, keeping context only in
memory. It arrived exactly once, with all five synthetic sentinels. The live
fixture recorded one opening, and the native input carried a `ready` receipt
with generation 1. The process exited successfully without a timeout. This
small control establishes the working handoff and guard, not large-context
reception, compaction, setup installation or a model's understanding.

A later control of the same unmodified artifact received larger snapshots
at startup and after a real `/clear`: 19,791 and 19,859 UTF-16 units,
occupying 19,796 and 19,864 UTF-8 bytes respectively. Each complete string
matched an isolated twin producer's output literally and arrived once. After
clear, the previous complete snapshot appeared zero times and the fresh
sentinel appeared once. The live fixture recorded one startup and one clear;
baseline invocations wrote receipts only in the twins. The process exited
successfully without a timeout or blocked proxy requests. An earlier attempt
failed while recreating a twin directory in the helper, not in the consumer.

Twelve subsequent synthetic stress prompts did not produce a compaction
boundary or a `SessionStart` with source `compact`. That stress check does not
establish automatic compaction or delivery after it.

Before compact openings were restricted, a native `/compact` control did emit a
compaction boundary and reopen with source `compact`. Its follow-up request
received the fresh Unicode
sentinel and the complete 19,858-unit snapshot (19,865 UTF-8 bytes), matching
the isolated twin producer literally and appearing once. The previous
complete snapshot appeared zero times. The live fixture recorded one startup
and one compact; three provider requests included the compaction operation.
The process exited successfully without a timeout or blocked proxy requests.
The helper's report was adjusted afterward to account for auxiliary
compaction requests, using the already captured counters without another
launch. This establishes explicit compaction with that artifact, not
automatic compaction. These controls used an explicit plugin directory;
they do not establish installation, update, undo or doctor behavior.

Two native failure controls then exercised the real producer: exclusive
access to the fixture's config prevented reading it, and an operating-system
lock held the producer's store lock. Neither initial opening sent a provider
request or recorded an opening receipt. After releasing the lock, a real
`/clear` received the current complete snapshot once, with no previous copy.
The module sets a five-second process timeout, but the native stream did not
expose the child error or confirm termination. These observations do not
establish that a timeout kills the child or rolls back its writes.

A native resume control exposed a different failure in the preceding
consumer: after adding a fresh Unicode sentinel, the producer prepared and
`prompt.context` published the new 19,859-unit snapshot, but the provider
request contained the old snapshot once and the new snapshot zero times.
The turn guard accepted publication without detecting that difference.
The current experimental module therefore blocks resume with a present tree;
this is a refusal, not a completed resume integration.

The corrected artifact then passed a separate native rejection control.
Startup delivered the complete 19,862-unit baseline once and preserved the
foreign block once. Resuming that same session recorded one resume receipt
but sent no provider request and published no fresh block. Both processes
exited successfully. The native stream did not expose the command hook's
JSON response, so the control establishes the absence of a provider request,
not captured `continue: false` output.

A separate diagnostic on the same saved session observed the historical
`context_sections` attachment passing through `prompt.attachment` before
the provider request. The event exposed text and engine origin, without
structured sections identifying the own block. That establishes a rendering
hook, not a safe replacement mechanism for preserving foreign context.
No transcript was edited. Fresh-context resume remains an installation gate.

A fixture-only `prompt.compose` alternative placed the snapshot in its own
namespaced system section with session scope. A subsequent control invalidated
`prompt.section` after each successful opening. Both invalidation calls were
observed, and the composed sections matched the startup and fresh resume
snapshots literally. Startup sent its complete snapshot once. Resume nevertheless
sent the startup snapshot once and the fresh snapshot zero times; trimming and
line-ending normalization gave the same result, and the fresh sentinel was
absent. The foreign section arrived once in both requests. The two launches
reused the same native session ID and recorded one receipt per opening.
These observations reject that alternative on Claude 2.1.294; they do not
establish the cause or a safe way to replace saved context.

A separate automatic-compaction control of the real context-block consumer
used a synthetic 100,000-token window, a 50% threshold and simulated usage of
60,000 tokens before summary and 10,000 afterward. Those values are fixture
inputs, not measured token counts. The tree changed before the automatic
trigger. An initial downstream attempt failed; a later attempt completed a
summary, emitted a native compaction boundary and reopened with source
`compact`. The fourth prompt produced a normal request after that opening.
It preserved the foreign block once but sent the previous complete snapshot
once, the fresh complete snapshot zero times and no fresh sentinel. One startup
receipt and one compact receipt were recorded; the process exited successfully
without a timeout or blocked proxy traffic.

The experimental consumer therefore rejects ready compact openings as well as
resume. `source: compact` alone does not distinguish manual from automatic
compaction, so the earlier explicit-compaction success does not authorize that
source. Clear remains the checked recovery path. These controls neither activate
the module nor establish installation, cancellation, update, undo or doctor
support.

The restricted artifact passed a separate native refusal control with the same
synthetic automatic-compaction setup. Startup sent the complete snapshot and
foreign block once. After the tree changed, a summary completed, one compact
boundary appeared and one compact receipt was recorded. The fourth prompt sent
no subsequent provider request. The process exited successfully without a
timeout or blocked proxy traffic. The native stream did not expose the command
hook's JSON response; this proves refusal on that path, not fresh delivery or
captured `continue: false` output.

Two further fixture-only resume controls invalidated `prompt.context` after
preparing the fresh snapshot; the second also invalidated `prompt.attachment`.
Both calls completed, but both resumed requests still contained the original
snapshot once and the fresh snapshot zero times. Foreign context survived once.
Each control reused the native session ID, recorded one receipt per opening and
exited successfully. Invalidation alone did not repair the observed delivery.

A separate attachment failure control compared the same synthetic expansion
failure with and without `$.turn.abort`. Without abort, the provider received a
request despite the omitted attachment. With abort, no request reached it.
The single-prompt print process exited with an error and did not expose an abort
return acknowledgement; that result does not establish that an interactive
session becomes unusable.

A subsequent control used one process with continuous JSON input. After the
attachment failure, the catch invoked `$.turn.abort`; no provider request
followed, and the process remained alive.
An actual `/clear` then prepared fresh context; the next prompt sent that full
snapshot without the previous one, and the process exited successfully. The
fixture recorded only startup and clear receipts. These checks used synthetic
attachments alongside the current context-block module, not a completed
reference transport. They establish the tested refusal and recovery behavior,
not producer cancellation, installation readiness or an abort acknowledgement.

A fixture-only reference candidate then replaced the own `prompt.context`
block with a stable SessionStart marker. Its attachment handler substituted
the current snapshot from memory; it retained no previous snapshots. Startup
sent the complete context once. A second process resumed the same session after
the fixture tree changed: the request contained the new complete context once,
the previous context zero times and the fresh Unicode sentinel once. Foreign
context appeared once in both requests, with no unexpanded marker. Both
processes exited successfully and recorded one receipt per opening. This is
fresh-delivery evidence for the candidate, not the current product module.
Compaction, cancellation and concurrency remain gates.

A separate copy of that candidate injected one expansion error after selecting
the own attachment. Its catch ran, no request reached the provider, and the
continuous-input process remained alive. After `/clear`, one request contained
the new complete snapshot once, the old snapshot zero times, the foreign block
once and no marker. Startup and clear each recorded a receipt; the process
exited successfully. This covers a failure after attachment classification,
not an earlier downstream exception or a confirmed abort return.

A pre-result failure comparison exposed why attachment selection must precede
the downstream call. Both fixture copies injected the same error inside the
handler before calling `next`, after recognizing the own marker in input text.
The late-selection control sent a request with no complete snapshot and one
unexpanded marker. The early-selection variant recorded the own attachment
before that error, entered its catch and sent no request. The process remained
alive; `/clear` recovered one request containing the fresh complete snapshot
once, the previous snapshot zero times, foreign context once and no marker.
This injected handler error does not establish independent downstream-plugin
failure behavior.

A separate positive control of the early-selection candidate retained literal
startup and resume delivery: startup contained its complete snapshot once;
resume contained only the fresh complete snapshot once, including the Unicode
sentinel. Foreign context appeared once and no marker survived in either
request. Both processes reused the same session and exited successfully, with
one receipt per opening. These candidates remain fixture-only.

Two independent-hook controls then observed the actual chain order: the own
input was selected before the separate hook ran. When that hook threw, the
downstream result retained the marker and the candidate delivered the complete
snapshot once. When the hook returned `text: null`, the result lost the marker;
the candidate let a request proceed with no complete snapshot. Foreign context
survived once in both cases. Early selection alone therefore does not establish
safe delivery: a selected own reference must also survive the downstream result
before expansion, or the turn must be refused.

A guarded fixture candidate added that result check. The same independent-hook
comparison then retained complete delivery after an exception, while marker
removal entered the own catch and sent no request. The continuous-input process
remained alive, and `/clear` recovered the fresh complete snapshot once without
the previous one or a marker; foreign context survived once. A separate positive
control of that guarded artifact also retained complete, fresh startup and
resume delivery. These controls do not establish cancellation of the producer.

An automatic-compaction control of the guarded candidate used the same synthetic
window and simulated usage as the earlier context-block control. It completed a
summary and one native compact boundary within three prompts. The fixture changed
before the automatic hook; startup and compact each recorded one receipt.
The first normal request after compact contained the fresh complete snapshot
once, the previous one zero times, foreign context once and no marker. The native
process exited successfully. The reference wrapper initially returned failure:
its predicate required four prompts and the legacy `prompt.context` observation
after compact, although the foreign block survived in both actual requests.
The captured metadata then passed a separate offline evaluation using at most
four prompts and preservation in the provider requests. No further native launch
was performed, and the original probe failure remains recorded. The automatic
control's output did not record startup marker absence; the separate positive
control checked that condition.
This observation does not establish installation readiness.

A separate manual-compaction control used the same guarded artifact on Claude
2.1.294, with one continuous process and three prompts. After startup, the
fixture tree changed and a real `/compact` produced one summary, one manual
trigger and one compact boundary. The first normal request after that boundary
contained the fresh complete snapshot once, the previous one zero times,
foreign context once, no reference marker and the new Unicode sentinel once.
Startup and compact each recorded one opening receipt. The process exited
successfully without timeout. This proves reception for the tested manual path,
not installation readiness or safety under concurrent dispatches.

A deterministic Node hook-chain control then held producer A until opening B
became ready. A's late completion did not replace B's context or foreign opening
fields. In a second interleaving, attachment A waited for its downstream result
while B opened and became the active turn. A's error handler returned
`{ text: null }`, losing both the foreign text and an additional downstream
field. B remained ready, expanded its current context and was not aborted.
The complete conservation criterion failed. This is synthetic hook-chain
evidence, not a native concurrency observation; the guarded artifact must not
be promoted on the strength of its successful reception controls.

A separate fixture candidate changed that catch to recover the downstream
result and remove only the exact own marker, preserving other text and fields.
The local 2.1.294 `Caught` declaration says that calling `next` after it has
settled replays its last result without dispatching downstream again. A Node
mock implementing that contract passed the late-producer interleaving, the
stale-attachment interleaving with A's signal aborted, and a current-attachment
failure. The stale result preserved foreign content without introducing either
snapshot; B remained ready and was not aborted. The current failure preserved
foreign content, blocked the receipt and aborted only its captured active turn.
The mock counted one downstream execution despite two calls to `next`.
These passes do not establish native replay behavior, native concurrency safety
or readiness to replace the product module.

A native control on 2.1.294 added foreign text inside the same selected
attachment. Without a fault, the provider received the complete snapshot once,
the foreign text once and no marker. When an independent downstream hook removed
only the marker, the catch replay recovered the foreign text and the downstream
hook ran once. The active turn sent no provider request; an ordered clear and
follow-up then delivered the fresh complete snapshot once, the previous one
zero times, the foreign text once and no marker. However, the exterior hook did
not observe the catch's final return in the abort-enabled control. The original
probe therefore remained rejected under its final-return criterion; replay
observation alone does not prove delivery of the catch's return.

A deliberately unsafe fixture control removed only the abort call. It observed
the final return with foreign text preserved and no marker, followed by a
provider request containing zero complete snapshots. This control passed its
negative-observation criterion and explicitly rejected safety. Preserving the
returned text is insufficient: the active incomplete turn must also be refused.
Neither control establishes native concurrent-opening behavior or installation
readiness, and the unsafe variant is excluded from the product.

The preserving fixture candidate also passed separate normal-opening controls
on 2.1.294. A resumed session reused its native session ID after a fixture
change; its first request contained the fresh complete snapshot once, the
previous one zero times, foreign context once and no marker. Manual and
automatic compaction controls each completed a summary and one actual boundary,
then delivered the same fresh-once, old-absent and foreign-preserved result.
The automatic control used synthetic usage and window values, rather than
measured model token consumption. Its legacy callback predicate remained false;
acceptance used the counts in actual provider requests. Startup and each
corresponding reopening recorded one receipt. These controls tested the preserving
candidate itself; they do not establish project-file installation, native concurrent-opening
safety or activation of the setup integration.

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
contained invented instructions. A loopback provider received each request;
initial consumer checks returned an intentional HTTP 400, and subsequent
lifecycle checks returned synthetic successful responses without inference.
These checks establish
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

Further isolated checks exercised local installation separately from loading.
Adding a directory marketplace did not activate the module; installing its
plugin did. Disable and uninstall removed its text from requests, while enable
and reinstall restored it. Editing the module in the directory source changed
the code used by a subsequent session even before a native update refreshed
the cached copy. A cached file alone is therefore insufficient evidence of
which module code ran. These checks did not exercise an interactive trust
prompt or remote plugin sources.

Project and local scopes did not isolate a marketplace's name in the global
registry. With the same marketplace name in two disposable projects, registering
B replaced that registry entry; A then loaded B's module. With distinct
marketplace names, both entries coexisted and A retained its own module after
B was registered. Both plugins still had the same plugin name. Complete
12,038-character snapshots matched the respective requests in the distinct-name
control. Neither a project settings declaration nor a local directory path
alone prevents a global marketplace-name collision.

Vivac setup currently writes project files without running the harness to
configure itself or reading its personal configuration. Native plugin
registration has effects outside those project files. The isolated installation
checks do not authorize adding that process to setup, nor establish its
ownership, rollback or undo contract. The experimental producer does not
change this setup boundary.

The planned project bundle has a narrower ownership boundary than its
directory. It owns three fixed relative paths: `.claude-plugin/plugin.json`,
`hooks/hooks.json` and `hooks/register.mjs`. A separate `.vivac-owned.json`
receipt records the bundle version and the SHA-256 digest of each file. The
receipt cannot choose additional paths; its entries must match the fixed list.
Its canonical bytes, version and digests must match a locally recognized
published bundle. Recalculating an edited file's digest does not make that
bundle owned. Supported older receipts must remain in the local catalog;
unknown versions are conflicts. These checks detect edits, not authenticate
who wrote the files.

Before planning any changes, the installer must validate the complete receipt,
the types of the path components and the contents of all owned files. Linked,
unreadable, incomplete or unrecognized bundles are conflicts. An intact older
bundle can be updated together; an edited bundle must not be partly replaced.
Undo preserves the whole bundle when an owned file has been edited. For an
intact bundle, it removes only the owned files and receipt, then removes only
empty bundle directories. The shared `.claude/skills` and `.claude` directories
remain. Additional files belong to the person and remain in place.
These requirements do not establish protection against concurrent filesystem
changes between inspection and writing.

This ownership contract is not yet connected to setup. A filesystem rehearsal
does not prove that the Rust installer implements it, that Claude loads the
installed bundle, or that its final context transport is safe. When bundle
diagnostics are added, doctor must distinguish intact project files from
observed module execution; neither the receipt nor a matching digest establishes
runtime loading. Doctor does not yet inspect this bundle.

A Windows filesystem rehearsal exercised 64 lifecycle cases with two synthetic
bundle versions, plus two deliberately unsafe controls. The cases covered
creation, idempotence, update, removal, edits, unrecognized receipts, missing
parts, invalid metadata, file-type conflicts, sharing locks and symbolic links.
Twelve cases injected a failure after each of the four changes in initial
installation, update and undo, checking restoration of owned bytes and
preservation of foreign sentinels. The shared parent directories survived undo.

The initial 65-case rehearsal counted mutation attempts but did not classify
their destinations during successful changes or rollback. A separate unsafe
rollback control wrote and restored a foreign sentinel and passed that original
criterion. The corrected 66-case rehearsal classified destinations in memory
and required zero forbidden mutation attempts for every ordinary operation.
Both unsafe controls were detected despite identical final sentinel bytes;
the mixed control also performed a legitimate four-file update. All 66 cases
met their respective criteria. Passing an unsafe control means detecting its
violation, not accepting the operation as safe.

This rehearsal used a Python prototype, not the Rust installer or a native
Claude session. Its audit covered the Python filesystem APIs used by that
prototype, not direct native syscalls. It did not establish crash recovery,
bounded reads, junction handling or protection against concurrent filesystem
changes. The original report and unsafe-control evidence remain separate from
the corrected result.

The [loading reference](https://code.claude.com/docs/en/plugins/loading#plugins-shared-through-a-repository)
also describes project plugins placed under `.claude/skills/`, identified as
`<name>@skills-dir`. They require acceptance of the native workspace trust
dialog for the session's primary working directory. Trusting a parent directory
or using `-p` alone does not meet that condition, and the loader does not search
parent directories for these plugins. This path could preserve installation
within project files, but its trust and loading behavior needs a separate
interactive check before adoption.

Six isolated non-interactive controls tested that location on Claude 2.1.294.
Without `--plugin-dir`, the valid plugin did not supply its module's context,
including a control with a synthetic trust entry whose consumption was not
independently established. Absent and invalid-manifest controls likewise
supplied no module context. Native validation passed, and the same plugin with
`--plugin-dir` supplied its complete 12,050-character snapshot and lifecycle
marker. No plugin or marketplace install records appeared in the synthetic
profile. These results distinguish a functioning module from unproven project
autodiscovery; they do not reject the documented interactive trust path.

Two subsequent interactive controls reached Claude's terminal interface, but
exited before its workspace trust dialog. Startup attempted a connection to
`api.anthropic.com`, which the fixture's proxy refused; the loopback model
endpoint received no requests. The newly initialized synthetic profiles had
no accepted project trust. These controls establish neither acceptance of the
dialog nor project-plugin loading, and do not disprove the documented path.

A final isolated control prepared onboarding and approved a fake API key in
the synthetic profile, without preaccepting project trust. Claude then showed
the native trust dialog for that fixture directory. After accepting it, the
project plugin loaded and supplied its complete 12,050-character snapshot in
both observed requests. The interactive process exited with status 0.
Reopening the same project and profile with `-p` also loaded the module and
supplied the entire snapshot in both requests; it exited with status 1 because
the loopback provider intentionally returned HTTP 400, without a timeout.
Claude persisted project trust and plugin usage in its synthetic profile, but
created no marketplace or plugin installation records. This establishes the
project-file loading path after native trust acceptance, not automatic onboarding
of a new account or loading before trust.

A native bridge then exercised the real bounded producer at startup and two
real `/clear` commands in one stream-json process. It prepared snapshots of
18,921, 19,974 and 21,027 UTF-8 bytes. Each subsequent request contained its
complete snapshot once, including the fresh constraint added before that
clear. Three native opening events invoked the producer three times and
recorded three receipts with matching source and session identifier. Both
clears produced `session.end` events; `/clear` never became a literal provider
prompt.

A separate real-producer bridge triggered two automatic compact boundaries
within one process. The complete startup snapshot was 18,957 UTF-8 bytes;
the two fresh compact snapshots were 18,993 and 19,029 bytes. Requests
immediately following the corresponding openings contained the entire current
snapshot once. After a new compact opening, the old complete snapshot was
absent. One producer invocation and one receipt matched each native opening.
The fresh constraints came from the fixture tree, not the submitted prompts.
Both bridges used the existing Codex JSON mode only as a fixture, verified
their own event log before writes and left the original workspace log unchanged.
They establish real-producer transport on those paths, not installation by
vivac setup or the experimental Claude protocol's native reception.

The same bridges were subsequently adapted to the experimental
`claude-module` producer, changing the command mode and validating
`vivac.claude.opening.v1` with `status: "ready"` before attaching its context.
With the minimal adaptation, startup and two real clears each delivered their
complete snapshot once in the first corresponding provider request. A separate
startup and two automatic-compaction cycle passed the same first-request
comparison, including fresh instructions and absence of the previous complete
snapshot. Each cycle matched three preparations to three opening receipts,
finished without a timeout and left the original workspace log unchanged.
A broader adapter did not pass its reception assertions; those results remain
unverified. The minimal bridges establish reception of this producer in those
fixtures, not the final module's failure guards, cancellation or setup loading.

Failure checks validated modules before loading them. Without a module, and
with a preparation exception but no `.catch` fallback, the provider received
a request. With the tested fallbacks, startup exceptions, rejected promises
and an actual hook timeout instead made `prompt.submit` drop the entry before
any request. A native resumed session with a preparation exception was also
blocked. After an automatic compact opening failed, a `turn.step` fallback
returned a terminal result without calling the next handler; no provider
request followed that failure. Claude reported a successful terminal result
in this variant: it means the runtime finished without a send, not that a model
completed the requested work. These checks do not cover a fallback that itself
fails or cancellation of an already transmitted provider request. The
[module event reference](https://code.claude.com/docs/en/plugins/mods/events)
documents that handler failures without a fallback can be skipped.

A separate command guard checked module absence. A loaded
`classic.UserPromptSubmit` handler added a fixture-owned field to a copy of
its native input. The settings command received that field: it allowed the
loaded control and blocked absent or throwing-module controls, with no provider
request in either blocked case. A fixed environment stamp failed the same
test: an absent module inherited the stamp from its parent and passed the
guard. This is evidence of that input-field path on the tested version, not an
authentication mechanism or a guarantee for future harness versions.

A native SDK interrupt during a suspended hook returned a matching success
control response and ended the turn with `aborted_streaming`, before any
provider request. The suspended wait reported neither completion nor rejection;
its abort signal remained unobserved. A follow-up completed in the same process,
but no clear boundary was observed. This proves abandonment of that turn,
not invalidation of a prepared snapshot or rollback of producer effects.
A second control sent an interrupt while `classic.SessionStart` reported a
pending preparation. The preparation reached ready before the interrupt's
success acknowledgement; the aborted turn sent no request, and the follow-up
used ready state. That ordering did not exercise cancellation of SessionStart
or a guard blocking pending state. It does not close the cancellation check.

Local declarations generated by Claude 2.1.277 give `next.signal` an
abandonment contract for its own dispatch, including budget expiry. In those declarations,
`$.process.run` accepts `timeoutMs`, which can kill its child, but has no
`signal` option. Those types alone do not establish runtime behavior on
2.1.294. A turn's
interruption and a `session.end` event are separate observations; neither alone
proves that a pending opening dispatch was abandoned. A consuming module must
check whether its preparation still belongs to the current opening before
publishing the result. Discarding a late result does not undo a receipt already
written by the producer or cancel a provider request already sent.

A native timeout fixture on 2.1.294 then checked late completion directly.
Its first opening waited on an in-module timer beyond the hook's budget.
The `.catch` handler invalidated that opening's generation before any provider
request, and `prompt.submit` dropped the first prompt. A real `/clear` prepared
snapshot B before timer A finished. When A resumed, its signal was aborted
and its generation was no longer current; it published no ready state and
supplied no context. The provider received B alone. The final timeout control
exited successfully without a process timeout. A separate quick control
received A completely, but its initial instrumentation did not capture the
process's exit code. These checks establish rejection of late timer completion
after hook timeout and clear, not cancellation of a `$.process.run` child,
SDK turn interruption or the final real-producer module.

The local declarations generated by 2.1.294 also document that a
`$.process.run` timeout kills its child and rejects the call. Its options expose
neither an abort signal nor a process handle. They do not promise that turn
interruption terminates that child. The separate `$.process.spawn` contract
ties child lifetime to its stream and dispatch scope. These declarations do
not replace observing the real producer under cancellation.

A first native child-observation control held the fixture's store lock and
looked for the producer by exact executable path and ancestry from the launched
Claude process. Neither its timeout case nor its planned interrupt case found
a matching child. No interrupt was sent without that match. Both cases recorded
zero provider requests and zero opening receipts before releasing the lock, and
Claude exited successfully. The probe rejected both as inconclusive. These
results establish neither child termination nor safe cancellation; the observer
and fixture preparation must be checked before interpreting a rerun.

The observer's Windows process-entry structure was missing a field. A corrected
structure passed a separate owned-child check, and a native rerun then found
the exact producer in both cases. With the store lock still held, each child
was alive at approximately one second and terminated at approximately 6.2
seconds; neither required cleanup termination. No provider request or opening
receipt preceded lock release. In the interrupt case, the acknowledgement
arrived at approximately five seconds, rather than before the one-second
observation. This does not demonstrate immediate cancellation by interruption.
Both clears recorded a new receipt. The interrupt recovery sent one provider
request; the timeout case reported a result but no provider request, so the
probe remained rejected. Its clear/follow-up result ordering needs an explicit
check before claiming timeout recovery.

A further native run waited explicitly for the clear result before submitting
the follow-up. Both timeout and interrupt controls then passed the observation
and recovery criteria: exact children alive at one second and terminated at
6.2 seconds while the lock remained held, no cleanup termination, no provider
request or opening receipt before release, and one clear receipt followed by
one actual provider request. Both native processes exited successfully.
The interrupt acknowledgement still arrived at approximately five seconds.
This establishes the tested bounded wait and recovery, not immediate
interrupt-driven termination, rollback of producer effects or cancellation
outside this locked-store fixture.

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
