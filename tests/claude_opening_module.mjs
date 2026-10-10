import assert from 'node:assert/strict';
import test from 'node:test';
import { register } from '../src/setup/claude-opening/hooks/register.mjs';

const protocol = 'vivac.claude.opening.v1';
const ready = context => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'ready', context }) });
const opening = (source = 'startup') => ({ source, session_id: 'opaque-session', cwd: '/project' });
const step = { turnId: 'opaque-turn', index: 2 };
const foreign = () => ({ blocks: [{ name: 'foreign', text: 'Foreign context' }], instructionFiles: ['instructions'], other: true });
function harness(produce = async () => ready('Complete context 🧭')) {
  const hooks = new Map();
  const calls = [];
  register((name, handler) => {
    const entry = { handler };
    hooks.set(name, entry);
    return { catch: fallback => { entry.fallback = fallback; } };
  });
  const api = { process: { run: async (...args) => { calls.push(args); return produce(...args); } } };
  const invoke = (name, event, next = async value => value, aborted = false, fallback = false) => {
    next.signal = { aborted };
    return hooks.get(name)[fallback ? 'fallback' : 'handler'](api, event, next);
  };
  return { calls, invoke };
}
async function collect(stream) {
  const yielded = [];
  let item;
  do { item = await stream.next(); if (!item.done) yielded.push(item.value); } while (!item.done);
  return { yielded, result: item.value };
}
async function advances(h) {
  let advanced = false;
  await collect(h.invoke('turn.step', step, async function* () { advanced = true; return 'sent'; }));
  return advanced;
}
const context = (h, next = async () => foreign(), event = {}, aborted = false, fallback = false) => h.invoke('prompt.context', event, next, aborted, fallback);

