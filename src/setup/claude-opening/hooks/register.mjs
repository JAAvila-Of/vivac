// Fixture-only candidate. It is not installed by the product.
const protocol = 'vivac.claude.opening.v1';
const marker = '@@VIVAC_OPENING_CURRENT_REFERENCE_8f716aa0c351448fb5ce9c8b21e7a699@@';
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
  let referencePublished = false;
  let activeTurn = null;
  const openings = new WeakMap();
  const attachments = new WeakMap();
  const returnedAttachments = new WeakSet();
  const attachmentFailures = new WeakMap();

  function invalidate() {
    generation += 1;
    phase = 'blocked';
    snapshot = null;
    referencePublished = false;
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

  function normalizeOwned(result) {
    if (result && typeof result === 'object' && !Array.isArray(result) && typeof result.text === 'string')
      return { ...result, text: result.text.split(marker).join('') };
    if (result && typeof result === 'object' && !Array.isArray(result) && result.text === null) return result;
    return { text: null };
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
          if (value.status === 'ready' && typeof value.context === 'string' && value.context.length > 0
              && utf8Bytes(value.context) <= 60000 && ['startup', 'clear', 'resume', 'compact'].includes(event.source)) {
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
    if (status === 'ready' && (!result || typeof result !== 'object' || Array.isArray(result)
        || (result.additionalContext !== undefined && !Array.isArray(result.additionalContext)))) {
      invalidate();
      return result;
    }
    phase = status;
    snapshot = candidate;
    if (status !== 'ready') return result;
    const publication = { ...result, additionalContext: [...(result.additionalContext ?? []), marker] };
    if (!current(token, next)) return result;
    referencePublished = true;
    return publication;
  }).catch(async ($, event, next) => {
    const token = openings.get(event);
    if (token !== undefined && current(token, next)) invalidate();
    const result = await next(event);
    current(token, next);
    return result;
  });

  on('session.end', async ($, event, next) => {
    invalidate();
    activeTurn = null;
    return next(event);
  });

  on('prompt.attachment', async ($, event, next) => {
    let result;
    let returned = false;
    try {
      const token = generation;
      const turn = activeTurn;
      const eligible = event.origin?.kind === 'hook' && event.origin?.event === 'SessionStart';
      const before = !eligible || current(token, next);
      if (eligible && typeof event.text === 'string' && event.text.includes(marker))
        attachments.set(event,{token,turn});
      result = await next(event);
      returned = true;
      returnedAttachments.add(event);
      if (attachments.has(event) && (typeof result?.text !== 'string' || !result.text.includes(marker)))
        throw new Error('Synthetic owned reference disappeared');
      if (!eligible || typeof result?.text !== 'string' || !result.text.includes(marker)) return result;
      attachments.set(event, { token, turn });
      if (!before || !current(token, next) || phase !== 'ready' || !referencePublished || !snapshot)
        throw new Error('Synthetic current reference is unavailable');
      const text = result.text.split(marker).join(snapshot.context);
      if (!current(token, next)) throw new Error('Synthetic current reference changed');
      return { ...result, text };
    } catch (error) {
      const selected = attachments.get(event);
      if (!returned || !selected) throw error;
      attachmentFailures.set(event, error);
      if (selected.token === generation) invalidate();
      try {
        if (selected.turn && selected.turn === activeTurn) {
          await $.turn.abort({ turnId: selected.turn.id });
        }
        return normalizeOwned(result);
      } catch (recoveryError) {
        attachmentFailures.set(event, recoveryError);
        throw recoveryError;
      }
    }
  }).catch(async ($, event, next) => {
    const selected = attachments.get(event);
    if (!selected) throw new Error('Synthetic attachment failure was not classified');
    if (returnedAttachments.has(event)) {
      if (attachmentFailures.has(event)) throw attachmentFailures.get(event);
      throw new Error('Synthetic attachment recovery failed after a downstream result');
    }
    if (selected.token === generation) invalidate();
    const result = await next(event);
    if (selected.turn && selected.turn === activeTurn) {
      await $.turn.abort({ turnId: selected.turn.id });
    }
    return normalizeOwned(result);
  });

  on('prompt.submit', async ($, event, next) => {
    if (typeof event.text === 'string' && /^\/(clear|resume)(?:\s|$)/.test(event.text.trim())) return next(event);
    if (next.signal.aborted || (phase !== 'ready' && phase !== 'absent')) return { drop: blockedMessage };
    return next(event);
  }).catch(() => ({ drop: blockedMessage }));

  on('turn.step', async function* ($, event, next) {
    if (next.signal.aborted || (phase !== 'absent' && (phase !== 'ready' || !referencePublished)))
      return yield* stopTurn(event);
    const turn = { id: event.turnId, generation };
    activeTurn = turn;
    try {
      return yield* next(event);
    } finally {
      if (activeTurn === turn) activeTurn = null;
    }
  }).catch(async function* ($, event) { return yield* stopTurn(event); });

  on('classic.UserPromptSubmit', async ($, event, next) => {
    const status = !next.signal.aborted && (phase === 'absent' || (phase === 'ready' && referencePublished)) ? phase : 'blocked';
    return next(receipt(event, status));
  }).catch(($, event, next) => next(receipt(event, 'blocked')));
}
