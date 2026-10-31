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
const continuationBlockedMessage = 'Vivac cannot guarantee current context for SendMessage continuations. SendMessage is disabled while Vivac manages this session. Start a new subagent with Agent instead.';

function toolHook(h) {
  const hook = h.hooks.get('classic.PreToolUse');
  assert.ok(hook, 'SendMessage continuation guard must be registered');
  assert.equal(hook.catcher, null, 'downstream failures must not retry next');
  return hook.handler;
}

async function assertSendDenied(h, event = { tool: 'SendMessage' }) {
  const next = nextResult({ native: true });
  const inaccessibleApi = new Proxy({}, { get() { assert.fail('denial must not access the SDK'); } });
  assert.deepEqual(await toolHook(h)(inaccessibleApi, event, next), { deny: continuationBlockedMessage });
  assert.deepEqual(next.calls, []);
}

function spawnHook(h) {
  const hook = h.hooks.get('agent.spawn');
  assert.ok(hook, 'native continuation guard must be registered');
  assert.equal(hook.catcher, null, 'downstream failures must not retry next');
  return hook.handler;
}

function assertSpawnDenied(h, event) {
  const next = nextResult({ native: true });
  const inaccessibleApi = new Proxy({}, { get() { assert.fail('denial must not access the SDK'); } });
  assert.deepEqual(spawnHook(h)(inaccessibleApi, event, next), { deny: continuationBlockedMessage });
  assert.deepEqual(next.calls, []);
}

test('managed native continuation is denied regardless of resume value without reading it', async () => {
  const h = harness();
  await h.open('A');
  const before = await h.receipt();
  for (const resume of [{ agentId: 'known-child' }, undefined, null]) {
    assertSpawnDenied(h, { resume });
  }
  const event = {};
  Object.defineProperty(event, 'resume', { get() { assert.fail('resume content must not be read'); } });
  assertSpawnDenied(h, event);
  assert.deepEqual(await h.receipt(), before);
  assert.deepEqual(h.abortCalls, []);
  assert.deepEqual(h.caught, []);
});

test('native continuation is denied before SessionStart, during preparation and after blocking', async () => {
  const pending = deferred();
  const h = harness(() => pending.promise);
  const event = { resume: { agentId: 'known-child' } };
  assertSpawnDenied(h, event);
  const opening = h.open('A');
  assertSpawnDenied(h, event);
  pending.resolve({ exitCode: 1, stdout: '' });
  await opening;
  assertSpawnDenied(h, event);
});

test('managed initial spawn preserves exact event and result including inherited resume', async () => {
  const h = harness();
  await h.open('A');
  const inherited = Object.create({ resume: { agentId: 'inherited-child' } });
  for (const event of [{ prompt: 'synthetic work' }, inherited]) {
    const result = { native: true };
    const calls = [];
    const next = value => { calls.push(value); return result; };
    assert.strictEqual(spawnHook(h)(h.$, event, next), result);
    assert.deepEqual(calls, [event]);
    assert.strictEqual(calls[0], event);
  }
});

test('absent native initial and resumed spawn preserve exact event, promise and result', async () => {
  const h = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
  await h.open('A');
  for (const event of [{ prompt: 'synthetic work' }, { resume: { agentId: 'native-child' } }]) {
    const result = { native: true };
    const promise = Promise.resolve(result);
    const calls = [];
    const next = value => { calls.push(value); return promise; };
    const returned = spawnHook(h)(h.$, event, next);
    assert.strictEqual(returned, promise);
    assert.strictEqual(await returned, result);
    assert.deepEqual(calls, [event]);
    assert.strictEqual(calls[0], event);
  }
});

test('managed SendMessage rejects every target and forged metadata without reading arguments', async () => {
  let producerCalls = 0;
  const h = harness(async () => { producerCalls++; return ready(contextA); });
  await h.open('A');
  await openChild(h, 'known-child');
  const before = await h.receipt();
  for (const target of ['known-child', 'unknown-child', 'teammate', 'other-session', 'agent-prefix', '', undefined]) {
    const event = { tool: 'SendMessage', recipient: target, agentId: 'forged-child',
      model: 'forged-model', session_id: 'other-session' };
    Object.defineProperty(event, 'arguments', { get() { assert.fail('SYNTHETIC_SECRET_ARGUMENT'); } });
    Object.defineProperty(event, 'input', { get() { assert.fail('SYNTHETIC_SECRET_INPUT'); } });
    await assertSendDenied(h, event);
  }
  assert.deepEqual(await h.receipt(), before);
  assert.equal(producerCalls, 2);
  assert.deepEqual(h.abortCalls, []);
  assert.deepEqual(h.caught, []);
});

test('managed denial does not expose argument content', async () => {
  const h = harness();
  await h.open('A');
  const secret = 'SYNTHETIC_SECRET_NEVER_RETURN_8ae4';
  const next = nextResult({ native: true });
  const result = await toolHook(h)(h.$, { tool: 'SendMessage', input: { recipient: secret, message: secret } }, next);
  assert.deepEqual(result, { deny: continuationBlockedMessage });
  assert.equal(JSON.stringify(result).includes(secret), false);
  assert.deepEqual(next.calls, []);
});

test('absent SendMessage preserves exact event, promise and result with one next call', async () => {
  const h = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
  await h.open('A');
  const event = { tool: 'SendMessage', input: { recipient: 'native-target', message: 'native-message' } };
  const result = { native: true };
  const promise = Promise.resolve(result);
  const calls = [];
  const next = value => { calls.push(value); return promise; };
  const returned = toolHook(h)(h.$, event, next);
  assert.strictEqual(returned, promise);
  assert.strictEqual(await returned, result);
  assert.deepEqual(calls, [event]);
  assert.strictEqual(calls[0], event);
});

for (const failureKind of ['throw', 'reject']) {
  test(`absent SendMessage propagates downstream ${failureKind} once`, async () => {
    const h = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
    await h.open('A');
    const event = { tool: 'SendMessage' };
    const failure = new Error('synthetic downstream failure');
    const calls = [];
    const next = value => {
      calls.push(value);
      if (failureKind === 'throw') throw failure;
      return Promise.reject(failure);
    };
    const handler = toolHook(h);
    if (failureKind === 'throw') assert.throws(() => handler(h.$, event, next), error => error === failure);
    else await assert.rejects(handler(h.$, event, next), error => error === failure);
    assert.deepEqual(calls, [event]);
  });
}

for (const tool of ['Agent', 'Read', 'sendmessage', undefined]) {
  test(`managed ${tool ?? 'missing tool'} passes through exactly once`, async () => {
    const h = harness();
    await h.open('A');
    const event = { tool, input: { synthetic: true } };
    const result = { native: true };
    const calls = [];
    const returned = toolHook(h)(h.$, event, value => { calls.push(value); return result; });
    assert.strictEqual(returned, result);
    assert.deepEqual(calls, [event]);
    assert.strictEqual(calls[0], event);
  });
}

test('SendMessage is denied before SessionStart, during preparation and after blocking', async () => {
  const pending = deferred();
  const h = harness(() => pending.promise);
  await assertSendDenied(h);
  const opening = h.open('A');
  await assertSendDenied(h);
  pending.resolve({ exitCode: 1, stdout: '' });
  await opening;
  await assertSendDenied(h);
  await h.invoke('session.end', {}, nextResult({}));
  await assertSendDenied(h);
});

