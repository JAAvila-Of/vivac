// Experimental complete opening context for vivac.
const protocol = 'vivac.claude.opening.v1';
const marker = '@@VIVAC_OPENING_CURRENT_REFERENCE_8f716aa0c351448fb5ce9c8b21e7a699@@';
const blockedMessage = 'The vivac opening context is not ready. Check the integration and restart the session.';
const continuationBlockedMessage = 'Vivac cannot guarantee current context for SendMessage continuations. SendMessage is disabled while Vivac manages this session. Start a new subagent with Agent instead.';

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

function validAgentId(value) {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{1,256}$/.test(value);
}

function recipient(marker) {
  return { generation: 0, phase: 'blocked', snapshot: null, referencePublished: false, activeTurn: null, marker };
}

function invalidate(state) {
  state.generation += 1;
  state.phase = 'blocked';
  state.snapshot = null;
  state.referencePublished = false;
}

function current(state, token, next) {
  if (token !== state.generation) return false;
  if (next.signal.aborted) {
    invalidate(state);
    return false;
  }
  return true;
}

function preparation(produced, allowed) {
  if (produced.exitCode === 0 && produced.isStdoutTruncated !== true && produced.isStderrTruncated !== true) {
    try {
      const value = JSON.parse(produced.stdout);
      if (value && typeof value === 'object' && !Array.isArray(value) && value.protocol === protocol) {
        if (value.status === 'ready' && typeof value.context === 'string' && value.context.length > 0
            && utf8Bytes(value.context) <= 60000 && allowed)
          return { phase: 'ready', snapshot: { context: value.context } };
        if (value.status === 'absent' && !Object.hasOwn(value, 'context'))
          return { phase: 'absent', snapshot: null };
      }
    } catch {}
  }
  return { phase: 'blocked', snapshot: null };
}

function publish(state, token, next, result, prepared) {
  if (!current(state, token, next)) return result;
  if (prepared.phase === 'ready' && (!result || typeof result !== 'object' || Array.isArray(result)
      || (result.additionalContext !== undefined && (!Array.isArray(result.additionalContext)
        || result.additionalContext.findIndex(context => typeof context !== 'string') !== -1)))) {
    invalidate(state);
    return result;
  }
  state.phase = prepared.phase;
  state.snapshot = prepared.snapshot;
  if (state.phase !== 'ready') return result;
  const publication = { ...result, additionalContext: [...(result.additionalContext ?? []), state.marker] };
  if (!current(state, token, next)) return result;
  state.referencePublished = true;
  return publication;
}

