import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const candidatePath = new URL('../src/setup/claude-opening/hooks/register.mjs', import.meta.url);
const { register } = await import(candidatePath.href);
const candidateSha256 = createHash('sha256').update(await readFile(candidatePath)).digest('hex').toUpperCase();
process.stdout.write(`# candidateSha256=${candidateSha256}; harness=offline-logical-hooks; nativeBehaviorProven=false\n`);

const marker = '@@VIVAC_OPENING_CURRENT_REFERENCE_8f716aa0c351448fb5ce9c8b21e7a699@@';
const protocol = 'vivac.claude.opening.v1';
const contextA = 'SYNTHETIC_CONTEXT_A_9d2d';
const contextB = 'SYNTHETIC_CONTEXT_B_613f';
const foreignA = 'SYNTHETIC_FOREIGN_A_55a1';
const foreignB = 'SYNTHETIC_FOREIGN_B_27b4';
const producerDiagnostic = 'SYNTHETIC_PRIVATE_DIAGNOSTIC';
const nativeEvents = ['classic.PreToolUse', 'agent.spawn', 'prompt.submit', 'turn.step'];

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((done, failed) => { resolve = done; reject = failed; });
  return { promise, resolve, reject };
}

function ready(context) {
  return { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'ready', context }) };
}

function nextResult(produce, signal = new AbortController().signal) {
  const calls = [];
  const next = (event) => {
    calls.push(event);
    return typeof produce === 'function' ? produce(event, calls.length) : produce;
  };
  next.signal = signal;
  next.calls = calls;
  return next;
}

function harness(processRun = async (_argv, options) => ready(JSON.parse(options.stdin).session_id === 'B' ? contextB : contextA)) {
  const hooks = new Map();
  const producerCalls = [];
  const $ = { process: { run: (...args) => { producerCalls.push(args); return processRun(...args); } } };
  Object.defineProperty($, 'turn', { get() { assert.fail('opening context must not control a native turn'); } });
  register((name, handler) => {
    assert.equal(hooks.has(name), false, name);
    hooks.set(name, handler);
    return { catch() { assert.fail('downstream errors must not be retried through a hook catcher'); } };
  });
  function invoke(name, event, next) {
    const handler = hooks.get(name);
    return handler ? handler($, event, next) : next(event);
  }
  function open(id, source = 'startup', value = { additionalContext: [foreignA], nativeField: id }) {
    return invoke('classic.SessionStart', { session_id: id, source, cwd: 'synthetic-fixture' }, nextResult(value));
  }
  function receipt(event = { text: 'synthetic prompt' }, signal) {
    return invoke('classic.UserPromptSubmit', event, nextResult(value => value, signal));
  }
  return { $, hooks, producerCalls, invoke, open, receipt };
}

function ownEvent(text = marker) {
  return { origin: { kind: 'hook', event: 'SessionStart' }, text, fixtureField: 'original-event' };
}

function childEvent(agentId, text) {
  return { agentId, origin: { kind: 'hook', event: 'SubagentStart' }, text };
}

function openChild(h, agent_id, next = nextResult({ additionalContext: [foreignA] })) {
  return h.invoke('classic.SubagentStart', { agent_id, cwd: 'child-fixture' }, next);
}

function attach(h, event, result = { text: event.text }, signal) {
  return h.invoke('prompt.attachment', event, nextResult(result, signal));
}

function assertNativeFlow(h) {
  for (const name of nativeEvents) assert.equal(h.hooks.has(name), false, name);
  for (const [name, event] of [
    ['classic.PreToolUse', { tool: 'SendMessage', recipient: 'teammate', agentId: 'unknown-child' }],
    ['agent.spawn', { resume: { agentId: 'unknown-child' } }],
    ['prompt.submit', { text: 'native follow-up' }],
    ['turn.step', { turnId: 'native-turn', agentId: 'unknown-child' }],
  ]) {
    Object.defineProperty(event, 'arguments', { get() { assert.fail('native arguments must remain unread'); } });
    const result = { native: name };
    const next = nextResult(result);
    assert.strictEqual(h.invoke(name, event, next), result);
    assert.deepEqual(next.calls, [event]);
    assert.strictEqual(next.calls[0], event);
  }
}