for (const source of ['clear', 'resume', 'compact']) {
  test(`SendMessage follows managed to absent to managed generations through ${source}`, async () => {
    let absent = false;
    const h = harness(async () => absent
      ? { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) } : ready(contextA));
    await h.open('A');
    await assertSendDenied(h);
    absent = true;
    await h.open('B', source);
    const event = { tool: 'SendMessage' };
    const result = { native: true };
    const next = nextResult(result);
    assert.strictEqual(await toolHook(h)(h.$, event, next), result);
    assert.deepEqual(next.calls, [event]);
    absent = false;
    await h.open('C', source);
    await assertSendDenied(h);
  });
}

test('denial preserves root receipt and permits fresh Agent context and its own child turn', async () => {
  const full = 'synthetic-child-context:'.repeat(2000);
  const calls = [];
  const h = harness(async (...args) => { calls.push(args); return ready(calls.length === 1 ? contextA : full); });
  await h.open('A');
  const before = await h.receipt();
  await assertSendDenied(h);
  const event = { tool: 'Agent', input: { prompt: 'synthetic work' } };
  const result = { nativeAgent: true };
  const next = nextResult(result);
  assert.strictEqual(await toolHook(h)(h.$, event, next), result);
  assert.deepEqual(next.calls, [event]);
  const opened = await openChild(h, 'fresh-child');
  assert.equal(calls.length, 2);
  assert.deepEqual(JSON.parse(calls[1][1].stdin), { hook_event_name: 'SubagentStart', agent_id: 'fresh-child' });
  const reference = opened.additionalContext.at(-1);
  assert.equal((await attachChild(h, 'fresh-child', reference)).text, full);
  assert.equal((await h.turn('fresh-child-turn', 'fresh-child')).kind, 'synthetic-turn');
  assert.deepEqual(await h.receipt(), before);
  assert.deepEqual(h.abortCalls, []);
});

test('child sender readiness cannot override the root SendMessage policy', async () => {
  let rootAbsent = false;
  let childAbsent = true;
  const h = harness(async (_argv, options) => {
    const absent = JSON.parse(options.stdin).agent_id ? childAbsent : rootAbsent;
    return absent ? { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) } : ready(contextA);
  });
  await h.open('A');
  await openChild(h, 'absent-child');
  await assertSendDenied(h, { tool: 'SendMessage', agentId: 'absent-child' });
  rootAbsent = true;
  childAbsent = false;
  await h.open('B', 'clear');
  await openChild(h, 'ready-child');
  const event = { tool: 'SendMessage', agentId: 'ready-child' };
  const result = { native: true };
  const next = nextResult(result);
  assert.strictEqual(await toolHook(h)(h.$, event, next), result);
  assert.deepEqual(next.calls, [event]);
});

async function openChild(h, agent_id, next = nextResult({ additionalContext: [foreignA] })) {
  return h.invoke('classic.SubagentStart', { agent_id, cwd: 'child-fixture' }, next);
}

function childEvent(agentId, text) {
  return { agentId, origin: { kind: 'hook', event: 'SubagentStart' }, text };
}

async function attachChild(h, id, reference, text = reference) {
  return h.invoke('prompt.attachment', childEvent(id, reference), nextResult({ text }));
}

for (const invalid of [null, 7, false, {}, [], undefined]) {
  for (const target of ['root', 'child']) {
    test(`${target} mixed additionalContext rejects ${String(invalid)} without publishing readiness`, async () => {
      const h = harness();
      const foreign = { additionalContext: [foreignA, invalid], retained: true };
      if (target === 'root') {
        assert.strictEqual(await h.open('A', 'startup', foreign), foreign);
        assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
        assert.equal((await h.turn('root-turn')).kind, 'stop');
      } else {
        await h.open('A');
        await openChild(h, 'sibling');
        assert.strictEqual(await openChild(h, 'target', nextResult(foreign)), foreign);
        assert.equal((await h.turn('target-turn', 'target')).kind, 'stop');
        assert.equal((await h.turn('sibling-turn', 'sibling')).kind, 'synthetic-turn');
        assert.equal((await h.receipt()).vivac_opening.status, 'ready');
      }
    });
  }
}

for (const duplicate of ['event', 'result', 'both']) {
  test(`child duplicate marker in ${duplicate} blocks and strips only its reference`, async () => {
    const h = harness();
    await h.open('A');
    const a = (await openChild(h, 'a')).additionalContext.at(-1);
    const b = (await openChild(h, 'b')).additionalContext.at(-1);
    await h.turn('root'); await h.turn('a-turn', 'a'); await h.turn('b-turn', 'b');
    const event = childEvent('a', duplicate === 'result' ? a : a + foreignA + a);
    const text = foreignA + a + (duplicate === 'event' ? '' : a) + b + marker + foreignB;
    const fields = { retained: true };
    const next = nextResult({ text, fields });
    const result = await h.invoke('prompt.attachment', event, next);
    assert.equal(result.text, foreignA + b + marker + foreignB);
    assert.strictEqual(result.fields, fields);
    assert.equal(result.text.includes(contextA), false);
    assert.deepEqual(next.calls, [event]);
    assert.deepEqual(h.abortCalls, ['a-turn']);
    assert.equal((await h.turn('blocked-a', 'a')).kind, 'stop');
    assert.equal((await attachChild(h, 'b', b)).text, contextA);
    assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  });
}

test('invalid child identifiers never launch a producer or publish a root reference', async () => {
  let calls = 0;
  const h = harness(async () => { calls++; return ready(contextA); });
  await h.open('A');
  for (const id of [undefined, null, '', 7, 'é', 'a.b', 'x'.repeat(257)]) {
    const foreign = { additionalContext: [foreignA] };
    assert.strictEqual(await openChild(h, id, nextResult(foreign)), foreign);
  }
  assert.equal(calls, 1);
  const maxId = 'x'.repeat(256);
  assert.equal((await openChild(h, maxId)).additionalContext.length, 2);
  assert.equal(calls, 2);
});

test('child attachment selection requires the exact native identity, hook and own marker', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  const b = (await openChild(h, 'b')).additionalContext.at(-1);
  for (const event of [
    childEvent(undefined, a), childEvent('unknown', a), childEvent(null, a), childEvent('b', a),
    { ...childEvent('a', a), origin: { kind: 'hook', event: 'SessionStart' } },
    { ...childEvent('a', a), origin: { kind: 'file', event: 'SubagentStart' } },
    { ...ownEvent(), agentId: 'a' },
  ]) {
    const result = { text: event.text, retained: true };
    const next = nextResult(result);
    assert.strictEqual(await h.invoke('prompt.attachment', event, next), result);
    assert.deepEqual(next.calls, [event]);
  }
  assert.equal((await attachChild(h, 'a', a)).text, contextA);
  assert.equal((await attachChild(h, 'b', b)).text, contextA);
});

test('overlapping child producers finish independently without sharing context', async () => {
  const a = deferred(); const b = deferred();
  const h = harness(async (_argv, options) => {
    const id = JSON.parse(options.stdin).agent_id;
    return id === 'a' ? a.promise : id === 'b' ? b.promise : ready(contextA);
  });
  await h.open('A');
  const first = openChild(h, 'a'); const second = openChild(h, 'b');
  b.resolve(ready(contextB));
  const refB = (await second).additionalContext.at(-1);
  assert.equal((await attachChild(h, 'b', refB)).text, contextB);
  a.resolve(ready(contextA));
  const refA = (await first).additionalContext.at(-1);
  assert.equal((await attachChild(h, 'a', refA)).text, contextA);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('compact retains the in-flight root turn as the abort target for a corrupt attachment', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('root-turn');
  await h.open('B', 'compact');
  const next = nextResult({ text: foreignA, retained: true });
  assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), next), { text: foreignA, retained: true });
  assert.deepEqual(h.abortCalls, ['root-turn']);
  assert.equal(next.calls.length, 1);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