export function register(on) {
  const root = recipient(marker);
  const children = new Map();
  const openings = new WeakMap();
  const attachments = new WeakMap();
  const returnedAttachments = new WeakSet();
  const attachmentFailures = new WeakMap();

  function invalidateAll(terminal = false) {
    invalidate(root);
    if (terminal) root.activeTurn = null;
    for (const child of children.values()) {
      invalidate(child);
      if (terminal) child.activeTurn = null;
    }
    children.clear();
  }

  function receipt(event, status) {
    return { ...event, vivac_opening: { protocol, status, generation: root.generation } };
  }

  function normalizeOwned(result, reference) {
    if (result && typeof result === 'object' && !Array.isArray(result) && typeof result.text === 'string')
      return { ...result, text: result.text.split(reference).join('') };
    if (result && typeof result === 'object' && !Array.isArray(result) && result.text === null) return result;
    return { text: null };
  }

  on('classic.SessionStart', async ($, event, next) => {
    invalidateAll();
    const token = root.generation;
    openings.set(event, { state: root, token });
    root.phase = 'pending';
    const options = { stdin: JSON.stringify({ hook_event_name: 'SessionStart', source: event.source, session_id: event.session_id }), timeoutMs: 5000 };
    if (typeof event.cwd === 'string') options.cwd = event.cwd;
    const produced = await $.process.run(['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], options);
    if (!current(root, token, next)) return next(event);
    const prepared = preparation(produced, ['startup', 'clear', 'resume', 'compact'].includes(event.source));
    const result = await next(event);
    return publish(root, token, next, result, prepared);
  }).catch(async ($, event, next) => {
    const selected = openings.get(event);
    if (selected && current(selected.state, selected.token, next)) invalidate(selected.state);
    const result = await next(event);
    if (selected) current(selected.state, selected.token, next);
    return result;
  });

  on('classic.SubagentStart', async ($, event, next) => {
    if (!validAgentId(event.agent_id)) return next(event);
    const previous = children.get(event.agent_id);
    if (previous) {
      invalidate(previous);
    }
    const state = recipient('@@VIVAC_OPENING_CHILD_REFERENCE_' + crypto.randomUUID() + '@@');
    children.set(event.agent_id, state);
    const token = state.generation;
    openings.set(event, { state, token });
    state.phase = 'pending';
    const options = { stdin: JSON.stringify({ hook_event_name: 'SubagentStart', agent_id: event.agent_id }), timeoutMs: 5000 };
    if (typeof event.cwd === 'string') options.cwd = event.cwd;
    const produced = await $.process.run(['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], options);
    if (!current(state, token, next)) return next(event);
    const prepared = preparation(produced, true);
    const result = await next(event);
    return publish(state, token, next, result, prepared);
  }).catch(async ($, event, next) => {
    const selected = openings.get(event);
    if (selected && current(selected.state, selected.token, next)) invalidate(selected.state);
    const result = await next(event);
    if (selected) current(selected.state, selected.token, next);
    return result;
  });

  on('session.end', async ($, event, next) => {
    invalidateAll(true);
    return next(event);
  });

  on('classic.PreToolUse', ($, event, next) => {
    if (event.tool === 'SendMessage' && root.phase !== 'absent') return { deny: continuationBlockedMessage };
    return next(event);
  });

  on('prompt.attachment', async ($, event, next) => {
    let result;
    let returned = false;
    try {
      const state = event.agentId === undefined ? root : validAgentId(event.agentId) ? children.get(event.agentId) : null;
      const token = state?.generation;
      const turn = state?.activeTurn;
      const reference = state?.marker;
      const eligible = !!state && event.origin?.kind === 'hook'
        && event.origin?.event === (state === root ? 'SessionStart' : 'SubagentStart');
      const before = !eligible || current(state, token, next);
      if (eligible && typeof event.text === 'string' && event.text.includes(reference))
        attachments.set(event, { state, token, turn, reference });
      result = await next(event);
      returned = true;
      returnedAttachments.add(event);
      if (attachments.has(event) && (typeof result?.text !== 'string' || !result.text.includes(reference)))
        throw new Error('Synthetic owned reference disappeared');
      if (!eligible || typeof result?.text !== 'string' || !result.text.includes(reference)) return result;
      attachments.set(event, { state, token, turn, reference });
      if (state !== root && (result.text.split(reference).length !== 2
          || (typeof event.text === 'string' && event.text.split(reference).length > 2)))
        throw new Error('Synthetic current reference is unavailable');
      if (!before || !current(state, token, next) || state.phase !== 'ready' || !state.referencePublished || !state.snapshot)
        throw new Error('Synthetic current reference is unavailable');
      const text = result.text.split(reference).join(state.snapshot.context);
      if (!current(state, token, next)) throw new Error('Synthetic current reference changed');
      if (state !== root) state.snapshot = null;
      return { ...result, text };
    } catch (error) {
      const selected = attachments.get(event);
      if (!returned || !selected) throw error;
      attachmentFailures.set(event, error);
      if (selected.token === selected.state.generation) invalidate(selected.state);
      try {
        if (selected.turn && selected.turn === selected.state.activeTurn) {
          await $.turn.abort({ turnId: selected.turn.id });
        }
        return normalizeOwned(result, selected.reference);
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
    if (selected.token === selected.state.generation) invalidate(selected.state);
    const result = await next(event);
    if (selected.turn && selected.turn === selected.state.activeTurn) {
      await $.turn.abort({ turnId: selected.turn.id });
    }
    return normalizeOwned(result, selected.reference);
  });

  on('prompt.submit', async ($, event, next) => {
    if (typeof event.text === 'string' && /^\/(clear|resume)(?:\s|$)/.test(event.text.trim())) return next(event);
    if (next.signal.aborted || (root.phase !== 'absent' && (root.phase !== 'ready' || !root.referencePublished))) return { drop: blockedMessage };
    return next(event);
  }).catch(() => ({ drop: blockedMessage }));

  on('turn.step', async function* ($, event, next) {
    const state = event.agentId === undefined ? root : validAgentId(event.agentId) ? children.get(event.agentId) : null;
    if (!state || next.signal.aborted || (state.phase !== 'absent' && (state.phase !== 'ready' || !state.referencePublished)))
      return yield* stopTurn(event);
    const turn = { id: event.turnId, generation: state.generation };
    state.activeTurn = turn;
    try {
      return yield* next(event);
    } finally {
      if (state.activeTurn === turn) state.activeTurn = null;
    }
  }).catch(async function* ($, event) { return yield* stopTurn(event); });

  on('classic.UserPromptSubmit', async ($, event, next) => {
    const status = !next.signal.aborted && (root.phase === 'absent' || (root.phase === 'ready' && root.referencePublished)) ? root.phase : 'blocked';
    return next(receipt(event, status));
  }).catch(($, event, next) => next(receipt(event, 'blocked')));
}