for (const state of ['uninitialized', 'ready', 'absent', 'pending', 'blocked']) {
  test(`${state} context preserves native execution without reading or rewriting events`, async () => {
    const pending = deferred();
    const h = harness(async () => state === 'pending' ? pending.promise
      : state === 'absent' ? { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }
      : state === 'blocked' ? { exitCode: 1, stdout: producerDiagnostic } : ready(contextA));
    let opening;
    if (state !== 'uninitialized') {
      opening = h.open('A');
      if (state !== 'pending') await opening;
    }
    assertNativeFlow(h);
    if (state === 'pending') { pending.resolve(ready(contextA)); await opening; }
  });
}

test('messages, resumed spawns and unknown child turns form an unchanged native continuation', async () => {
  const h = harness();
  await h.open('A');
  const known = (await openChild(h, 'known-child')).additionalContext.at(-1);
  const before = await h.receipt();
  for (const target of ['known-child', 'unknown-child', 'teammate', 'other-session', '', undefined]) {
    const message = { tool: 'SendMessage', input: { recipient: target, message: 'native message' } };
    const delivered = { isDelivered: true, target };
    const next = nextResult(Promise.resolve(delivered));
    const promise = h.invoke('classic.PreToolUse', message, next);
    assert.strictEqual(await promise, delivered);
    assert.deepEqual(next.calls, [message]);
    const resumed = { resume: { agentId: target } };
    const resumedNext = nextResult(delivered);
    assert.strictEqual(h.invoke('agent.spawn', resumed, resumedNext), delivered);
    assert.deepEqual(resumedNext.calls, [resumed]);
    const event = { turnId: 'resumed-turn', agentId: target };
    const stream = (async function* () { yield { kind: 'native-step' }; return delivered; })();
    const turnNext = nextResult(stream);
    assert.strictEqual(h.invoke('turn.step', event, turnNext), stream);
    assert.deepEqual(await stream.next(), { done: false, value: { kind: 'native-step' } });
    assert.deepEqual(await stream.next(), { done: true, value: delivered });
    assert.deepEqual(turnNext.calls, [event]);
    const raw = { text: marker, native: true };
    assert.strictEqual(await attach(h, childEvent(target, marker), raw), raw);
  }
  assert.equal(h.producerCalls.length, 2);
  assert.equal((await attach(h, childEvent('known-child', known))).text, contextA);
  assert.deepEqual(await h.receipt(), before);
});

for (const name of nativeEvents) {
  for (const failure of ['throw', 'reject']) {
    test(`${name} preserves the exact native ${failure} with one next call`, async () => {
      const h = harness();
      await h.open('A');
      const event = { tool: 'SendMessage', resume: null, agentId: 'unknown' };
      const error = new Error('synthetic native failure');
      const next = nextResult(() => { if (failure === 'throw') throw error; return Promise.reject(error); });
      if (failure === 'throw') assert.throws(() => h.invoke(name, event, next), value => value === error);
      else await assert.rejects(h.invoke(name, event, next), value => value === error);
      assert.deepEqual(next.calls, [event]);
    });
  }
  test(`${name} preserves a native promise without replacing it`, () => {
    const h = harness();
    const promise = Promise.resolve({ native: true });
    const event = {};
    Object.defineProperty(event, 'resume', { get() { assert.fail('resume must not be inspected'); } });
    const next = nextResult(promise);
    assert.strictEqual(h.invoke(name, event, next), promise);
    assert.deepEqual(next.calls, [event]);
  });
}

test('the reviewed opening bundle has the expected candidate bytes', () => {
  assert.equal(candidateSha256, '7D064161DBB079F1584013186F7A9F6DBEAF8815E22CF5A026950D29B77E0E4A');
});