for (const boundary of ['clear', 'replacement']) {
  test(`child attachment across ${boundary} aborts its old live turn and preserves new recipients`, async () => {
    const h = harness();
    await h.open('A');
    const old = (await openChild(h, 'a')).additionalContext.at(-1);
    await h.turn('old-turn', 'a');
    const pending = deferred(); const entered = deferred();
    const event = childEvent('a', old);
    const next = nextResult(() => { entered.resolve(); return pending.promise; });
    const attachment = h.invoke('prompt.attachment', event, next);
    await entered.promise;
    if (boundary === 'clear') await h.open('B', 'clear');
    const fresh = (await openChild(h, 'a')).additionalContext.at(-1);
    const sibling = (await openChild(h, 'b')).additionalContext.at(-1);
    await h.turn('fresh-turn', 'a'); await h.turn('sibling-turn', 'b'); await h.turn('root-turn');
    pending.resolve({ text: foreignA + old + fresh + sibling + marker, retained: true });
    assert.deepEqual(await attachment, { text: foreignA + fresh + sibling + marker, retained: true });
    assert.deepEqual(h.abortCalls, ['old-turn']);
    assert.deepEqual(next.calls, [event]);
    assert.equal((await attachChild(h, 'a', fresh)).text, contextA);
    assert.equal((await attachChild(h, 'b', sibling)).text, contextA);
    assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  });
}

test('replacing a child during attachment recovery strips only its captured marker', async () => {
  const h = harness();
  await h.open('A');
  const old = (await openChild(h, 'a')).additionalContext.at(-1);
  await h.turn('old-turn', 'a');
  const pending = deferred(); const entered = deferred();
  const controller = new AbortController();
  const event = childEvent('a', old);
  const next = nextResult(() => { entered.resolve(); return pending.promise; }, controller.signal);
  const result = h.invoke('prompt.attachment', event, next);
  await entered.promise;
  const fresh = (await openChild(h, 'a')).additionalContext.at(-1);
  await h.turn('fresh-turn', 'a');
  controller.abort();
  pending.resolve({ text: foreignA + old + fresh + marker, retained: true });
  assert.deepEqual(await result, { text: foreignA + fresh + marker, retained: true });
  assert.equal(next.calls.length, 1);
  assert.deepEqual(h.abortCalls, ['old-turn']);
  assert.equal((await attachChild(h, 'a', fresh)).text, contextA);
  assert.equal((await h.turn('still-fresh', 'a')).kind, 'synthetic-turn');
});

for (const boundary of ['session.end', 'classic.SessionStart', 'classic.SubagentStart']) {
  test(`${boundary} prevents a late child producer from republishing an old opening`, async () => {
    const pending = deferred();
    let calls = 0;
    const h = harness(async () => ++calls === 2 ? pending.promise : ready(contextB));
    await h.open('A');
    const old = openChild(h, 'a');
    let fresh;
    if (boundary === 'session.end') await h.invoke(boundary, {}, nextResult({}));
    else if (boundary === 'classic.SessionStart') await h.open('B', 'resume');
    else fresh = (await openChild(h, 'a')).additionalContext.at(-1);
    pending.resolve(ready(contextA));
    assert.deepEqual((await old).additionalContext, [foreignA]);
    if (fresh) assert.equal((await attachChild(h, 'a', fresh)).text, contextB);
    else assert.equal((await h.turn('old-child', 'a')).kind, 'stop');
  });
}

test('a disappearing child marker aborts only the captured child turn and preserves next once', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  await openChild(h, 'b');
  await h.turn('root'); await h.turn('child-a', 'a'); await h.turn('child-b', 'b');
  const result = { text: foreignA + marker, retained: true };
  const next = nextResult(result);
  assert.deepEqual(await h.invoke('prompt.attachment', childEvent('a', a), next), result);
  assert.equal(next.calls.length, 1);
  assert.deepEqual(h.abortCalls, ['child-a']);
  assert.equal((await h.turn('a-blocked', 'a')).kind, 'stop');
  assert.equal((await h.turn('b-ready', 'b')).kind, 'synthetic-turn');
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

for (const failure of ['exception', 'timeout', 'signal during producer', 'signal during next']) {
  test(`child ${failure} stays blocked without invalidating root`, async () => {
    const controller = new AbortController();
    const h = harness(async (_argv, options) => {
      if (!JSON.parse(options.stdin).agent_id) return ready(contextA);
      if (failure === 'exception' || failure === 'timeout') throw new Error(failure);
      if (failure === 'signal during producer') controller.abort();
      return ready(contextB);
    });
    await h.open('A');
    const foreign = { additionalContext: [foreignA] };
    const next = nextResult(() => {
      if (failure === 'signal during next') controller.abort();
      return foreign;
    }, controller.signal);
    assert.strictEqual(await openChild(h, 'a', next), foreign);
    assert.equal(next.calls.length, 1);
    assert.equal((await h.turn('a-blocked', 'a')).kind, 'stop');
    assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  });
}

test('absent child opens without publication or parent context and permits only its own turn', async () => {
  const h = harness(async (_argv, options) => JSON.parse(options.stdin).agent_id
    ? { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) } : ready(contextA));
  await h.open('A');
  assert.deepEqual((await openChild(h, 'a')).additionalContext, [foreignA]);
  assert.equal((await h.turn('absent-child', 'a')).kind, 'synthetic-turn');
  assert.equal((await h.turn('unknown', 'b')).kind, 'stop');
  assert.equal((await attachChild(h, 'a', marker)).text, marker);
});

