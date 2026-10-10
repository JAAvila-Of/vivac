const protocol = 'vivac.claude.opening.v1';
const blockedMessage = 'The vivac opening context is not ready. Check the integration and restart the session.';

function utf8Bytes(text) {
  let bytes = 0;
  for (const character of text) {
    const point = character.codePointAt(0);
    bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    if (bytes > 60000) return bytes;
  }
  return bytes;
}

function* stopTurn(event) {
  yield { kind: 'stop', stopReason: 'end_turn', usage: null };
  return { turnId: event.turnId, index: event.index, answer: '', toolUses: [],
    stopReason: 'end_turn', usage: null };
}

export function register(on) {
  let generation = 0;
  let phase = 'blocked';
  let snapshot = null;
  let expanded = false;
  const openings = new WeakMap();
  const contexts = new WeakMap();
  const blockName = `vivac-opening-${Math.random().toString(36).slice(2)}`;

  function invalidate() {
    generation += 1;
    phase = 'blocked';
    snapshot = null;
    expanded = false;
  }

  function current(token, next) {
    if (token !== generation) return false;
    if (next.signal.aborted) {
      invalidate();
      return false;
    }
    return true;
  }

  function receipt(event, status) {
    return { ...event, vivac_opening: { protocol, status, generation } };
  }

  on('classic.SessionStart', async ($, event, next) => {
    invalidate();
    const token = generation;
    openings.set(event, token);
    phase = 'pending';
    const options = { stdin: JSON.stringify({ source: event.source, session_id: event.session_id }), timeoutMs: 5000 };
    if (typeof event.cwd === 'string') options.cwd = event.cwd;
    const produced = await $.process.run(['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], options);
    if (!current(token, next)) return next(event);
    let candidate = null;
    let status = 'blocked';
    if (produced.exitCode === 0) {
      try {
        const value = JSON.parse(produced.stdout);
        if (value && typeof value === 'object' && !Array.isArray(value) && value.protocol === protocol) {
          if (value.status === 'ready' && typeof value.context === 'string' && value.context.length > 0 && utf8Bytes(value.context) <= 60000) {
            candidate = { context: value.context };
            status = 'ready';
          } else if (value.status === 'absent' && !Object.hasOwn(value, 'context')) {
            status = 'absent';
          }
        }
      } catch {}
    }
    const result = await next(event);
    if (!current(token, next)) return result;
    phase = status;
    snapshot = candidate;
    return result;
  }).catch(async ($, event, next) => {
    const token = openings.get(event);
    if (token !== undefined && current(token, next)) invalidate();
    const result = await next(event);
    current(token, next);
    return result;
  });

  on('session.end', async ($, event, next) => {
    invalidate();
    return next(event);
  });

  on('prompt.context', async ($, event, next) => {
    const token = generation;
    expanded = false;
    contexts.set(event, token);
    const result = await next(event);
    if (!current(token, next) || phase !== 'ready') return result;
    if (!result || typeof result !== 'object' || Array.isArray(result) || !Array.isArray(result.blocks)
        || result.blocks.some(block => block?.name === blockName)) {
      invalidate();
      return result;
    }
    if (!current(token, next)) return result;
    const publication = { ...result, blocks: [...result.blocks, { name: blockName, text: snapshot.context }] };
    if (!current(token, next)) return result;
    expanded = true;
    return publication;
  }).catch(async ($, event, next) => {
    const token = contexts.get(event);
    if (token !== undefined && current(token, next)) invalidate();
    const result = await next(event);
    current(token, next);
    return result;
  });

  on('prompt.submit', async ($, event, next) => {
    if (typeof event.text === 'string' && /^\/(clear|resume)(?:\s|$)/.test(event.text.trim())) return next(event);
    if (next.signal.aborted || (phase !== 'ready' && phase !== 'absent')) return { drop: blockedMessage };
    return next(event);
  }).catch(() => ({ drop: blockedMessage }));

  on('turn.step', async function* ($, event, next) {
    if (next.signal.aborted || (phase !== 'absent' && (phase !== 'ready' || !expanded))) return yield* stopTurn(event);
    return yield* next(event);
  }).catch(async function* ($, event) { return yield* stopTurn(event); });

  on('classic.UserPromptSubmit', async ($, event, next) => {
    const status = !next.signal.aborted && (phase === 'ready' || phase === 'absent') ? phase : 'blocked';
    return next(receipt(event, status));
  }).catch(($, event, next) => next(receipt(event, 'blocked')));
}
