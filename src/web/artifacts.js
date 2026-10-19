(function () {
  'use strict';
  const $ = id => document.getElementById(id);
  const bootstrap = $('bootstrap');
  if (!bootstrap) return;
  const config = JSON.parse(bootstrap.textContent);
  const fragment = new URLSearchParams(location.hash.slice(1));
  let page = [], offset = 0, next = null, listGeneration = 0, detailGeneration = 0;
  let selected = null, latest = null, revisions = [], historyOffset = 0, historyNext = null;
  let selectedLabel = null;
  let capabilities = null, writing = false, dialogFocus = null;
  let historyLoading = false, historyPrevious = null, historyFollowing = null;
  const short = id => String(id).slice(-8);
  const basename = source => String(source).split(/[\\/]/).pop();
  const typeLabel = type => ({ file: 'Single UTF-8 file', bundle: 'Bundle', sqlite_application: 'SQLite application', sqlite_snapshot: 'SQLite data' })[type] || type;
  const bundleType = type => ['bundle', 'sqlite_application'].includes(type);
  const isBundle = entry => bundleType(entry.delivery_type);
  const descriptionText = entry => entry.description || 'No description recorded.';
  const deliveryLabel = entry => isBundle(entry) ? typeLabel(entry.delivery_type) + ' · Entry: ' + entry.entrypoint + ' · ' + entry.member_count + ' members' + (entry.database ? ' · Database: ' + entry.database : '') : typeLabel('file');
  const formats = values => (values || []).map(value => value === 'utf8_text' ? 'UTF-8 text' : value).join(', ');
  const observationStorageKey = 'vivac.artifact-checks.v1';
  const observations = new Map();
  const observationFields = ['project', 'site', 'artifact', 'revision', 'digest', 'size', 'retained', 'source', 'checked_at'];
  const knownStatuses = ['matches', 'changed', 'missing', 'read_failed', 'foreign_site'];
  const identityValue = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
  function validObservation(value) {
    return value && typeof value === 'object' && !Array.isArray(value)
      && Object.keys(value).length === observationFields.length && observationFields.every(field => Object.hasOwn(value, field))
      && typeof value.project === 'string' && /^\/[A-Za-z0-9/_-]{1,256}$/.test(value.project)
      && ['site', 'artifact', 'revision'].every(field => identityValue(value[field]))
      && typeof value.digest === 'string' && /^[a-f0-9]{64}$/.test(value.digest)
      && Number.isSafeInteger(value.size) && value.size >= 0
      && knownStatuses.filter(status => status !== 'foreign_site').includes(value.retained) && knownStatuses.includes(value.source)
      && typeof value.checked_at === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/.test(value.checked_at)
      && Number.isFinite(Date.parse(value.checked_at))
      && new Date(value.checked_at).toISOString() === value.checked_at.replace(/(?:\.(\d{1,3}))?Z$/, (_, fraction) => '.' + (fraction || '').padEnd(3, '0') + 'Z');
  }
  function observationKey(value) {
    return JSON.stringify([value.project, value.site, value.artifact, value.revision, value.digest, value.size]);
  }
  try {
    const raw = sessionStorage.getItem(observationStorageKey);
    if (raw && raw.length <= 200000) {
      const cache = JSON.parse(raw);
      if (cache && cache.version === 1 && Object.keys(cache).length === 2 && Array.isArray(cache.entries) && cache.entries.length <= 200) {
        cache.entries.filter(validObservation).forEach(value => observations.set(observationKey(value), value));
      }
    }
  } catch (_) { /* The current view remains usable when session storage is unavailable. */ }
  function observationFor(entry) {
    return observations.get(observationKey(Object.assign({ project: config.project_base }, entry)));
  }
  function rememberObservation(entry, result) {
    if (result.artifact !== entry.artifact || result.revision !== entry.revision) return null;
    const value = { project: config.project_base, site: entry.site, artifact: entry.artifact, revision: entry.revision,
      digest: entry.digest, size: entry.size, retained: result.retained?.status, source: result.source?.status, checked_at: result.checked_at };
    if (!validObservation(value)) return null;
    const key = observationKey(value); observations.delete(key); observations.set(key, value);
    while (observations.size > 200) observations.delete(observations.keys().next().value);
    try { sessionStorage.setItem(observationStorageKey, JSON.stringify({ version: 1, entries: [...observations.values()] })); }
    catch (_) { /* Keep the observation in this view when session storage cannot be written. */ }
    return value;
  }
  function observationText(value) {
    if (!value) return 'Not checked in this view. Integrity does not imply approval.';
    const describe = status => ({ matches: 'matches', changed: 'changed', missing: 'missing', read_failed: 'read failed', foreign_site: 'unknown — source belongs to another site' })[status];
    return 'Retained: ' + describe(value.retained) + '. Source: ' + describe(value.source) + '. Last checked ' + value.checked_at
      + '. Not rechecked since. Integrity does not imply approval.';
  }
  function sizeLabel(bytes) {
    if (!Number.isFinite(bytes)) return 'Not specified';
    return bytes >= 1024 * 1024 ? Number((bytes / (1024 * 1024)).toFixed(2)) + ' MiB (' + bytes + ' bytes)' : bytes + ' bytes';
  }
  function element(tag, text, className) {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  }
  function button(text, callback) {
    const node = element('button', text); node.type = 'button';
    node.addEventListener('click', callback); return node;
  }
  function note(text) { $('message').textContent = text; }
  function nodeLink(node, text) {
    const link = element('a', text);
    link.href = config.project_base + '/why/' + encodeURIComponent(node);
    return link;
  }
  function producer(entry) {
    const node = entry.producer_node;
    return nodeLink(node ? node.node : entry.producer,
      node ? (node.alias || short(node.node)) + ' · ' + node.title : entry.producer);
  }
  function references(entry) {
    const box = element('div');
    const refs = entry.decision_references || [];
    if (!refs.length) box.append(element('p', 'No reference to this revision', 'muted'));
    refs.forEach(ref => box.append(nodeLink(ref.node, (ref.alias || short(ref.node)) + ' · ' + ref.kind + ' reference')));
    if (entry.decision_references_truncated) box.append(element('p', 'Showing ' + refs.length + ' of ' + entry.decision_references_total + ' references.'));
    return box;
  }
  function inventoryReferences(entry) {
    const box = element('div');
    const refs = entry.artifact_decision_references || [];
    if (!refs.length) box.append(element('p', 'No decision reference recorded for this artifact', 'muted'));
    refs.forEach(ref => box.append(nodeLink(ref.node, short(ref.revision) + ' · ' + (ref.alias || short(ref.node)) + ' · decision reference · ' + (ref.revision === entry.revision ? 'Latest revision' : 'Earlier revision'))));
    if (entry.artifact_decision_references_truncated) box.append(element('p', 'Showing ' + refs.length + ' of ' + entry.artifact_decision_references_total + ' decision references.'));
    return box;
  }
  async function request(operation, arguments_) {
    let response, envelope;
    try {
      response = await fetch(config.base + '/' + operation, {
        method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-Vivac-Token': config.token },
        body: JSON.stringify(Object.assign({ operation }, arguments_ || {}))
      });
      envelope = await response.json();
    } catch (_) {
      throw new Error(operation === 'capture' ? 'The response was not received. The delivery may have been recorded; inspect revisions before retrying.' : 'The response was not received. Inspect the current state before retrying.');
    }
    const result = envelope.result || envelope;
    if (result.error || envelope.error) {
      const error = new Error(result.error || envelope.error); error.responseReceived = true; throw error;
    }
    if (!response.ok && !(operation === 'check' && result.retained && result.source)) throw new Error('Artifact operation failed. Run vivac artifacts check --store to inspect retained custody.');
    return result;
  }
  function saveFragment() {
    const params = new URLSearchParams();
    if (selected) { params.set('artifact', selected.artifact); params.set('revision', selected.revision); }
    if ($('scope').value === 'from' && $('from').value.trim()) params.set('from', $('from').value.trim());
    history.replaceState(null, '', '#' + params.toString());
  }
  function inventory() {
    $('rows').replaceChildren();
    const query = $('search').value.toLowerCase();
    const shown = page.filter(entry => [entry.source, entry.description, entry.producer, entry.producer_node?.alias, entry.producer_node?.title].join(' ').toLowerCase().includes(query));
    shown.forEach(entry => {
      const row = element('tr'), delivery = element('th'); delivery.scope = 'row';
      const open = button(basename(entry.source), () => openArtifact(entry.artifact, entry.revision, entry.revision)); open.className = 'file-link';
      delivery.append(open, element('p', descriptionText(entry), 'delivery-description'), producer(entry), element('p', deliveryLabel(entry), 'small muted'));
      delivery.append(previewAction(entry, 'Preview latest revision'));
      const identity = element('td'); identity.append(element('strong', short(entry.revision) + ' · Latest'), element('p', entry.source, 'small muted'));
      const refs = element('td'); refs.append(inventoryReferences(entry));
      const integrity = element('td'); integrity.append(element('span', observationText(observationFor(entry)), 'status'), element('p', 'Check a selected revision', 'small muted'));
      row.append(delivery, identity, refs, integrity); $('rows').append(row);
    });
    if (!shown.length) { const row = element('tr'), cell = element('td', page.length ? 'No deliveries match this page search.' : 'No deliveries in this scope.'); cell.colSpan = 4; row.append(cell); $('rows').append(row); }
  }
  async function loadList(at) {
    const generation = ++listGeneration; offset = at; next = null;
    $('prev-page').disabled = true; $('next-page').disabled = true;
    note('Loading delivery metadata…');
    const args = { limit: 20, offset: at };
    if ($('scope').value === 'all') args.all = true;
    if ($('scope').value === 'from') {
      if (!$('from').value.trim()) { note('Enter a node alias or ID.'); return; }
      args.from = $('from').value.trim();
    }
    try {
      const result = await request('list', args);
      if (generation !== listGeneration) return;
      page = result.artifacts; next = result.next_offset; inventory();
      note('Showing ' + page.length + ' of ' + result.total + ' deliveries. Search applies to this page. Metadata only; bytes have not been checked.');
    } catch (error) { if (generation === listGeneration) { page = []; inventory(); note(error.message); } }
    finally { if (generation === listGeneration) { $('prev-page').disabled = at === 0; $('next-page').disabled = next == null; } }
  }
  async function openArtifact(artifact, revision, latestRevision) {
    const generation = ++detailGeneration;
    selected = null; latest = latestRevision || null; historyOffset = 0;
    $('inventory').hidden = true; $('detail').hidden = false;
    $('revision-content').textContent = 'Loading revision metadata…'; $('revisions').replaceChildren(); note('');
    try {
      const result = await request('show', { artifact, limit: 20, offset: 0 });
      if (generation !== detailGeneration) return;
      revisions = result.revisions; historyNext = result.next_offset;
      if (result.latest_revision) latest = result.latest_revision;
      selected = revisions.find(entry => entry.revision === revision) || null;
      if (!selected && revision) {
        const exact = await request('show', { artifact, revision, limit: 20, offset: 0 });
        if (generation !== detailGeneration) return;
        selected = exact.revisions[0];
      }
      if (!selected && latest) {
        selected = revisions.find(entry => entry.revision === latest);
        if (!selected) {
          const exact = await request('show', { artifact, revision: latest, limit: 20, offset: 0 });
          if (generation !== detailGeneration) return;
          selected = exact.revisions[0];
        }
      }
      if (!selected) selected = revisions[0];
      if (!selected) throw new Error('No recorded revision is available. Run vivac artifacts check --store.');
      renderRevision();
    } catch (error) { if (generation === detailGeneration) { $('revision-content').textContent = error.message; note(error.message); } }
  }
  function revisionLabel(entry) {
    return short(entry.revision) + (entry.revision === latest ? ' · Latest' : '') + (entry.revision === selected.revision ? ' · Selected' : '');
  }
  async function historyPage(at) {
    if (historyLoading) return;
    historyLoading = true;
    if (historyPrevious) historyPrevious.disabled = true;
    if (historyFollowing) historyFollowing.disabled = true;
    const generation = ++detailGeneration, artifact = selected.artifact;
    $('revisions').setAttribute('aria-busy', 'true');
    try {
      const result = await request('show', { artifact, limit: 20, offset: at });
      if (generation !== detailGeneration) return;
      revisions = result.revisions; historyOffset = at; historyNext = result.next_offset; renderRail();
    } catch (error) { if (generation === detailGeneration) note(error.message); }
    finally {
      historyLoading = false;
      if (generation === detailGeneration) {
        $('revisions').setAttribute('aria-busy', 'false');
        if (historyPrevious) historyPrevious.disabled = historyOffset === 0;
        if (historyFollowing) historyFollowing.disabled = historyNext == null;
      }
    }
  }
  function renderRail() {
    $('revisions').setAttribute('aria-busy', 'false');
    $('revisions').replaceChildren(element('h3', 'Revision history'));
    revisions.forEach(entry => {
      const pick = button(revisionLabel(entry), () => { if (writing) return; ++detailGeneration; selected = entry; note(''); renderRevision(); });
      pick.append(element('span', descriptionText(entry), 'revision-description small muted'));
      pick.setAttribute('aria-pressed', entry.revision === selected.revision); $('revisions').append(pick);
    });
    const previous = button('Previous revisions', () => historyPage(Math.max(0, historyOffset - 20)));
    const following = button('Next revisions', () => historyPage(historyNext));
    previous.disabled = historyOffset === 0; following.disabled = historyNext == null;
    historyPrevious = previous; historyFollowing = following;
    $('revisions').append(previous, following);
  }
  function renderRevision() {
    renderRail(); saveFragment();
    const entry = selected, box = $('revision-content'); box.replaceChildren();
    selectedLabel = element('p', revisionLabel(entry), 'mono');
    box.append(element('h2', basename(entry.source)), selectedLabel, element('p', descriptionText(entry), 'delivery-description'), element('p', entry.source, 'muted'), element('p', deliveryLabel(entry), 'small muted'));
    const facts = element('div', undefined, 'facts');
    facts.append(element('p', entry.size + ' bytes'), element('p', 'Digest: ' + entry.digest, 'mono'), element('p', 'Producer'), producer(entry)); box.append(facts);
    if (entry.delivery_type === 'sqlite_application') {
      box.append(element('p', 'Database profile: ' + entry.profile, 'small muted'),
        element('p', 'Runtime pinned to this revision: ' + entry.runtime, 'small muted'),
        element('p', 'Runtime SHA-256: ' + entry.runtime_sha256, 'mono'),
        element('p', 'Updating vivac does not replace this revision’s runtime. Restore includes its complete runtime and license notices.', 'small muted'),
        element('p', 'The retained database is a logical export, not a byte-for-byte copy of the source. Changes made in the preview stay in browser memory; they do not update this retained revision.', 'notice'));
    }
    box.append(element('h3', 'References to this revision'), references(entry), element('p', 'A reference is not inferred approval. Integrity checks do not approve a delivery.', 'small muted'));
    const status = element('div', observationText(observationFor(entry)), 'notice'); status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite'); box.append(status);
    const actions = element('div', undefined, 'actions');
    const check = button('Check selected revision', async () => {
      const generation = detailGeneration; check.disabled = true; status.textContent = 'Checking retained bytes and source…';
      try {
        const result = await request('check', { artifact: entry.artifact, revision: entry.revision });
        if (generation !== detailGeneration || selected !== entry) return;
        const observation = rememberObservation(entry, result);
        status.textContent = observation ? observationText(observation) : observationText(observationFor(entry)) + ' The check response could not be validated.';
        inventory();
      } catch (error) { if (generation === detailGeneration && selected === entry) status.textContent = observationText(observationFor(entry)) + ' ' + error.message; }
      finally { check.disabled = false; }
    });
    const capture = button('Capture a new revision', () => captureDialog(true));
    capture.disabled = !latest;
    actions.append(check, button('Restore selected revision', () => restoreDialog(entry)), capture);
    actions.append(isBundle(entry) ? element('p', 'Bundle download is not supported. Restore the complete selected revision to a new directory.', 'small muted') : nativeForm(entry, 'download', 'Download selected revision'), previewAction(entry, 'Preview selected revision', true)); box.append(actions);
    if (isBundle(entry)) {
      const members = element('section', undefined, 'bundle-members');
      box.append(members); memberPage(entry, members, 0);
    }
  }
  async function memberPage(entry, box, offset) {
    const generation = detailGeneration;
    box.textContent = 'Loading member metadata…';
    box.setAttribute('aria-busy', 'true');
    try {
      const result = await request('show', { artifact: entry.artifact, revision: entry.revision, limit: 20, offset });
      if (generation !== detailGeneration || selected !== entry) return;
      const members = result.members || [];
      box.replaceChildren(element('h3', 'Members of this revision'), element('p', 'Metadata only; bytes have not been checked. Check selected revision verifies the complete bundle, regardless of this page.', 'small muted'));
      if (result.media_review && result.media_review.profile === 'reviewed-media-v1') {
        box.append(element('p', 'Prior human media review is declared for these member digests. Automatic inspection covers structure and accessible text, not pixels or speech. The declaration does not imply approval of the work.', 'notice'));
      }
      const table = element('table'), header = element('tr');
      for (const label of ['Member path', 'Media type', 'Bytes', 'SHA-256']) header.append(element('th', label));
      const head = element('thead'); head.append(header); table.append(head);
      const body = element('tbody');
      for (const member of members) {
        const row = element('tr');
        row.append(element('td', member.path), element('td', member.media_type), element('td', member.bytes), element('td', member.sha256, 'mono')); body.append(row);
      }
      table.append(body);
      const wrap = element('div', undefined, 'table-wrap'); wrap.append(table);
      const previous = button('Previous members', () => memberPage(entry, box, Math.max(0, offset - 20)));
      const following = button('Next members', () => memberPage(entry, box, result.members_next_offset));
      previous.disabled = offset === 0; following.disabled = result.members_next_offset == null;
      box.append(wrap, element('p', 'Showing ' + members.length + ' of ' + result.total_members + ' members.'), previous, following);
    } catch (error) { if (generation === detailGeneration && selected === entry) box.textContent = error.message; }
    finally { box.setAttribute('aria-busy', 'false'); }
  }
  function nativeForm(entry, operation, label) {
    const form = element('form'); form.method = 'post'; form.action = config.base + '/' + operation; form.target = '_blank'; form.rel = 'noopener';
    for (const [name, value] of Object.entries({ artifact: entry.artifact, revision: entry.revision, token: config.token })) { const input = element('input'); input.type = 'hidden'; input.name = name; input.value = value; form.append(input); }
    const submit = element('button', label); submit.type = 'submit'; form.append(submit); return form;
  }
  function previewAction(entry, label, explainFormat = false) {
    if (!isBundle(entry) && !/\.html?$/i.test(entry.source)) return element(explainFormat ? 'p' : 'span', explainFormat ? 'Only retained HTML files can be previewed.' : '', 'small muted');
    if (!capabilities) return element('p', 'Checking preview support…', 'small muted');
    if (capabilities.execution?.provided !== true || capabilities.execution?.scope !== 'retained_html_preview') return element('p', 'Preview is not supported by this engine.', 'small muted');
    if (isBundle(entry) && capabilities.execution.bundle_preview !== true) return element('p', 'Bundle preview is not supported by this engine.', 'small muted');
    if (entry.delivery_type === 'sqlite_application' && capabilities.delivery_types?.sqlite_application?.supported !== true) return element('p', 'SQLite application preview is not supported by this engine.', 'small muted');
    return nativeForm(entry, 'preview', label);
  }
  function modal(title) {
    dialogFocus = document.activeElement;
    $('dialog-content').replaceChildren(element('h2', title));
    $('dialog').showModal();
  }
  function input(id, label, value) {
    const wrapper = element('label', label), field = element('input'); field.id = id; field.value = value || ''; field.required = true;
    wrapper.append(field); $('dialog-content').append(wrapper); return field;
  }
  function dialogActions(label, callback) {
    const status = element('p', '', 'dialog-message'); status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite');
    const confirm = button(label, async () => { if (writing) return; await callback(confirm, status); });
    confirm.id = label === 'Restore selected revision' ? 'restore-confirm' : 'capture-confirm';
    const cancel = button('Cancel', () => { if (!writing) $('dialog').close(); });
    const actions = element('div', undefined, 'actions'); actions.append(cancel, confirm); $('dialog-content').append(status, actions);
    return { confirm, cancel };
  }
  async function getCapabilities() { if (!capabilities) capabilities = await request('capabilities'); return capabilities; }
  async function captureDialog(continuing) {
    const entry = selected;
    let previous = latest, needsHeadApproval = false;
    if (continuing && !previous) { note('The latest revision is unknown. Return to inventory before capturing a new revision.'); return; }
    let caps;
    try { caps = await getCapabilities(); } catch (error) { note(error.message); return; }
    modal(continuing ? 'Capture a new revision' : 'Capture a delivery');
    const continuation = element('p', '', 'notice');
    function describeContinuation() { continuation.textContent = 'Continue latest revision ' + short(previous) + (entry.revision !== previous ? '; the selected older revision stays unchanged.' : '.'); }
    if (continuing) { describeContinuation(); $('dialog-content').append(continuation); }
    const structureLabel = element('label', 'Delivery structure'), types = element('select'); types.id = 'capture-type';
    Object.entries(caps.delivery_types).forEach(([name, capability]) => { const available = (name === 'file' || bundleType(name)) && capability.supported; const option = element('option', typeLabel(name) + (available ? ' · supported' : ' · ' + (capability.reason || 'Not supported by this capture surface.'))); option.value = name; option.disabled = !available; types.append(option); });
    types.value = continuing && isBundle(entry) ? entry.delivery_type : 'file'; types.disabled = continuing; structureLabel.append(types); $('dialog-content').append(structureLabel);
    const path = input('capture-path', bundleType(types.value) ? 'Bundle selection file' : 'Project-relative source path', continuing ? (isBundle(entry) ? entry.selection : entry.source) : '');
    const selectionHelp = element('p', bundleType(types.value) ? 'Use a project-relative JSON file declaring the complete delivery.' : '', 'small muted');
    $('dialog-content').append(selectionHelp);
    types.addEventListener('change', () => {
      path.value = '';
      const label = path.parentNode;
      if (label) { label.textContent = bundleType(types.value) ? 'Bundle selection file' : 'Project-relative source path'; label.append(path); }
      selectionHelp.textContent = bundleType(types.value) ? 'Use a project-relative JSON file declaring the complete delivery.' : '';
      describeLimits();
    });
    const from = input('capture-producer', 'Producer node alias or ID', $('scope').value === 'from' ? $('from').value.trim() : '');
    const description = input('capture-description', 'Description', continuing ? entry.description : '');
    description.maxLength = 560;
    description.setAttribute('aria-describedby', 'capture-description-help');
    const descriptionHelp = element('p', 'Briefly describe what this delivery does (up to 280 characters).', 'small muted');
    descriptionHelp.id = 'capture-description-help'; $('dialog-content').append(descriptionHelp);
    const limits = element('p', '', 'small muted');
    function describeLimits() {
      const capability = caps.delivery_types[types.value];
      limits.textContent = capability ? 'Formats: ' + formats(capability.formats) + '. Maximum ' + (bundleType(types.value) ? 'revision' : 'file') + ' size: ' + sizeLabel(capability.max_bytes) + '.' : '';
    }
    describeLimits(); $('dialog-content').append(limits);
    $('dialog-content').append(element('p', 'Capture retains a delivery locally and records its producer. It does not record approval.'));
    const controls = dialogActions('Capture delivery', async (confirm, status) => {
      if (needsHeadApproval) return;
      if (!(types.value === 'file' || bundleType(types.value)) || !caps.delivery_types[types.value]?.supported) { status.textContent = 'This delivery structure is not supported.'; return; }
      if (!path.value.trim() || !from.value.trim()) { status.textContent = 'Source path and producer node are required.'; (!path.value.trim() ? path : from).focus(); return; }
      const descriptionValue = description.value.trim();
      if (!descriptionValue) { status.textContent = 'A description is required.'; description.focus(); return; }
      if (/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/.test(description.value) || [...descriptionValue].length > 280) { status.textContent = 'Description must be a single line of up to 280 characters.'; description.focus(); return; }
      const args = { [bundleType(types.value) ? 'bundle' : 'path']: path.value.trim(), from: from.value.trim(), description: descriptionValue };
      if (continuing) { args.artifact = entry.artifact; args.previous = previous; }
      writing = true; confirm.disabled = true; controls.cancel.disabled = true; status.textContent = 'Capturing delivery…';
      try { const result = await request('capture', args); const delivery = result.delivery; $('dialog').close(); note('Delivery captured.'); await openArtifact(delivery.artifact, delivery.revision, delivery.revision); }
      catch (error) {
        status.textContent = error.message;
        if (continuing && error.responseReceived) {
          try {
            const current = await request('show', { artifact: entry.artifact, limit: 20, offset: 0 });
            if (current.latest_revision && current.latest_revision !== previous) {
              const currentHead = current.latest_revision;
              latest = currentHead; needsHeadApproval = true;
              renderRail(); if (selectedLabel) selectedLabel.textContent = revisionLabel(selected);
              status.append(element('p', 'Latest changed to ' + short(currentHead) + '. Review the current head before another capture.'));
              status.append(button('Continue from latest revision ' + short(currentHead), () => {
                previous = currentHead; needsHeadApproval = false; describeContinuation();
                status.textContent = 'Latest revision ' + short(currentHead) + ' selected as the previous revision. Capture delivery to retry explicitly.';
                confirm.disabled = false; path.focus();
              }));
            }
          } catch (refreshError) { status.append(element('p', 'The latest revision could not be refreshed. ' + refreshError.message)); }
        }
        path.focus();
      }
      finally { writing = false; confirm.disabled = needsHeadApproval; controls.cancel.disabled = false; }
    }); path.focus();
  }
  function restoreDialog(entry) {
    modal('Restore ' + short(entry.revision));
    $('dialog-content').append(element('p', isBundle(entry) ? 'Create a new complete directory from this exact retained revision. The destination must not already exist.' : 'Create a new project file from this exact retained revision. The destination must not already exist.'));
    const path = input('restore-path', 'New project-relative destination');
    const controls = dialogActions('Restore selected revision', async (confirm, status) => {
      if (!path.value.trim()) { status.textContent = 'A new destination is required.'; path.focus(); return; }
      writing = true; confirm.disabled = true; controls.cancel.disabled = true;
      try { const result = await request('restore', { artifact: entry.artifact, revision: entry.revision, to: path.value.trim() }); status.textContent = 'Restored selected revision to ' + result.destination + '.'; }
      catch (error) { status.textContent = error.message; path.focus(); }
      finally { writing = false; confirm.disabled = false; controls.cancel.disabled = false; }
    }); path.focus();
  }
  $('dialog').addEventListener('cancel', event => { if (writing) event.preventDefault(); });
  $('dialog').addEventListener('close', () => { if (dialogFocus) dialogFocus.focus(); });
  $('capture').addEventListener('click', () => captureDialog(false));
  $('capabilities').addEventListener('click', async () => {
    try {
      const caps = await getCapabilities(); modal('Supported types');
      Object.entries(caps.delivery_types).forEach(([type, capability]) => $('dialog-content').append(element('h3', typeLabel(type)), element('p', capability.supported ? 'Supported. Formats: ' + formats(capability.formats) + '. Maximum size: ' + sizeLabel(capability.max_bytes) + '.' : capability.reason)));
      let execution = 'Execution: support unknown.';
      if (caps.execution?.provided === false) execution = 'Execution: not provided by this engine.';
      if (caps.execution?.provided === true) execution = caps.execution.scope === 'retained_html_preview' ? 'Execution: restricted preview of retained HTML revisions. Complete network isolation is not guaranteed.' + (caps.execution.bundle_preview === true ? ' Bundle preview serves declared HTML, CSS and JavaScript members.' : '') : 'Execution: not supported in this view.';
      $('dialog-content').append(element('p', execution), button('Close', () => $('dialog').close()));
    } catch (error) { note(error.message); }
  });
  $('back').addEventListener('click', () => { if (writing) return; ++detailGeneration; selected = null; $('detail').hidden = true; $('inventory').hidden = false; saveFragment(); loadList(offset); });
  $('search').addEventListener('input', inventory);
  $('scope').addEventListener('change', () => { $('from-label').hidden = $('scope').value !== 'from'; $('from').hidden = $('scope').value !== 'from'; $('from').disabled = $('scope').value !== 'from'; loadList(0); });
  $('from').addEventListener('change', () => { if ($('scope').value === 'from') loadList(0); });
  $('prev-page').addEventListener('click', () => loadList(Math.max(0, offset - 20)));
  $('next-page').addEventListener('click', () => { if (next != null) loadList(next); });
  if (fragment.get('from')) { $('scope').value = 'from'; $('from').value = fragment.get('from'); $('from-label').hidden = false; $('from').hidden = false; $('from').disabled = false; }
  loadList(0);
  getCapabilities().then(() => { inventory(); if (selected) renderRevision(); }).catch(error => note(error.message));
  if (fragment.get('artifact')) openArtifact(fragment.get('artifact'), fragment.get('revision'), null);
})();