test('aborted child attachment strips only its marker and preserves root submission guards', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  await h.turn('a-turn', 'a');
  const controller = new AbortController();
  const result = { text: a + marker + foreignA, retained: true };
  const next = nextResult(() => { controller.abort(); return result; }, controller.signal);
  assert.deepEqual(await h.invoke('prompt.attachment', childEvent('a', a), next),
    { text: marker + foreignA, retained: true });
  assert.deepEqual(h.abortCalls, ['a-turn']);
  assert.equal(next.calls.length, 1);
  const prompt = { text: 'work' };
  assert.strictEqual(await h.invoke('prompt.submit', prompt, nextResult(value => value)), prompt);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('child downstream rejection recovers once and aborts only its captured turn', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  const b = (await openChild(h, 'b')).additionalContext.at(-1);
  await h.turn('root'); await h.turn('a-turn', 'a'); await h.turn('b-turn', 'b');
  const failure = new Error('synthetic child downstream rejection');
  const next = nextResult((_event, call) => {
    if (call === 1) throw failure;
    assert.equal(call, 2);
    return { text: a + b + marker, retained: true };
  });
  assert.deepEqual(await h.invoke('prompt.attachment', childEvent('a', a), next),
    { text: b + marker, retained: true });
  assert.strictEqual(h.caught[0].error, failure);
  assert.equal(next.calls.length, 2);
  assert.deepEqual(h.abortCalls, ['a-turn']);
  assert.equal((await attachChild(h, 'b', b)).text, contextA);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('child context at exactly 60000 UTF8 bytes expands intact without inferred cwd', async () => {
  const full = '🧭'.repeat(15000);
  const calls = [];
  const h = harness(async (...args) => { calls.push(args); return { ...ready(full), isStdoutTruncated: false, isStderrTruncated: false }; });
  const event = { agent_id: 'max-child' };
  const opened = await h.invoke('classic.SubagentStart', event, nextResult({}));
  const reference = opened.additionalContext[0];
  assert.equal(Object.hasOwn(calls[0][1], 'cwd'), false);
  assert.equal((await attachChild(h, 'max-child', reference, foreignA + reference + foreignB)).text,
    foreignA + full + foreignB);
  assert.equal((await h.turn('ready-child', 'max-child')).kind, 'synthetic-turn');
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

test('child prepares fresh context after root A and preserves complete foreign fields', async () => {
  const calls = [];
  const full = 'prefix:' + '🧭'.repeat(14000) + ':suffix';
  const h = harness(async (...args) => { calls.push(args); return ready(calls.length === 1 ? contextA : full); });
  await h.open('A');
  const opened = await openChild(h, 'child_A-1');
  const reference = opened.additionalContext.at(-1);
  assert.notEqual(reference, marker);
  assert.equal(reference.includes('child_A-1'), false);
  assert.deepEqual(calls[1], [['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], {
    stdin: JSON.stringify({ hook_event_name: 'SubagentStart', agent_id: 'child_A-1' }), timeoutMs: 5000, cwd: 'child-fixture',
  }]);
  const fields = { retained: true };
  const next = nextResult({ text: foreignA + reference + foreignB, fields });
  const result = await h.invoke('prompt.attachment', childEvent('child_A-1', reference), next);
  assert.equal(result.text, foreignA + full + foreignB);
  assert.strictEqual(result.fields, fields);
  assert.equal(next.calls.length, 1);
  assert.equal((await h.turn('child-turn', 'child_A-1')).kind, 'synthetic-turn');
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('unknown and malformed child turns never use root readiness', async () => {
  const h = harness();
  await h.open('A');
  for (const id of ['unknown', '', 'x'.repeat(257), 'é', null, 7])
    assert.equal((await h.turn('blocked-' + String(id), id)).kind, 'stop');
  assert.equal((await h.turn('root')).kind, 'synthetic-turn');
});

test('child duplicate marker blocks after one expansion and only aborts its own active turn', async () => {
  const h = harness();
  await h.open('A');
  const a = (await openChild(h, 'a')).additionalContext.at(-1);
  const b = (await openChild(h, 'b')).additionalContext.at(-1);
  assert.notEqual(a, b);
  assert.equal((await attachChild(h, 'a', a)).text, contextA);
  await h.turn('root'); await h.turn('a-turn', 'a'); await h.turn('b-turn', 'b');
  assert.equal((await attachChild(h, 'a', a, a + b + marker)).text, b + marker);
  assert.deepEqual(h.abortCalls, ['a-turn']);
  assert.equal((await h.turn('a-blocked', 'a')).kind, 'stop');
  assert.equal((await attachChild(h, 'b', b)).text, contextA);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

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
  const next = async (event) => {
    calls.push(event);
    return typeof produce === 'function' ? produce(event, calls.length) : produce;
  };
  next.signal = signal;
  next.calls = calls;
  return next;
}

function harness(processRun = async (_argv, options) => ready(JSON.parse(options.stdin).session_id === 'B' ? contextB : contextA)) {
  const hooks = new Map();
  const caught = [];
  const abortCalls = [];
  const turns = new Map();
  const $ = { process: { run: processRun }, turn: { abort: async ({ turnId }) => {
    abortCalls.push(turnId);
    if (turns.has(turnId)) await returnTurn(turnId);
  } } };
  register((name, handler) => {
    const hook = { handler, catcher: null };
    hooks.set(name, hook);
    return { catch(catcher) { hook.catcher = catcher; } };
  });
  async function invoke(name, event, next) {
    const hook = hooks.get(name);
    try {
      return await hook.handler($, event, next);
    } catch (error) {
      caught.push({ name, event, error });
      return await hook.catcher($, event, next);
    }
  }
  async function turn(turnId, agentId) {
    const next = async function* () { yield { kind: 'synthetic-turn' }; return { continued: true }; };
    next.signal = new AbortController().signal;
    const iterator = hooks.get('turn.step').handler($, { turnId, index: 0, ...(agentId === undefined ? {} : { agentId }) }, next);
    turns.set(turnId, iterator);
    const first = await iterator.next();
    if (first.done) turns.delete(turnId);
    return first.value;
  }
  async function finishTurn(turnId) {
    const result = await turns.get(turnId).next();
    if (result.done) turns.delete(turnId);
    return result;
  }
  async function returnTurn(turnId) {
    const result = await turns.get(turnId).return();
    if (result.done) turns.delete(turnId);
    return result;
  }
  async function rejectTurn(turnId, error) {
    try { return await turns.get(turnId).throw(error); }
    finally { turns.delete(turnId); }
  }
  async function open(id, source = 'startup', value = { additionalContext: ['foreign opening'], nativeField: id }) {
    return invoke('classic.SessionStart', { session_id: id, source, cwd: 'synthetic-fixture' }, nextResult(value));
  }
  async function receipt() {
    return invoke('classic.UserPromptSubmit', { text: 'synthetic prompt' }, nextResult((event) => event));
  }
  return { $, hooks, caught, abortCalls, invoke, turn, finishTurn, returnTurn, rejectTurn, open, receipt };
}

function ownEvent(text = marker) {
  return { origin: { kind: 'hook', event: 'SessionStart' }, text, fixtureField: 'original-event' };
}

async function assertFreshB(h) {
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  const result = { text: `${marker}${foreignB}`, foreignField: { retained: true } };
  const next = nextResult(result);
  const attached = await h.invoke('prompt.attachment', ownEvent(), next);
  assert.equal(attached.text, `${contextB}${foreignB}`);
  assert.strictEqual(attached.foreignField, result.foreignField);
  assert.equal(next.calls.length, 1);
}

test('stale A recovers its returned result locally while fresh B and turn B survive', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const entered = deferred();
  const pending = deferred();
  const event = ownEvent();
  const controller = new AbortController();
  const next = nextResult((_event, call) => {
    if (call > 1) throw new Error('Replay attempted after a real result');
    entered.resolve();
    return pending.promise;
  }, controller.signal);
  const attachment = h.invoke('prompt.attachment', event, next);
  await entered.promise;
  await h.open('B', 'resume');
  await h.turn('turn-B');
  controller.abort();
  const real = { text: `before:${marker}${foreignA}${marker}:after`, foreignField: { retained: true }, foreignMarkerField: marker };
  pending.resolve(real);
  const recovered = await attachment;
  assert.equal(recovered.text, `before:${foreignA}:after`);
  assert.strictEqual(recovered.foreignField, real.foreignField);
  assert.equal(recovered.foreignMarkerField, marker);
  assert.equal(real.text, `before:${marker}${foreignA}${marker}:after`);
  assert.deepEqual(next.calls, [event]);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, []);
  await assertFreshB(h);
});

test('current owned disappearance uses actual foreign result, invalidates and aborts only its turn', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const event = ownEvent();
  const real = { text: foreignA, foreignField: { retained: true } };
  const next = nextResult((_event, call) => {
    if (call > 1) throw new Error('Replay attempted after a real result');
    return real;
  });
  const recovered = await h.invoke('prompt.attachment', event, next);
  assert.equal(recovered.text, foreignA);
  assert.strictEqual(recovered.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event]);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

test('current owned attachment without an active turn recovers without an abort', async () => {
  const h = harness();
  await h.open('A');
  const real = { text: foreignA, retained: true };
  const next = nextResult(real);
  assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), next), real);
  assert.equal(next.calls.length, 1);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

test('next rejection rethrows the same error and native catch invalidates before awaiting replay', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const rejection = new Error('synthetic downstream failure');
  const replayEntered = deferred();
  const replay = deferred();
  const event = ownEvent();
  const next = nextResult((_event, call) => {
    if (call === 1) throw rejection;
    assert.equal(call, 2);
    replayEntered.resolve();
    return replay.promise;
  });
  const attachment = h.invoke('prompt.attachment', event, next);
  await replayEntered.promise;
  assert.strictEqual(h.caught[0].error, rejection);
  assert.strictEqual(h.caught[0].event, event);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
  await h.open('B', 'clear');
  await h.turn('turn-B');
  const real = { text: `${marker}${foreignA}`, foreignField: { retained: true } };
  replay.resolve(real);
  const recovered = await attachment;
  assert.equal(recovered.text, foreignA);
  assert.strictEqual(recovered.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event, event]);
  assert.deepEqual(h.abortCalls, []);
  await assertFreshB(h);
});

test('native fallback for next rejection aborts the still-active selected turn and preserves the replay result', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const rejection = new Error('synthetic next failure');
  const real = { text: `${foreignA}${marker}`, foreignField: { retained: true } };
  const event = ownEvent();
  const next = nextResult((_event, call) => { if (call === 1) throw rejection; return real; });
  const recovered = await h.invoke('prompt.attachment', event, next);
  assert.strictEqual(h.caught[0].error, rejection);
  assert.equal(recovered.text, foreignA);
  assert.strictEqual(recovered.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event, event]);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

test('an unclassified post-result error reaches the native catch unchanged and does not replay', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const failure = new Error('synthetic foreign accessor failure');
  const event = ownEvent('foreign event without an owned marker');
  const real = { get text() { throw failure; }, foreignField: true };
  const next = nextResult(real);
  await assert.rejects(h.invoke('prompt.attachment', event, next), /Synthetic attachment failure was not classified/);
  assert.strictEqual(h.caught[0].error, failure);
  assert.strictEqual(h.caught[0].event, event);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('an unclassified next rejection reaches the native catch unchanged', async () => {
  const h = harness();
  await h.open('A');
  const failure = new Error('synthetic unclassified next failure');
  const event = ownEvent('foreign event without an owned marker');
  const next = nextResult(() => { throw failure; });
  await assert.rejects(h.invoke('prompt.attachment', event, next), /Synthetic attachment failure was not classified/);
  assert.strictEqual(h.caught[0].error, failure);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('a marker first classified from the real result is recovered locally if the opening is blocked', async () => {
  const h = harness();
  const real = { text: `${marker}${foreignA}`, retained: true };
  const next = nextResult(real);
  const recovered = await h.invoke('prompt.attachment', ownEvent('foreign event text'), next);
  assert.deepEqual(recovered, { text: foreignA, retained: true });
  assert.equal(next.calls.length, 1);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, []);
});

test('a valid current reference expands only text and preserves foreign result fields', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const real = { text: `${marker}${foreignA}${marker}`, foreignField: { retained: true }, foreignMarkerField: marker };
  const next = nextResult(real);
  const recovered = await h.invoke('prompt.attachment', ownEvent(), next);
  assert.equal(recovered.text, `${contextA}${foreignA}${contextA}`);
  assert.strictEqual(recovered.foreignField, real.foreignField);
  assert.equal(recovered.foreignMarkerField, marker);
  assert.equal(real.text, `${marker}${foreignA}${marker}`);
  assert.equal(next.calls.length, 1);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, []);
});

test('an owned null-text object retains its exact identity and fields during local recovery', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const real = { text: null, foreignField: { retained: true } };
  const next = nextResult(real);
  assert.strictEqual(await h.invoke('prompt.attachment', ownEvent(), next), real);
  assert.equal(next.calls.length, 1);
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

for (const [label, real] of [['null', null], ['undefined', undefined], ['number', 7], ['boolean', false], ['string', 'foreign scalar'], ['array', []], ['missing text', { foreignField: true }], ['non-string text', { text: 7, foreignField: true }]]) {
  test(`owned malformed ${label} uses baseline null normalization without a replay`, async () => {
    const h = harness();
    await h.open('A');
    await h.turn('turn-A');
    const next = nextResult(real);
    assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), next), { text: null });
    assert.equal(next.calls.length, 1);
    assert.equal(h.caught.length, 0);
    assert.deepEqual(h.abortCalls, ['turn-A']);
    assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
  });
}