for (const [version, digest] of [
  ['v1', 'B4BB19565116606AFD310765AC16FF46A336294AD38BD905915D91035068E376'],
  ['v3', 'AA0C9DC2BA5C348D65CFEF17F891ABA3B9853B555D0D9E71B56E1100F33C6F07'],
  ['v3-release', '7A3AFC91606D1E756BACABAEFF84B98A3415392865DCB0E6BFBED946B58C84AF'],
]) {
  test(`the historical opening reference preserves the exact ${version} bytes`, async () => {
    const path = new URL(`../src/setup/claude-opening-versions/opening-reference-${version}/hooks/register.mjs`, import.meta.url);
    assert.equal(createHash('sha256').update(await readFile(path)).digest('hex').toUpperCase(), digest);
  });
}

test('one preparation per opening preserves invocation options and complete foreign fields', async () => {
  const h = harness();
  const foreign = { additionalContext: [foreignA], retained: { value: true } };
  const opened = await h.open('A', 'startup', foreign);
  assert.deepEqual(opened.additionalContext, [foreignA, marker]);
  assert.strictEqual(opened.retained, foreign.retained);
  assert.deepEqual(foreign.additionalContext, [foreignA]);
  assert.deepEqual(h.producerCalls[0], [['vivac', 'session', 'start', '--hook', '--context', 'claude-module'],
    { stdin: JSON.stringify({ hook_event_name: 'SessionStart', source: 'startup', session_id: 'A' }), timeoutMs: 5000, cwd: 'synthetic-fixture' }]);
  for (let index = 0; index < 3; index++) {
    const real = { text: `${foreignA}${marker}${foreignB}`, retained: foreign.retained };
    const result = await attach(h, ownEvent(), real);
    assert.equal(result.text, `${foreignA}${contextA}${foreignB}`);
    assert.strictEqual(result.retained, foreign.retained);
  }
  assert.equal(h.producerCalls.length, 1);
  const child = await openChild(h, 'target');
  const reference = child.additionalContext.at(-1);
  assert.match(reference, /^@@VIVAC_OPENING_CHILD_REFERENCE_[a-f0-9-]+@@$/);
  assert.deepEqual(JSON.parse(h.producerCalls[1][1].stdin), { hook_event_name: 'SubagentStart', agent_id: 'target' });
  assert.equal((await attach(h, childEvent('target', reference))).text, contextA);
  assert.equal((await attach(h, childEvent('target', reference), { text: reference, native: true })).text, '');
  assert.equal(h.producerCalls.length, 2);
  assertNativeFlow(h);
});

for (const recipient of ['root', 'child']) {
  test(`${recipient} accepts exactly 60000 UTF8 bytes without truncation`, async () => {
    const context = '🧭'.repeat(15000);
    assert.equal(Buffer.byteLength(context), 60000);
    const h = harness(async () => ready(context));
    const opened = recipient === 'root' ? await h.open('A') : await openChild(h, 'target');
    const reference = opened.additionalContext.at(-1);
    const event = recipient === 'root' ? ownEvent(reference) : childEvent('target', reference);
    const real = { text: `${foreignA}${reference}${foreignB}`, retained: {} };
    const result = await attach(h, event, real);
    assert.equal(result.text, `${foreignA}${context}${foreignB}`);
    assert.strictEqual(result.retained, real.retained);
    assertNativeFlow(h);
  });
}

