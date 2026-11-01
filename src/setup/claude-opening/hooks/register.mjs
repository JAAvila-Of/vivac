// Experimental complete opening context for vivac.
const protocol = 'vivac.claude.opening.v1';
const marker = '@@VIVAC_OPENING_CURRENT_REFERENCE_8f716aa0c351448fb5ce9c8b21e7a699@@';

function utf8Bytes(text) {
  let bytes = 0;
  for (const character of text) {
    const point = character.codePointAt(0);
    bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    if (bytes > 60000) return bytes;
  }
  return bytes;
}

function validAgentId(value) {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{1,256}$/.test(value);
}

function recipient(marker) {
  return { generation: 0, phase: 'blocked', snapshot: null, referencePublished: false, marker };
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

function replaceText(result, text) {
  const descriptors = Object.getOwnPropertyDescriptors(result);
  descriptors.text = { value: text, enumerable: true, configurable: true, writable: true };
  return Object.create(Object.getPrototypeOf(result), descriptors);
}

function normalizeOwned(result, reference) {
  if (result && typeof result === 'object' && !Array.isArray(result) && typeof result.text === 'string')
    return replaceText(result, result.text.split(reference).join(''));
  return result;
}

export function register(on) {
  const root = recipient(marker);
  const children = new Map();

  function invalidateAll() {
    invalidate(root);
    for (const child of children.values()) invalidate(child);
    children.clear();
  }

  async function open($, event, next, state, input, allowed) {
    const token = state.generation;
    state.phase = 'pending';
    let prepared = { phase: 'blocked', snapshot: null };
    try {
      const options = { stdin: JSON.stringify(input), timeoutMs: 5000 };
      if (typeof event.cwd === 'string') options.cwd = event.cwd;
      const produced = await $.process.run(['vivac', 'session', 'start', '--hook', '--context', 'claude-module'], options);
      if (current(state, token, next)) prepared = preparation(produced, allowed);
    } catch {
      if (token === state.generation) invalidate(state);
    }
    let result;
    try {
      result = await next(event);
    } catch (error) {
      if (token === state.generation) invalidate(state);
      throw error;
    }
    try {
      return publish(state, token, next, result, prepared);
    } catch {
      if (token === state.generation) invalidate(state);
      return result;
    }
  }

  on('classic.SessionStart', ($, event, next) => {
    invalidateAll();
    return open($, event, next, root,
      { hook_event_name: 'SessionStart', source: event.source, session_id: event.session_id },
      ['startup', 'clear', 'resume', 'compact'].includes(event.source));
  });

  on('classic.SubagentStart', ($, event, next) => {
    if (!validAgentId(event.agent_id)) return next(event);
    const previous = children.get(event.agent_id);
    if (previous) invalidate(previous);
    const state = recipient('@@VIVAC_OPENING_CHILD_REFERENCE_' + crypto.randomUUID() + '@@');
    children.set(event.agent_id, state);
    return open($, event, next, state,
      { hook_event_name: 'SubagentStart', agent_id: event.agent_id }, true);
  });

  on('session.end', ($, event, next) => {
    invalidateAll();
    return next(event);
  });

  on('prompt.attachment', async ($, event, next) => {
    const state = event.agentId === undefined ? root : validAgentId(event.agentId) ? children.get(event.agentId) : null;
    const token = state?.generation;
    const reference = state?.marker;
    const eligible = !!state && event.origin?.kind === 'hook'
      && event.origin?.event === (state === root ? 'SessionStart' : 'SubagentStart');
    const before = !eligible || current(state, token, next);
    let result;
    try {
      result = await next(event);
    } catch (error) {
      if (eligible && token === state.generation && typeof event.text === 'string' && event.text.includes(reference))
        invalidate(state);
      throw error;
    }
    try {
      const previouslyOwned = eligible && typeof event.text === 'string' && event.text.includes(reference);
      if (!eligible || typeof result?.text !== 'string' || !result.text.includes(reference)) {
        if (previouslyOwned && token === state.generation) invalidate(state);
        return result;
      }
      const duplicate = state !== root && (result.text.split(reference).length !== 2
        || (typeof event.text === 'string' && event.text.split(reference).length > 2));
      if (duplicate || !before || !current(state, token, next) || state.phase !== 'ready'
          || !state.referencePublished || !state.snapshot) {
        if (token === state.generation) invalidate(state);
        return normalizeOwned(result, reference);
      }
      const text = result.text.split(reference).join(state.snapshot.context);
      if (!current(state, token, next)) return normalizeOwned(result, reference);
      const expanded = replaceText(result, text);
      if (state !== root) state.snapshot = null;
      return expanded;
    } catch {
      if (eligible && token === state.generation) invalidate(state);
      return result;
    }
  });

  on('classic.UserPromptSubmit', ($, event, next) => {
    const status = !next.signal.aborted && (root.phase === 'absent' || (root.phase === 'ready' && root.referencePublished)) ? root.phase : 'blocked';
    return next({ ...event, vivac_opening: { protocol, status, generation: root.generation } });
  });
}