test('foreign origins and foreign eligible attachments retain actual results unchanged', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  for (const [event, real] of [
    [{ origin: { kind: 'file' }, text: marker }, { text: `${marker}${foreignA}`, foreignField: true }],
    [ownEvent(foreignA), { text: foreignA, foreignField: true }],
    [ownEvent(foreignA), null],
    [ownEvent(foreignA), undefined],
    [ownEvent(foreignA), 7],
    [ownEvent(foreignA), []],
    [ownEvent(foreignA), { text: null, foreignField: true }],
  ]) {
    const next = nextResult(real);
    assert.strictEqual(await h.invoke('prompt.attachment', event, next), real);
    assert.deepEqual(next.calls, [event]);
  }
  assert.equal(h.caught.length, 0);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

test('a late producer A does not replace fresh opening B', async () => {
  const pending = deferred();
  const entered = deferred();
  const h = harness(async (_argv, options) => {
    if (JSON.parse(options.stdin).session_id === 'A') { entered.resolve(); return pending.promise; }
    return ready(contextB);
  });
  const openingA = h.open('A');
  await entered.promise;
  const openingB = await h.open('B', 'resume');
  pending.resolve(ready(contextA));
  await openingA;
  assert.deepEqual(openingB.additionalContext, ['foreign opening', marker]);
  assert.equal(openingB.nativeField, 'B');
  await h.turn('turn-B');
  await assertFreshB(h);
  assert.deepEqual(h.abortCalls, []);
});

test('session end invalidates the opening and clears the active abort target', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const event = ownEvent();
  const pending = deferred();
  const entered = deferred();
  const next = nextResult(() => { entered.resolve(); return pending.promise; });
  const attachment = h.invoke('prompt.attachment', event, next);
  await entered.promise;
  const end = { reason: 'synthetic end' };
  assert.strictEqual(await h.invoke('session.end', end, nextResult((value) => value)), end);
  pending.resolve({ text: `${marker}${foreignA}`, retained: true });
  assert.deepEqual(await attachment, { text: foreignA, retained: true });
  assert.equal(next.calls.length, 1);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
});