const producerCases = [
  ['invalid protocol', { protocol: 'wrong', status: 'ready', context: producerDiagnostic }],
  ['missing protocol', { status: 'ready', context: producerDiagnostic }],
  ['rejected status', { protocol, status: 'rejected', reason: producerDiagnostic }],
  ['unknown status', { protocol, status: 'unknown', reason: producerDiagnostic }],
  ['missing status', { protocol, context: producerDiagnostic }],
  ['absent with context', { protocol, status: 'absent', context: producerDiagnostic }],
  ['absent with null context', { protocol, status: 'absent', context: null }],
  ['numeric context', { protocol, status: 'ready', context: 42 }],
  ['boolean context', { protocol, status: 'ready', context: false }],
  ['null context', { protocol, status: 'ready', context: null }],
  ['array context', { protocol, status: 'ready', context: ['unexpected'] }],
  ['object context', { protocol, status: 'ready', context: { unexpected: true } }],
  ['missing context', { protocol, status: 'ready' }],
  ['empty context', { protocol, status: 'ready', context: '' }],
  ['null envelope', null],
  ['array envelope', []],
  ['scalar envelope', 7],
].map(([label, value]) => [label, { exitCode: 0, stdout: JSON.stringify(value) }]);
producerCases.push(
  ['stdout truncated', { ...ready(producerDiagnostic), isStdoutTruncated: true }],
  ['stderr truncated', { ...ready(producerDiagnostic), isStderrTruncated: true }],
  ['nonzero exit', { exitCode: 1, stdout: ready(producerDiagnostic).stdout }],
  ['unparseable output', { exitCode: 0, stdout: producerDiagnostic }],
  ['UTF8 60001 bytes', ready('🧭'.repeat(15000) + 'x')],
);

for (const [label, produced] of producerCases) {
  for (const recipient of ['root', 'child']) {
    test(`${recipient} rejects ${label} context without blocking native work or exposing diagnostics`, async () => {
      let failing = false;
      const h = harness(async () => failing ? produced : ready(contextA));
      await h.open('A');
      const sibling = await openChild(h, 'sibling');
      failing = true;
      const foreign = { additionalContext: [foreignB], retained: {} };
      const opened = recipient === 'root' ? await h.open('B', 'resume', foreign)
        : await openChild(h, 'target', nextResult(foreign));
      assert.strictEqual(opened, foreign);
      assert.equal(JSON.stringify(opened).includes(producerDiagnostic), false);
      if (recipient === 'root') assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
      else {
        assert.equal((await h.receipt()).vivac_opening.status, 'ready');
        assert.equal((await attach(h, childEvent('sibling', sibling.additionalContext.at(-1)))).text, contextA);
      }
      const raw = { text: recipient === 'root' ? marker : 'unclassified child text', retained: {} };
      const event = recipient === 'root' ? ownEvent() : childEvent('target', raw.text);
      const result = await attach(h, event, raw);
      assert.equal(result.text, recipient === 'root' ? '' : raw.text);
      assert.strictEqual(result.retained, raw.retained);
      assertNativeFlow(h);
    });
  }
}

for (const recipient of ['root', 'child']) {
  test(`${recipient} producer rejection is contained without retrying the native handler`, async () => {
    const h = harness(async () => { throw new Error(producerDiagnostic); });
    const event = recipient === 'root' ? { source: 'startup', session_id: 'A' } : { agent_id: 'target' };
    const real = { additionalContext: [foreignA], retained: {} };
    const next = nextResult(real);
    assert.strictEqual(await h.invoke(recipient === 'root' ? 'classic.SessionStart' : 'classic.SubagentStart', event, next), real);
    assert.deepEqual(next.calls, [event]);
    assert.equal(JSON.stringify(real).includes(producerDiagnostic), false);
    assertNativeFlow(h);
  });
}

for (const recipient of ['root', 'child']) {
  for (const additionalContext of [null, 'foreign', [foreignA, null], [foreignA, 7], [foreignA, {}], [foreignA, []]]) {
    test(`${recipient} malformed foreign additionalContext remains intact without publishing readiness`, async () => {
      const h = harness();
      await h.open('A');
      const real = { additionalContext, retained: {} };
      const opened = recipient === 'root' ? await h.open('B', 'resume', real) : await openChild(h, 'target', nextResult(real));
      assert.strictEqual(opened, real);
      assert.strictEqual(opened.additionalContext, additionalContext);
      if (recipient === 'root') assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
      assertNativeFlow(h);
    });
  }
}