test('complete cached context preserves foreign blocks with one preparation per opening', async () => {
  const h = harness();
  const result = await h.invoke('classic.SessionStart', opening(), async () => ({ additionalContext: ['Foreign hook context'], other: true }));
  assert.deepEqual(result, { additionalContext: ['Foreign hook context'], other: true });
  assert.deepEqual(h.calls[0], [['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], { stdin: JSON.stringify({ source: 'startup', session_id: 'opaque-session' }), timeoutMs: 5000, cwd: '/project' }]);
  assert.equal(await advances(h), false);
  for (let index = 0; index < 2; index++) {
    const event = { prompt: 'work' };
    const received = await context(h, async value => { assert.deepEqual(value, event); return foreign(); }, event);
    assert.deepEqual(received.instructionFiles, ['instructions']);
    assert.equal(received.other, true);
    assert.deepEqual(received.blocks[0], foreign().blocks[0]);
    assert.equal(received.blocks[1].text, 'Complete context 🧭');
    assert.match(received.blocks[1].name, /^vivac-opening-/);
    assert.equal(received.blocks.length, 2);
    assert.equal(await advances(h), true);
  }
  assert.equal(h.calls.length, 1);
  assert.equal((await h.invoke('classic.UserPromptSubmit', { vivac_opening: { status: 'spoofed' } })).vivac_opening.status, 'ready');
});

test('absent tree permits work without an own block or inferred cwd', async () => {
  for (const source of ['startup', 'clear', 'resume', 'compact', 'unknown', undefined]) {
    const h = harness(async () => ({ exitCode: 0, stdout: JSON.stringify({ protocol, status: 'absent' }) }));
    await h.invoke('classic.SessionStart', { source });
    assert.equal('cwd' in h.calls[0][1], false);
    assert.deepEqual(await context(h), foreign());
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'absent');
    assert.equal((await h.invoke('prompt.submit', { text: 'work' })).text, 'work');
    assert.equal(await advances(h), true);
    assert.equal(h.calls.length, 1);
  }
});

for (const source of ['resume', 'compact', 'unknown', undefined]) {
  test(`ready ${source ?? 'missing source'} stays blocked after context rendering and clear recovers with fresh context`, async () => {
    let ordinal = 0;
    const h = harness(async () => ready(++ordinal === 1 ? 'Startup context' : ordinal === 2 ? 'Unverified opening context' : 'Fresh clear context'));
    await h.invoke('classic.SessionStart', opening());
    const published = await context(h);
    assert.equal(await advances(h), true);
    const hookResult = { additionalContext: ['Foreign hook context'], other: true };
    assert.deepEqual(await h.invoke('classic.SessionStart', { ...opening(), source }, async () => hookResult), hookResult);
    assert.equal(h.calls.length, 2);
    assert.equal(JSON.parse(h.calls[1][1].stdin).source, source);
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
    assert.match((await h.invoke('prompt.submit', { text: 'work' })).drop, /not ready/);
    assert.equal(await advances(h), false);
    assert.deepEqual(await context(h), foreign());
    assert.deepEqual(await context(h, async () => published), published);
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
    assert.match((await h.invoke('prompt.submit', { text: 'work' })).drop, /not ready/);
    assert.equal(await advances(h), false);
    assert.equal(h.calls.length, 2);

    assert.equal((await h.invoke('prompt.submit', { text: '/clear' })).text, '/clear');
    await h.invoke('session.end', { reason: 'clear' });
    assert.deepEqual(await h.invoke('classic.SessionStart', opening('clear'), async () => hookResult), hookResult);
    const recovered = await context(h);
    assert.deepEqual(recovered.blocks[0], foreign().blocks[0]);
    assert.deepEqual(recovered.instructionFiles, foreign().instructionFiles);
    assert.equal(recovered.other, true);
    assert.equal(recovered.blocks.length, 2);
    assert.equal(recovered.blocks[1].text, 'Fresh clear context');
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'ready');
    assert.equal((await h.invoke('prompt.submit', { text: 'work' })).text, 'work');
    assert.equal(await advances(h), true);
    assert.equal(h.calls.length, 3);
    assert.deepEqual(h.calls.map(([, options]) => JSON.parse(options.stdin).source), ['startup', source, 'clear']);
  });
}

test('invalid producers block work and never expose raw diagnostics', async () => {
  const invalid = [
    { protocol: 'wrong', status: 'ready', context: 'private raw error' },
    { protocol, status: 'ready', context: '🧭'.repeat(15001) },
    { protocol, status: 'rejected', reason: 'private raw error' },
    { protocol, status: 'unknown' },
    { protocol, status: 'absent', context: 'unexpected context' },
    { protocol, status: 'ready', context: 42 },
    { protocol, status: 'ready', context: '' },
  ].map(value => ({ exitCode: 0, stdout: JSON.stringify(value) }));
  invalid.push({ exitCode: 1, stdout: ready('context').stdout }, { exitCode: 0, stdout: 'not json' });
  for (const produced of invalid) {
    const h = harness(async () => produced);
    await h.invoke('classic.SessionStart', opening());
    assert.deepEqual(await context(h), foreign());
    const blocked = await h.invoke('prompt.submit', { text: 'work' });
    assert.match(blocked.drop, /not ready/);
    assert.equal(JSON.stringify(blocked).includes('private raw error'), false);
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
    assert.equal(await advances(h), false);
    assert.equal((await h.invoke('prompt.submit', { text: '/clear' })).text, '/clear');
    assert.equal((await h.invoke('prompt.submit', { text: '/resume opaque' })).text, '/resume opaque');
  }
});

test('the complete multibyte context at the byte boundary remains intact', async () => {
  const text = '🧭'.repeat(15000);
  const h = harness(async () => ready(text));
  await h.invoke('classic.SessionStart', opening());
  assert.equal((await context(h)).blocks[1].text, text);
});

test('abortion during opening or publication cannot authorize a turn', async () => {
  const h = harness();
  const next = async () => { next.signal.aborted = true; return { additionalContext: ['Foreign'] }; };
  assert.deepEqual(await h.invoke('classic.SessionStart', opening(), next), { additionalContext: ['Foreign'] });
  assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
  await h.invoke('classic.SessionStart', opening('clear'));
  const contextNext = async () => { contextNext.signal.aborted = true; return foreign(); };
  assert.deepEqual(await context(h, contextNext), foreign());
  assert.equal(await advances(h), false);
});

test('late opening and stale opening catch cannot overwrite clear B', async () => {
  let resolveA;
  let ordinal = 0;
  const h = harness(() => ++ordinal === 1 ? new Promise(resolve => { resolveA = resolve; }) : Promise.resolve(ready('B')));
  const eventA = opening();
  const pendingA = h.invoke('classic.SessionStart', eventA);
  await h.invoke('session.end', { reason: 'clear' });
  await h.invoke('classic.SessionStart', opening('clear'));
  resolveA(ready('A'));
  assert.equal((await pendingA).additionalContext, undefined);
  await h.invoke('classic.SessionStart', eventA, async () => ({}), false, true);
  assert.equal((await context(h)).blocks[1].text, 'B');
  assert.equal(await advances(h), true);
});

test('late context A and its catch preserve already published B and foreign blocks', async () => {
  let ordinal = 0;
  const h = harness(async () => ready(++ordinal === 1 ? 'A' : 'B'));
  await h.invoke('classic.SessionStart', opening());
  let resolveContext;
  const eventA = { prompt: 'A' };
  const pending = context(h, value => {
    assert.deepEqual(value, eventA);
    return new Promise(resolve => { resolveContext = resolve; });
  }, eventA);
  await h.invoke('classic.SessionStart', opening('clear'));
  const resultB = await context(h);
  resolveContext(resultB);
  assert.deepEqual(await pending, resultB);
  assert.equal(resultB.blocks.some(block => block.text === 'A'), false);
  assert.deepEqual(await context(h, async () => resultB, eventA, false, true), resultB);
  assert.equal(await advances(h), true);
});

test('invalid context results and name collisions block without changing foreign data', async () => {
  for (const result of [null, {}, { blocks: null }, []]) {
    const h = harness();
    await h.invoke('classic.SessionStart', opening());
    assert.deepEqual(await context(h, async () => result), result);
    assert.equal(await advances(h), false);
    assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
  }
  const h = harness();
  await h.invoke('classic.SessionStart', opening());
  const published = await context(h);
  const colliding = { blocks: [{ name: published.blocks[1].name, text: 'Foreign collision' }] };
  assert.deepEqual(await context(h, async () => colliding), colliding);
  assert.equal(await advances(h), false);
});

test('a failed later render cannot inherit publication from an earlier successful render', async () => {
  const h = harness();
  await h.invoke('classic.SessionStart', opening());
  await context(h);
  assert.equal(await advances(h), true);
  const event = {};
  await assert.rejects(context(h, async () => { throw new Error('synthetic failure'); }, event), /synthetic failure/);
  assert.equal(await advances(h), false);
  assert.deepEqual(await context(h, async () => foreign(), event, false, true), foreign());
  assert.equal((await h.invoke('classic.UserPromptSubmit', {})).vivac_opening.status, 'blocked');
  await h.invoke('classic.SessionStart', opening('clear'));
  await context(h);
  assert.equal(await advances(h), true);
});

test('catch guards stop without downstream work and replace incoming receipts', async () => {
  const h = harness();
  let called = false;
  const stopped = await collect(h.invoke('turn.step', step, async function* () { called = true; }, false, true));
  assert.equal(called, false);
  assert.deepEqual(stopped.yielded, [{ kind: 'stop', stopReason: 'end_turn', usage: null }]);
  assert.equal(stopped.result.stopReason, 'end_turn');
  assert.match((await h.invoke('prompt.submit', {}, async () => { called = true; }, false, true)).drop, /not ready/);
  assert.equal(called, false);
  const original = { text: 'work', vivac_opening: { status: 'ready' } };
  assert.equal((await h.invoke('classic.UserPromptSubmit', original, async value => value, false, true)).vivac_opening.status, 'blocked');
  assert.equal(original.vivac_opening.status, 'ready');
});

test('producer exceptions and session end block without exposing diagnostics', async () => {
  const h = harness(async () => { throw new Error('private producer diagnostic'); });
  const event = opening();
  await assert.rejects(h.invoke('classic.SessionStart', event), /private producer diagnostic/);
  assert.deepEqual(await h.invoke('classic.SessionStart', event, async () => ({ additionalContext: ['Foreign'] }), false, true), { additionalContext: ['Foreign'] });
  const receipt = await h.invoke('classic.UserPromptSubmit', {});
  assert.equal(receipt.vivac_opening.status, 'blocked');
  assert.equal(JSON.stringify(receipt).includes('private producer diagnostic'), false);
  const loaded = harness();
  await loaded.invoke('classic.SessionStart', opening());
  await context(loaded);
  assert.equal(await advances(loaded), true);
  await loaded.invoke('session.end', { reason: 'exit' });
  assert.equal(await advances(loaded), false);
});