test('opening ready, absent and blocked controls preserve submission and turn guards', async () => {
  for (const source of ['startup', 'clear', 'resume', 'compact']) {
    const h = harness();
    const opening = await h.open('A', source);
    assert.deepEqual(opening.additionalContext, ['foreign opening', marker]);
    assert.equal(opening.nativeField, 'A');
    const event = { text: 'synthetic prompt' };
    assert.strictEqual(await h.invoke('prompt.submit', event, nextResult((value) => value)), event);
    assert.equal((await h.turn('turn-A')).kind, 'synthetic-turn');
  }
  const absent = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
  assert.deepEqual((await absent.open('A')).additionalContext, ['foreign opening']);
  assert.equal((await absent.receipt()).vivac_opening.status, 'absent');
  const event = { text: 'synthetic prompt' };
  assert.strictEqual(await absent.invoke('prompt.submit', event, nextResult((value) => value)), event);
  assert.equal((await absent.turn('absent-turn')).kind, 'synthetic-turn');
  for (const produced of [{ exitCode: 1, stdout: '' }, { exitCode: 0, stdout: '{' }, ready(''), ready('x'.repeat(60001))]) {
    const blocked = harness(async () => produced);
    await blocked.open('A');
    assert.equal((await blocked.receipt()).vivac_opening.status, 'blocked');
    assert.equal(typeof (await blocked.invoke('prompt.submit', event, nextResult((value) => value))).drop, 'string');
    assert.equal((await blocked.turn('blocked-turn')).kind, 'stop');
    for (const command of ['/clear', '/resume synthetic']) {
      const escape = { text: command };
      assert.strictEqual(await blocked.invoke('prompt.submit', escape, nextResult((value) => value)), escape);
    }
  }
});

test('a rejected owned abort after a real result propagates its exact error without replay or disturbing B', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const failure = new Error('synthetic abort rejection');
  const entered = deferred();
  const pending = deferred();
  h.$.turn.abort = async ({ turnId }) => {
    h.abortCalls.push(turnId);
    entered.resolve();
    await pending.promise;
  };
  const event = ownEvent();
  const next = nextResult({ text: foreignA, foreignField: true });
  const attachment = h.invoke('prompt.attachment', event, next).then(value => ({ value }), error => ({ error }));
  await entered.promise;
  assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
  await h.open('B', 'resume');
  await h.turn('turn-B');
  pending.reject(failure);
  const settled = await attachment;
  assert.deepEqual(next.calls, [event]);
  assert.strictEqual(settled.error, failure);
  assert.strictEqual(h.caught[0].error, failure);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  await assertFreshB(h);
});

for (const field of ['text', 'foreignField']) {
  test(`owned recovery ${field} getter failure propagates the exact error and never replays`, async () => {
    const h = harness();
    await h.open('A');
    await h.turn('turn-A');
    const failure = new Error(`synthetic ${field} accessor failure`);
    const real = { text: foreignA, foreignField: true };
    Object.defineProperty(real, field, { enumerable: true, get() { throw failure; } });
    const event = ownEvent();
    const next = nextResult(real);
    let observed;
    try { await h.invoke('prompt.attachment', event, next); } catch (error) { observed = error; }
    assert.deepEqual(next.calls, [event]);
    assert.strictEqual(observed, failure);
    assert.strictEqual(h.caught[0].error, failure);
    assert.deepEqual(h.abortCalls, ['turn-A']);
    assert.equal((await h.receipt()).vivac_opening.status, 'blocked');
    await h.open('B', 'resume');
    await h.turn('turn-B');
    await assertFreshB(h);
    assert.deepEqual(h.abortCalls, ['turn-A']);
  });
}

test('a registered catch after a successful own result refuses explicitly without replay or a new abort', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const event = ownEvent();
  const next = nextResult({ text: `${marker}${foreignA}`, foreignField: true });
  assert.equal((await h.invoke('prompt.attachment', event, next)).text, `${contextA}${foreignA}`);
  await assert.rejects(h.hooks.get('prompt.attachment').catcher(h.$, event, next), /Synthetic attachment recovery failed after a downstream result/);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, []);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
});

async function collectTurn(iterator) {
  const yielded = [];
  let item;
  do {
    item = await iterator.next();
    if (!item.done) yielded.push(item.value);
  } while (!item.done);
  return { yielded, result: item.value };
}

async function assertBlockedWork(h, diagnostic = 'SYNTHETIC_PRIVATE_DIAGNOSTIC') {
  const original = { text: 'work', foreignField: { retained: true }, vivac_opening: { status: 'ready', generation: 999 } };
  const receiptNext = nextResult(value => value);
  const receipt = await h.invoke('classic.UserPromptSubmit', original, receiptNext);
  assert.equal(receipt.vivac_opening.status, 'blocked');
  assert.equal(receipt.vivac_opening.protocol, protocol);
  assert.notEqual(receipt.vivac_opening.generation, 999);
  assert.strictEqual(receipt.foreignField, original.foreignField);
  assert.equal(original.vivac_opening.status, 'ready');
  assert.equal(JSON.stringify(receipt).includes(diagnostic), false);
  assert.equal(receiptNext.calls.length, 1);
  const promptNext = nextResult(() => { throw new Error('Blocked prompt reached downstream'); });
  const refused = await h.invoke('prompt.submit', original, promptNext);
  assert.match(refused.drop, /not ready/);
  assert.equal(JSON.stringify(refused).includes(diagnostic), false);
  assert.equal(promptNext.calls.length, 0);
  let advanced = false;
  const turnNext = async function* () { advanced = true; yield { kind: 'unexpected' }; };
  turnNext.signal = new AbortController().signal;
  const stopped = await collectTurn(h.hooks.get('turn.step').handler(h.$, { turnId: 'blocked-turn', index: 2 }, turnNext));
  assert.equal(advanced, false);
  assert.deepEqual(stopped.yielded, [{ kind: 'stop', stopReason: 'end_turn', usage: null }]);
  assert.equal(stopped.result.turnId, 'blocked-turn');
  assert.equal(stopped.result.index, 2);
  assert.equal(stopped.result.stopReason, 'end_turn');
  for (const text of ['/clear', '/resume synthetic']) {
    const command = { text };
    const next = nextResult(value => value);
    assert.strictEqual(await h.invoke('prompt.submit', command, next), command);
    assert.deepEqual(next.calls, [command]);
  }
}

test('the reviewed opening bundle has the expected candidate bytes', () => {
  assert.equal(candidateSha256, 'AA0C9DC2BA5C348D65CFEF17F891ABA3B9853B555D0D9E71B56E1100F33C6F07');
});

test('the historical opening reference preserves the exact native-compatible v1 bytes', async () => {
  const path = new URL('../src/setup/claude-opening-versions/opening-reference-v1/hooks/register.mjs', import.meta.url);
  assert.equal(createHash('sha256').update(await readFile(path)).digest('hex').toUpperCase(),
    'B4BB19565116606AFD310765AC16FF46A336294AD38BD905915D91035068E376');
});

const producerDiagnostic = 'SYNTHETIC_PRIVATE_DIAGNOSTIC';
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
  ['stdout truncated', { ...ready(contextB), isStdoutTruncated: true }],
  ['stderr truncated', { ...ready(contextB), isStderrTruncated: true }],
  ['nonzero exit', { exitCode: 1, stdout: ready(producerDiagnostic).stdout }],
  ['unparseable output', { exitCode: 0, stdout: producerDiagnostic }],
  ['UTF8 60001 bytes', ready('🧭'.repeat(15000) + 'x')],
);