for (const id of ['', null, 7, false, {}, [], 'invalid/id', 'x'.repeat(257)]) {
  test(`invalid child identity ${String(id)} does not prepare or borrow context`, async () => {
    const h = harness();
    await h.open('A');
    const real = { native: true };
    const event = { agent_id: id };
    const next = nextResult(Promise.resolve(real));
    assert.strictEqual(await h.invoke('classic.SubagentStart', event, next), real);
    assert.deepEqual(next.calls, [event]);
    assert.equal(h.producerCalls.length, 1);
    const attachment = { text: marker, retained: {} };
    assert.strictEqual(await attach(h, childEvent(id, marker), attachment), attachment);
    assertNativeFlow(h);
  });
}

test('exact child identity and hook origin are required without changing foreign results', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  const b = (await openChild(h, 'b')).additionalContext.at(-1);
  for (const event of [
    childEvent('unknown', a), childEvent('a', b), childEvent('b', a), childEvent('A', a),
    { ...childEvent('a', a), origin: { kind: 'hook', event: 'SessionStart' } },
    { ...childEvent('a', a), origin: { kind: 'file', event: 'SubagentStart' } },
    ownEvent(a),
  ]) {
    const real = { text: event.text, retained: {} };
    assert.strictEqual(await attach(h, event, real), real);
  }
  assert.equal((await attach(h, childEvent('a', a))).text, contextA);
  assert.equal((await attach(h, childEvent('b', b))).text, contextA);
});

for (const duplicate of ['event', 'result', 'both']) {
  test(`duplicate child marker in ${duplicate} is omitted without disturbing siblings or native work`, async () => {
    const h = harness();
    await h.open('A');
    const sibling = (await openChild(h, 'sibling')).additionalContext.at(-1);
    const target = (await openChild(h, 'target')).additionalContext.at(-1);
    const event = childEvent('target', duplicate === 'result' ? target : target + target);
    const real = { text: foreignA + (duplicate === 'event' ? target : target + target) + foreignB, retained: {} };
    const next = nextResult(real);
    const result = await h.invoke('prompt.attachment', event, next);
    assert.equal(result.text, foreignA + foreignB);
    assert.strictEqual(result.retained, real.retained);
    assert.deepEqual(next.calls, [event]);
    const subsequent = await attach(h, childEvent('target', target));
    assert.equal(subsequent.text, '');
    assert.equal(subsequent.text.includes(contextA), false);
    assert.equal((await attach(h, childEvent('sibling', sibling))).text, contextA);
    assert.equal((await h.receipt()).vivac_opening.status, 'ready');
    assertNativeFlow(h);
  });
}

test('a foreign field accessor is preserved without reading it when expanding owned text', async () => {
  const h = harness();
  await h.open('A');
  const real = { text: marker };
  const getter = () => { assert.fail('foreign accessor must not be evaluated'); };
  Object.defineProperty(real, 'foreignField', { enumerable: true, get: getter });
  const result = await attach(h, ownEvent(), real);
  assert.equal(result.text, contextA);
  assert.strictEqual(Object.getOwnPropertyDescriptor(result, 'foreignField').get, getter);
});

for (const real of [null, undefined, 7, false, 'foreign scalar', [], {}, { text: 7 }, { text: null }]) {
  test('malformed or missing foreign attachment text is preserved exactly and never replayed', async () => {
    const h = harness();
    await h.open('A');
    const event = ownEvent();
    const next = nextResult(real);
    assert.strictEqual(await h.invoke('prompt.attachment', event, next), real);
    assert.deepEqual(next.calls, [event]);
    assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
    assertNativeFlow(h);
  });
}

test('a disappearing owned reference preserves the actual foreign result and invalidates only its opening', async () => {
  const h = harness();
  await h.open('A');
  const target = (await openChild(h, 'target')).additionalContext.at(-1);
  const sibling = (await openChild(h, 'sibling')).additionalContext.at(-1);
  const real = { text: foreignB, retained: {} };
  assert.strictEqual(await attach(h, childEvent('target', target), real), real);
  assert.equal((await attach(h, childEvent('target', target))).text, '');
  assert.equal((await attach(h, childEvent('sibling', sibling))).text, contextA);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  assertNativeFlow(h);
});