for (const [label, produced] of producerCases) {
  test(`child producer ${label} fails closed without damaging root or sibling`, async () => {
    let failing = false;
    const h = harness(async () => failing ? produced : ready(contextA));
    await h.open('A'); await openChild(h, 'sibling'); failing = true;
    const foreign = { additionalContext: [foreignB], retained: true };
    assert.strictEqual(await openChild(h, 'target', nextResult(foreign)), foreign);
    assert.equal((await h.turn('target-turn', 'target')).kind, 'stop');
    assert.equal((await h.turn('sibling-turn', 'sibling')).kind, 'synthetic-turn');
    assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  });

  test(`producer ${label} blocks work and keeps diagnostics out of output`, async () => {
    const h = harness(async () => produced);
    const foreign = { additionalContext: [foreignA], foreignField: { retained: true } };
    assert.strictEqual(await h.open('A', 'startup', foreign), foreign);
    const next = nextResult({ text: foreignA, foreignField: foreign.foreignField });
    const attached = await h.invoke('prompt.attachment', ownEvent(foreignA), next);
    assert.equal(attached.text, foreignA);
    assert.strictEqual(attached.foreignField, foreign.foreignField);
    assert.equal(next.calls.length, 1);
    await assertBlockedWork(h, producerDiagnostic);
  });
}

test('a producer exception preserves fallback fields without exposing its diagnostic or authorizing work', async () => {
  const failure = new Error(producerDiagnostic);
  const h = harness(async () => { throw failure; });
  const event = { source: 'startup', session_id: 'A' };
  const foreign = { additionalContext: [foreignA], foreignField: { retained: true } };
  const next = nextResult(foreign);
  assert.strictEqual(await h.invoke('classic.SessionStart', event, next), foreign);
  assert.strictEqual(h.caught[0].error, failure);
  assert.deepEqual(next.calls, [event]);
  await assertBlockedWork(h, producerDiagnostic);
});

test('the complete UTF8 context at exactly 60000 bytes is published without truncation', async () => {
  const context = '🧭'.repeat(15000);
  assert.equal(Buffer.byteLength(context, 'utf8'), 60000);
  assert.equal(Buffer.byteLength(context + 'x', 'utf8'), 60001);
  const h = harness(async () => ready(context));
  await h.open('A');
  const foreignField = { retained: true };
  const result = await h.invoke('prompt.attachment', ownEvent(), nextResult({ text: `${foreignA}${marker}${foreignB}`, foreignField }));
  assert.equal(result.text, `${foreignA}${context}${foreignB}`);
  assert.strictEqual(result.foreignField, foreignField);
  assert.equal((await h.receipt()).vivac_opening.status, 'ready');
  assert.equal((await h.turn('boundary-turn')).kind, 'synthetic-turn');
});

for (const source of ['unknown', undefined]) {
  test(`ready ${source ?? 'missing source'} blocks old publication and fresh clear recovers`, async () => {
    const h = harness();
    await h.open('A');
    const first = await h.invoke('prompt.attachment', ownEvent(), nextResult({ text: marker }));
    assert.equal(first.text, contextA);
    const event = { session_id: 'A' };
    if (source !== undefined) event.source = source;
    const foreign = { additionalContext: [foreignA], retained: true };
    assert.strictEqual(await h.invoke('classic.SessionStart', event, nextResult(foreign)), foreign);
    await assertBlockedWork(h);
    const next = nextResult({ text: `${marker}${foreignA}` });
    assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), next), { text: foreignA });
    assert.equal(next.calls.length, 1);
    await assertBlockedWork(h);
    await h.open('B', 'clear');
    await assertFreshB(h);
    assert.equal((await h.turn('recovered-turn')).kind, 'synthetic-turn');
  });
}

test('same-generation abort during production never authorizes an opening', async () => {
  const controller = new AbortController();
  const h = harness(async () => { controller.abort(); return ready(contextA); });
  const event = { session_id: 'A', source: 'startup' };
  const foreign = { additionalContext: [foreignA], retained: true };
  const next = nextResult(foreign, controller.signal);
  assert.strictEqual(await h.invoke('classic.SessionStart', event, next), foreign);
  assert.deepEqual(next.calls, [event]);
  await assertBlockedWork(h);
});

test('same-generation abort while opening downstream returns prevents publication', async () => {
  const controller = new AbortController();
  const h = harness();
  const event = { session_id: 'A', source: 'startup' };
  const foreign = { additionalContext: [foreignA], retained: true };
  const next = nextResult(() => { controller.abort(); return foreign; }, controller.signal);
  assert.strictEqual(await h.invoke('classic.SessionStart', event, next), foreign);
  assert.deepEqual(next.calls, [event]);
  await assertBlockedWork(h);
});

test('same-generation abort during attachment publication strips only its marker and blocks work', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const controller = new AbortController();
  const event = ownEvent();
  const real = { text: `${foreignA}${marker}`, foreignField: { retained: true } };
  const next = nextResult(() => { controller.abort(); return real; }, controller.signal);
  const attached = await h.invoke('prompt.attachment', event, next);
  assert.equal(attached.text, foreignA);
  assert.strictEqual(attached.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  await assertBlockedWork(h);
});

test('late opening publication and stale opening fallback preserve B and foreign result fields', async () => {
  const h = harness();
  const pending = deferred();
  const entered = deferred();
  const eventA = { session_id: 'A', source: 'startup' };
  const nextA = nextResult(() => { entered.resolve(); return pending.promise; });
  const openingA = h.invoke('classic.SessionStart', eventA, nextA);
  await entered.promise;
  await h.open('B', 'clear');
  const foreign = { additionalContext: [foreignA], foreignField: { retained: true } };
  pending.resolve(foreign);
  assert.strictEqual(await openingA, foreign);
  assert.deepEqual(nextA.calls, [eventA]);
  const controller = new AbortController();
  controller.abort();
  const replay = nextResult(foreign, controller.signal);
  assert.strictEqual(await h.hooks.get('classic.SessionStart').catcher(h.$, eventA, replay), foreign);
  assert.deepEqual(replay.calls, [eventA]);
  await assertFreshB(h);
  assert.equal((await h.turn('turn-B')).kind, 'synthetic-turn');
});

for (const [label, foreign] of [
  ['null', null], ['undefined', undefined], ['array', []], ['number', 7], ['string', 'foreign'],
  ['null additionalContext', { additionalContext: null, foreignField: true }],
  ['string additionalContext', { additionalContext: foreignA, foreignField: true }],
  ['object additionalContext', { additionalContext: { foreign: true }, foreignField: true }],
]) {
  test(`malformed SessionStart downstream ${label} stays unchanged and blocks work`, async () => {
    const h = harness();
    const event = { session_id: 'A', source: 'startup' };
    const next = nextResult(foreign);
    assert.strictEqual(await h.invoke('classic.SessionStart', event, next), foreign);
    assert.deepEqual(next.calls, [event]);
    await assertBlockedWork(h);
  });
}

test('guard catchers stop without downstream work and replace forged receipts without mutation', async () => {
  const h = harness();
  await h.open('A');
  const promptNext = nextResult(() => { throw new Error('Prompt catcher called next'); });
  assert.match((await h.hooks.get('prompt.submit').catcher(h.$, { text: 'work' }, promptNext)).drop, /not ready/);
  assert.equal(promptNext.calls.length, 0);
  let advanced = false;
  const turnNext = async function* () { advanced = true; };
  turnNext.signal = new AbortController().signal;
  const stopped = await collectTurn(h.hooks.get('turn.step').catcher(h.$, { turnId: 'catch-turn', index: 3 }, turnNext));
  assert.equal(advanced, false);
  assert.deepEqual(stopped.yielded, [{ kind: 'stop', stopReason: 'end_turn', usage: null }]);
  assert.equal(stopped.result.stopReason, 'end_turn');
  const original = { text: 'work', foreignField: { retained: true }, vivac_opening: { status: 'ready' } };
  const next = nextResult(value => value);
  const received = await h.hooks.get('classic.UserPromptSubmit').catcher(h.$, original, next);
  assert.equal(received.vivac_opening.status, 'blocked');
  assert.strictEqual(received.foreignField, original.foreignField);
  assert.equal(original.vivac_opening.status, 'ready');
  assert.equal(next.calls.length, 1);
});

test('a later failed attachment cannot reuse an earlier successful publication or call next twice', async () => {
  const h = harness();
  await h.open('A');
  assert.equal((await h.invoke('prompt.attachment', ownEvent(), nextResult({ text: marker }))).text, contextA);
  await h.turn('turn-A');
  const event = ownEvent();
  const real = { text: foreignA, foreignField: { retained: true } };
  const next = nextResult((_event, ordinal) => {
    assert.equal(ordinal, 1, 'No downstream replay is permitted after a result');
    return real;
  });
  const received = await h.invoke('prompt.attachment', event, next);
  assert.equal(received.text, foreignA);
  assert.strictEqual(received.foreignField, real.foreignField);
  assert.equal(received.text.includes(contextA), false);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  await assertBlockedWork(h);
  const oldReferenceNext = nextResult({ text: `${marker}${foreignB}` });
  assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), oldReferenceNext), { text: foreignB });
  assert.equal(oldReferenceNext.calls.length, 1);
  await assertBlockedWork(h);
  await h.open('B', 'clear');
  await assertFreshB(h);
});

test('one preparation per opening retains invocation options and never repeats for attachment reads', async () => {
  const calls = [];
  const h = harness(async (...args) => { calls.push(args); return ready(calls.length === 1 ? contextA : contextB); });
  const eventA = { source: 'startup', session_id: 'A', cwd: 'synthetic-fixture' };
  const foreign = { additionalContext: [foreignA], foreignField: { retained: true } };
  const opening = await h.invoke('classic.SessionStart', eventA, nextResult(foreign));
  assert.deepEqual(opening.additionalContext, [foreignA, marker]);
  assert.strictEqual(opening.foreignField, foreign.foreignField);
  assert.deepEqual(foreign.additionalContext, [foreignA]);
  assert.deepEqual(calls, [[['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], {
    stdin: JSON.stringify({ hook_event_name: 'SessionStart', source: 'startup', session_id: 'A' }), timeoutMs: 5000, cwd: 'synthetic-fixture',
  }]]);
  for (let index = 0; index < 2; index++) {
    assert.equal((await h.invoke('prompt.attachment', ownEvent(), nextResult({ text: marker }))).text, contextA);
  }
  await h.receipt();
  assert.equal(calls.length, 1);
  await h.invoke('classic.SessionStart', { source: 'clear', session_id: 'B' }, nextResult({}));
  assert.equal(calls.length, 2);
  assert.equal(Object.hasOwn(calls[1][1], 'cwd'), false);
  await assertFreshB(h);
  assert.equal(calls.length, 2);
});

test('absent openings accept all sources without context publication or inferred cwd', async () => {
  for (const source of ['startup', 'clear', 'resume', 'compact', 'unknown', undefined]) {
    const calls = [];
    const h = harness(async (...args) => { calls.push(args); return { exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }; });
    const event = { source, session_id: 'absent' };
    const foreign = { additionalContext: [foreignA] };
    assert.strictEqual(await h.invoke('classic.SessionStart', event, nextResult(foreign)), foreign);
    assert.equal(Object.hasOwn(calls[0][1], 'cwd'), false);
    assert.equal((await h.receipt()).vivac_opening.status, 'absent');
    const prompt = { text: 'work' };
    assert.strictEqual(await h.invoke('prompt.submit', prompt, nextResult(value => value)), prompt);
    assert.equal((await h.turn('absent-turn')).kind, 'synthetic-turn');
    const foreignResult = { text: foreignA, retained: true };
    assert.strictEqual(await h.invoke('prompt.attachment', ownEvent(foreignA), nextResult(foreignResult)), foreignResult);
    assert.equal(calls.length, 1);
  }
});

test('session end blocks prompt and turn after a successful attachment publication', async () => {
  const h = harness();
  await h.open('A');
  assert.equal((await h.invoke('prompt.attachment', ownEvent(), nextResult({ text: marker }))).text, contextA);
  assert.equal((await h.turn('turn-A')).kind, 'synthetic-turn');
  const event = { reason: 'exit', retained: true };
  const next = nextResult(value => value);
  assert.strictEqual(await h.invoke('session.end', event, next), event);
  assert.deepEqual(next.calls, [event]);
  await assertBlockedWork(h);
  const attachmentNext = nextResult({ text: `${marker}${foreignA}`, retained: true });
  assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), attachmentNext), { text: foreignA, retained: true });
  assert.equal(attachmentNext.calls.length, 1);
  assert.deepEqual(h.abortCalls, []);
  await assertBlockedWork(h);
});

for (const completion of ['finish', 'return', 'reject']) {
  test(`an A generator released by ${completion} is never an abort target for a later attachment failure`, async () => {
    const h = harness();
    await h.open('A');
    assert.equal((await h.turn('turn-A')).kind, 'synthetic-turn');
    if (completion === 'finish') {
      assert.deepEqual(await h.finishTurn('turn-A'), { value: { continued: true }, done: true });
    } else if (completion === 'return') {
      assert.deepEqual(await h.returnTurn('turn-A'), { value: undefined, done: true });
    } else {
      const error = new Error('synthetic generator rejection');
      await assert.rejects(h.rejectTurn('turn-A', error), failure => failure === error);
    }
    const event = ownEvent();
    const real = { text: foreignA, foreignField: { retained: true } };
    const next = nextResult(real);
    const received = await h.invoke('prompt.attachment', event, next);
    assert.equal(received.text, foreignA);
    assert.strictEqual(received.foreignField, real.foreignField);
    assert.deepEqual(next.calls, [event]);
    assert.deepEqual(h.abortCalls, []);
    await assertBlockedWork(h);
  });
}

test('finishing A after B starts does not clear B as the current abort target', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  await h.open('B', 'resume');
  await h.turn('turn-B');
  assert.deepEqual(await h.finishTurn('turn-A'), { value: { continued: true }, done: true });
  await assertFreshB(h);
  const event = ownEvent();
  const real = { text: foreignB, foreignField: { retained: true } };
  const next = nextResult(real);
  const received = await h.invoke('prompt.attachment', event, next);
  assert.equal(received.text, foreignB);
  assert.strictEqual(received.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, ['turn-B']);
  await assertBlockedWork(h);
});

test('an active A abort closes only its matching generator and recovery uses the actual result once', async () => {
  const h = harness();
  await h.open('A');
  await h.turn('turn-A');
  const event = ownEvent();
  const real = { text: foreignA, foreignField: { retained: true } };
  const next = nextResult(real);
  const received = await h.invoke('prompt.attachment', event, next);
  assert.equal(received.text, foreignA);
  assert.strictEqual(received.foreignField, real.foreignField);
  assert.deepEqual(next.calls, [event]);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  const subsequentNext = nextResult({ text: `${marker}${foreignB}`, retained: true });
  assert.deepEqual(await h.invoke('prompt.attachment', ownEvent(), subsequentNext), { text: foreignB, retained: true });
  assert.equal(subsequentNext.calls.length, 1);
  assert.deepEqual(h.abortCalls, ['turn-A']);
  await assertBlockedWork(h);
});