for (const name of ['classic.SessionStart', 'classic.SubagentStart', 'prompt.attachment', 'classic.UserPromptSubmit', 'session.end']) {
  for (const kind of ['throw', 'reject']) {
    test(`${name} propagates the same downstream ${kind} without a fallback call`, async () => {
      const h = harness();
      await h.open('A');
      const event = name === 'classic.SessionStart' ? { source: 'resume', session_id: 'B' }
        : name === 'classic.SubagentStart' ? { agent_id: 'target' }
        : name === 'prompt.attachment' ? ownEvent() : { text: 'native prompt' };
      const error = new Error('synthetic downstream failure');
      const next = nextResult(() => { if (kind === 'throw') throw error; return Promise.reject(error); });
      await assert.rejects(async () => h.invoke(name, event, next), value => value === error);
      assert.equal(next.calls.length, 1);
      if (name !== 'classic.UserPromptSubmit') assert.strictEqual(next.calls[0], event);
      assertNativeFlow(h);
    });
  }
}

test('overlapping root producers cannot replace a newer opening', async () => {
  const pending = deferred();
  const h = harness(async (_argv, options) => JSON.parse(options.stdin).session_id === 'A' ? pending.promise : ready(contextB));
  const a = h.open('A');
  await h.open('B', 'resume');
  pending.resolve(ready(contextA));
  assert.deepEqual((await a).additionalContext, [foreignA]);
  assert.equal((await attach(h, ownEvent())).text, contextB);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('overlapping children and a replacement never share a snapshot or restore invalidated state', async () => {
  const pending = deferred();
  let calls = 0;
  const h = harness(async (_argv, options) => {
    const id = JSON.parse(options.stdin).agent_id;
    if (id === 'target' && calls++ === 0) return pending.promise;
    return ready(id === 'target' ? contextB : contextA);
  });
  await h.open('A');
  const old = openChild(h, 'target');
  const sibling = (await openChild(h, 'sibling')).additionalContext.at(-1);
  const replacement = (await openChild(h, 'target')).additionalContext.at(-1);
  pending.resolve(ready(contextA));
  assert.deepEqual((await old).additionalContext, [foreignA]);
  assert.equal((await attach(h, childEvent('target', replacement))).text, contextB);
  assert.equal((await attach(h, childEvent('sibling', sibling))).text, contextA);
});

for (const boundary of ['clear', 'resume', 'compact', 'session.end']) {
  test(`${boundary} invalidates old owned payloads while retaining the native execution path`, async () => {
    const h = harness();
    await h.open('A');
    const child = (await openChild(h, 'target')).additionalContext.at(-1);
    const pending = deferred();
    const old = h.invoke('prompt.attachment', ownEvent(), nextResult(pending.promise));
    if (boundary === 'session.end') await h.invoke('session.end', {}, nextResult({}));
    else await h.open('B', boundary);
    pending.resolve({ text: marker + foreignA, retained: true });
    assert.deepEqual(await old, { text: foreignA, retained: true });
    const raw = { text: child, retained: true };
    assert.strictEqual(await attach(h, childEvent('target', child), raw), raw);
    if (boundary === 'session.end') assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
    else assert.equal((await attach(h, ownEvent())).text, contextB);
    assertNativeFlow(h);
  });
}

test('a replaced child attachment strips its captured marker without consuming the new child context', async () => {
  let context = contextA;
  const h = harness(async () => ready(context));
  await h.open('A');
  const old = (await openChild(h, 'target')).additionalContext.at(-1);
  const pending = deferred();
  const rendered = h.invoke('prompt.attachment', childEvent('target', old), nextResult(pending.promise));
  context = contextB;
  const fresh = (await openChild(h, 'target')).additionalContext.at(-1);
  pending.resolve({ text: old + foreignB });
  assert.equal((await rendered).text, foreignB);
  assert.equal((await attach(h, childEvent('target', fresh))).text, contextB);
  const unknown = { text: old, retained: true };
  assert.strictEqual(await attach(h, childEvent('target', old), unknown), unknown);
  assertNativeFlow(h);
});

for (const stage of ['producer', 'opening downstream', 'attachment before', 'attachment downstream']) {
  test(`an aborted signal at ${stage} prevents owned publication without controlling native work`, async () => {
    const controller = new AbortController();
    const h = harness(async () => { if (stage === 'producer') controller.abort(); return ready(contextA); });
    const event = { source: 'startup', session_id: 'A' };
    const real = { additionalContext: [foreignA], retained: true };
    const next = nextResult(() => { if (stage === 'opening downstream') controller.abort(); return real; }, controller.signal);
    const opened = await h.invoke('classic.SessionStart', event, next);
    assert.deepEqual(next.calls, [event]);
    if (stage.startsWith('attachment')) {
      assert.deepEqual(opened.additionalContext, [foreignA, marker]);
      if (stage === 'attachment before') controller.abort();
      const attachment = ownEvent();
      const actual = { text: marker + foreignB, retained: {} };
      const attachmentNext = nextResult(() => { controller.abort(); return actual; }, controller.signal);
      const result = await h.invoke('prompt.attachment', attachment, attachmentNext);
      assert.equal(result.text, foreignB);
      assert.strictEqual(result.retained, actual.retained);
      assert.deepEqual(attachmentNext.calls, [attachment]);
    } else assert.strictEqual(opened, real);
    assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
    assertNativeFlow(h);
  });
}

for (const source of ['startup', 'clear', 'resume', 'compact', 'unknown', undefined]) {
  test(`absent ${source ?? 'missing'} opening publishes no own context and preserves native work`, async () => {
    const h = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
    const event = { session_id: 'A' };
    if (source !== undefined) event.source = source;
    const real = { additionalContext: [foreignA], retained: true };
    assert.strictEqual(await h.invoke('classic.SessionStart', event, nextResult(real)), real);
    assert.equal((await h.receipt()).vivac_opening.status, 'absent');
    assert.equal(Object.hasOwn(h.producerCalls[0][1], 'cwd'), false);
    assertNativeFlow(h);
  });
}

for (const source of ['unknown', undefined]) {
  test(`a ready ${source ?? 'missing'} source is not published and a later valid opening recovers`, async () => {
    const h = harness();
    await h.open('A');
    const event = { session_id: 'A' };
    if (source !== undefined) event.source = source;
    const real = { additionalContext: [foreignA], retained: true };
    assert.strictEqual(await h.invoke('classic.SessionStart', event, nextResult(real)), real);
    assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
    assert.equal((await attach(h, ownEvent())).text, '');
    assertNativeFlow(h);
    await h.open('B', 'clear');
    assert.equal((await attach(h, ownEvent())).text, contextB);
  });
}

test('receipts describe preparation, replace forged metadata and never claim provider reception', async () => {
  const h = harness();
  const event = { text: 'native prompt', retained: {}, vivac_opening: { status: 'forged' } };
  const blocked = await h.receipt(event);
  assert.notStrictEqual(blocked, event);
  assert.strictEqual(blocked.retained, event.retained);
  assert.equal(event.vivac_opening.status, 'forged');
  assert.deepEqual(blocked.vivac_opening, { protocol, status: 'blocked', generation: 0 });
  await h.open('A');
  const prepared = await h.receipt(event);
  assert.deepEqual(prepared.vivac_opening, { protocol, status: 'ready', generation: 1 });
  assert.equal(Object.hasOwn(prepared.vivac_opening, 'received'), false);
  const controller = new AbortController();
  controller.abort();
  assert.equal((await h.receipt(event, controller.signal)).vivac_opening.status, 'blocked');
  assertNativeFlow(h);
});
