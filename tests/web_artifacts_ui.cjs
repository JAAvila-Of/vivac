const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

class Element {
  constructor(tag, document) { this.tagName = tag.toUpperCase(); this.document = document; this.children = []; this.listeners = {}; this.attributes = {}; this.value = ''; this.hidden = false; this.disabled = false; this._text = ''; }
  set textContent(value) { this._text = String(value); this.children = []; }
  set innerHTML(value) { throw new Error('Active HTML rendering is forbidden: ' + value); }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  set id(value) { this.document.elements[value] = this; this._id = value; }
  get id() { return this._id; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this._text = ''; this.children = children; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  click() { if (!this.disabled) return this.listeners.click?.({ target: this, preventDefault() {} }); }
  focus() { this.document.activeElement = this; }
  showModal() { this.open = true; }
  close() { this.open = false; this.listeners.close?.(); }
  reportValidity() { return !!this.value; }
}
const document = { elements: {}, createElement(tag) { return new Element(tag, this); }, getElementById(id) { return this.elements[id]; } };
for (const id of ['bootstrap', 'inventory', 'detail', 'rows', 'revision-content', 'revisions', 'message', 'scope', 'from-label', 'from', 'search', 'next-page', 'prev-page', 'capture', 'capabilities', 'back', 'dialog', 'dialog-content']) {
  const element = document.createElement('div'); element.id = id;
}
document.elements.bootstrap.textContent = JSON.stringify({ token: 'abc', base: '/p/project/artifacts', project_base: '/p/project' });
document.elements.scope.value = 'all';
const requests = [];
const revision = (id, source = 'layout.html') => ({ artifact: 'artifact', revision: id, site: 'site-a', source, producer: 'node', description: id === 'r1' ? null : 'Explore <img onerror=attack> by month.', size: 50, digest: 'a'.repeat(64), decision_references: [] });
const latest = revision('r2');
latest.artifact_decision_references = [{ revision: 'r1', node: 'decision', alias: 'd1', kind: 'decision', state: 'closed' }];
latest.artifact_decision_references_total = 1;
let head = 'r2', captureNetworkFailure = false, captureConflict = false, deliveryHead = 'r3';
const caps = { delivery_types: { file: { supported: true, formats: ['utf8_text'], max_bytes: 99 }, bundle: { supported: false, reason: 'Not supported' } }, execution: { provided: true, scope: 'retained_html_preview' } };
let pendingList = null;
const fetch = async (url, options) => {
  const body = JSON.parse(options.body); requests.push({ url, body, options });
  let result;
  if (body.operation === 'capabilities') result = caps;
  if (body.operation === 'list') {
    if (pendingList) return new Promise(resolve => { pendingList.push(resolve); });
    result = { artifacts: [latest], total: 1, next_offset: null };
  }
  if (body.operation === 'show') result = { revisions: body.revision ? [revision(body.revision)] : [revision('r1'), latest, ...(head === 'r3' ? [revision('r3')] : [])], latest_revision: head, total: 2, next_offset: null };
  if (body.operation === 'check') result = { artifact: body.artifact, revision: body.revision, retained: { status: 'matches' }, source: { status: 'foreign_site' }, checked_at: '2026-10-13T03:30:04Z' };
  if (body.operation === 'restore') result = { error: 'Destination already exists.' };
  if (body.operation === 'capture') {
    if (captureNetworkFailure === true) throw new TypeError('Failed to fetch');
    if (captureConflict) { head = 'r4'; result = { error: 'The expected previous revision is no longer current.' }; }
    else { head = deliveryHead; result = { delivery: revision(deliveryHead) }; }
  }
  return { ok: body.operation !== 'check', json: async () => {
    if (body.operation === 'capture' && captureNetworkFailure === 'parse') throw new SyntaxError('Invalid JSON');
    return { exit_code: result.error ? 1 : 0, result };
  } };
};
const stored = new Map();
const sessionStorage = { getItem: key => stored.get(key) || null, setItem: (key, value) => stored.set(key, value) };
const context = { document, fetch, sessionStorage, URLSearchParams, location: { hash: '#artifact=artifact&revision=r1' }, history: { replaceState() {} }, console };
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/web/artifacts.js'), 'utf8'), context);
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const all = root => [root, ...root.children.flatMap(all)];
const button = text => all(document.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === text);
(async () => {
  await settle();
  const previewForm = (root, label) => all(root).find(e => e.tagName === 'FORM' && e.children.some(child => child.textContent === label));
  const assertPreview = (form, expectedRevision) => {
    assert(form, 'HTML revisions must offer an exact native preview form');
    assert.equal(form.action, '/p/project/artifacts/preview');
    assert.equal(form.method, 'post'); assert.equal(form.target, '_blank'); assert.equal(form.rel, 'noopener');
    assert.deepEqual(Object.fromEntries(form.children.filter(e => e.tagName === 'INPUT').map(e => [e.name, e.value])), { artifact: 'artifact', revision: expectedRevision, token: 'abc' });
  };
  assert(document.elements.rows.textContent.includes(latest.description), 'Inventory must show the latest description as plain text');
  assert.match(document.elements['revision-content'].textContent, /No description recorded\./, 'Legacy revisions must not inherit the latest description');
  assert(!document.elements['revision-content'].textContent.includes(latest.description));
  document.elements.search.value = 'by month'; document.elements.search.listeners.input();
  assert(document.elements.rows.textContent.includes(latest.description), 'Search must include descriptions');
  document.elements.search.value = ''; document.elements.search.listeners.input();
  assertPreview(previewForm(document.elements.rows, 'Preview latest revision'), 'r2');
  assertPreview(previewForm(document.elements['revision-content'], 'Preview selected revision'), 'r1');
  await document.elements.capabilities.click(); await settle();
  assert.match(document.elements['dialog-content'].textContent, /Execution: restricted preview of retained HTML revisions\. Complete network isolation is not guaranteed\./);
  assert(!document.elements['dialog-content'].textContent.includes('retained_html_preview'));
  document.elements.dialog.close();
  assert.match(document.elements.revisions.textContent, /r1.*Selected/);
  assert.match(document.elements.revisions.textContent, /r2.*Latest/, 'A direct old revision link must identify the authoritative latest head');
  assert.match(document.elements.rows.textContent, /r1.*d1.*Earlier/, 'Inventory must retain earlier decision references');
  head = 'r3';
  await all(document.elements.rows).find(e => e.tagName === 'BUTTON').click(); await settle();
  assert.match(document.elements.revisions.textContent, /r3.*Latest/, 'Show must supersede an old inventory head');
  await all(document.elements.revisions).find(e => e.tagName === 'BUTTON' && e.textContent.includes('r1')).click(); await settle();
  assert.match(document.elements.revisions.textContent, /r3.*Latest/);
  document.elements.scope.value = 'focus'; document.elements.scope.listeners.change(); await settle();
  assert.match(document.elements['revision-content'].textContent, /r1.*Selected/, 'Scope changes must preserve the old revision selection');
  document.elements.capture.focus(); await document.elements.capture.click(); await settle();
  assert.match(document.elements['dialog-content'].textContent, /UTF-8 text/);
  assert.match(document.elements['dialog-content'].textContent, /99 bytes/);
  assert.equal(document.elements['capture-description'].value, '', 'A new identity must not inherit the selected artifact description');
  document.elements['capture-path'].value = 'new-delivery.html'; document.elements['capture-producer'].value = 't1';
  const newCaptureRequests = requests.length;
  await document.elements['capture-confirm'].click(); await settle();
  assert.equal(requests.length, newCaptureRequests, 'A new identity requires its own description before capture');
  assert.match(document.elements['dialog-content'].textContent, /A description is required\./);
  document.elements.dialog.close();
  assert.equal(document.activeElement, document.elements.capture, 'Cancelling toolbar capture must restore its focus');
  assert.match(document.elements['revision-content'].textContent, /r1.*Selected/, 'Cancelling toolbar capture must preserve the old revision selection');
  await button('Check selected revision').click(); await settle();
  assert.equal(requests.at(-1).body.revision, 'r1');
  assert.match(document.elements['revision-content'].textContent, /unknown.*other site/i);
  const checksBeforeReload = requests.filter(r => r.body.operation === 'check').length;
  async function reload(overrides = {}) {
    const copy = { elements: {}, createElement: document.createElement, getElementById: document.getElementById };
    for (const id of Object.keys(document.elements)) { const e = copy.createElement('div'); e.id = id; }
    copy.elements.bootstrap.textContent = JSON.stringify({ token: 'different-token', base: '/p/project/artifacts', project_base: overrides.project || '/p/project' });
    copy.elements.scope.value = 'all';
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/web/artifacts.js'), 'utf8'), {
      ...context, document: copy, sessionStorage: overrides.storage || sessionStorage,
      fetch: overrides.fetch || fetch, location: { hash: '#artifact=' + (overrides.artifact || 'artifact') + '&revision=' + (overrides.revision || 'r1') }
    });
    await settle(); return copy;
  }
  for (const [sensitivity, coverage, expected] of [
    ['detected', 'complete', /Sensitive content detected/],
    ['unknown', 'partial', /Sensitivity unknown.*Partial; some content was not inspected/],
    ['not_detected', 'complete', /No sensitive content detected in inspected text/]
  ]) {
    const report = { policy: 'allow_insecure', sensitivity, coverage, review_required: true };
    const riskView = await reload({ fetch: async (url, options) => {
      const body = JSON.parse(options.body);
      if (body.operation !== 'show') return fetch(url, options);
      const entry = { ...revision('r1'), inspection: report };
      return { ok: true, json: async () => ({ result: { revisions: [entry], latest_revision: 'r2', total: 1, next_offset: null } }) };
    } });
    const text = riskView.elements['revision-content'].textContent;
    assert.match(text, expected);
    assert.match(text, /Explicit permissive custody policy/);
    assert.match(text, /At capture: human review required.*Custody does not imply approval/);
    assert(!text.includes('content is safe'), 'Inspection must not claim safety');
  }
  for (const [execution, message] of [[{ provided: false }, 'Execution: not provided by this engine.'], [{ provided: true, scope: 'other' }, 'Execution: not supported in this view.'], [undefined, 'Execution: support unknown.']]) {
    caps.execution = execution;
    const unsupported = await reload();
    assert(!previewForm(unsupported.elements.rows, 'Preview latest revision'));
    assert(!previewForm(unsupported.elements['revision-content'], 'Preview selected revision'));
    assert.match(unsupported.elements['revision-content'].textContent, /Preview is not supported/);
    await unsupported.elements.capabilities.click(); await settle();
    assert(unsupported.elements['dialog-content'].textContent.includes(message));
    unsupported.elements.dialog.close();
  }
  caps.execution = { provided: true, scope: 'retained_html_preview' };
  const originalSource = latest.source; latest.source = 'layout.HTM';
  assert(previewForm((await reload({ revision: 'r2' })).elements.rows, 'Preview latest revision'), 'HTML suffix matching must ignore case');
  latest.source = originalSource;
  const bundleRequests = [];
  const bundleRevision = id => ({ ...revision(id, 'delivery-root'), artifact: 'bundle-artifact', delivery_type: 'bundle', entrypoint: 'pages/index.html', member_count: 21, selection: 'bundle-selection.json' });
  const bundleView = await reload({ artifact: 'bundle-artifact', fetch: async (url, options) => {
    const body = JSON.parse(options.body); bundleRequests.push(body);
    if (body.operation === 'capabilities') return { ok: true, json: async () => ({ result: { ...caps, delivery_types: { ...caps.delivery_types, bundle: { supported: true, formats: ['html', 'css', 'js', 'png', 'mp4'], max_bytes: 104857600, media: { max_revision_bytes: 10485760 } } }, execution: { ...caps.execution, bundle_preview: true } } }) };
    if (body.operation === 'list') return { ok: true, json: async () => ({ result: { artifacts: [bundleRevision('r2')], total: 1, next_offset: null } }) };
    if (body.operation === 'show') return { ok: true, json: async () => ({ result: { revisions: body.revision ? [bundleRevision(body.revision)] : [bundleRevision('r1'), bundleRevision('r2')], latest_revision: 'r2', total: 2, next_offset: null, members: body.offset ? [{ path: 'scripts/app.js', bytes: 17, sha256: 'b'.repeat(64), media_type: 'text/javascript' }] : Array.from({ length: 20 }, (_, i) => ({ path: 'styles/file' + i + '.css', bytes: 23, sha256: 'a'.repeat(64), media_type: 'text/css' })), total_members: 21, members_next_offset: body.offset ? null : 20 } }) };
    if (body.operation === 'capture') return { ok: true, json: async () => ({ result: { delivery: bundleRevision('r3') } }) };
    if (body.operation === 'check') return { ok: true, json: async () => ({ result: { artifact: body.artifact, revision: body.revision, retained: { status: 'matches' }, source: { status: 'missing' }, checked_at: '2026-10-14T03:30:04Z' } }) };
    return fetch(url, options);
  } });
  await settle();
  assert.match(bundleView.elements.rows.textContent, /Bundle.*pages\/index.html.*21 members/);
  assert.match(bundleView.elements['revision-content'].textContent, /Bundle.*pages\/index.html.*21 members/);
  const bundleForm = previewForm(bundleView.elements['revision-content'], 'Preview selected revision');
  assert(bundleForm, 'Bundle root names must not need an HTML extension');
  assert.equal(bundleForm.children.find(e => e.name === 'revision').value, 'r1');
  assert(!previewForm(bundleView.elements['revision-content'], 'Download selected revision'), 'Bundle download must not masquerade as a single file');
  assert.match(bundleView.elements['revision-content'].textContent, /Bundle download is not supported/);
  assert.match(bundleView.elements['revision-content'].textContent, /styles\/file0.css.*text\/css/);
  assert.match(bundleView.elements['revision-content'].textContent, /Metadata only/);
  await all(bundleView.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Next members').click(); await settle();
  assert.match(bundleView.elements['revision-content'].textContent, /scripts\/app.js.*text\/javascript/);
  assert.equal(bundleRequests.at(-1).revision, 'r1');
  assert.equal(bundleRequests.at(-1).offset, 20);
  await all(bundleView.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
  assert.equal(bundleRequests.at(-1).operation, 'check');
  assert.equal(bundleRequests.at(-1).revision, 'r1');
  assert(!Object.hasOwn(bundleRequests.at(-1), 'offset'), 'Integrity checks must cover the whole revision independently of the visible member page');
  await all(bundleView.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Capture a new revision').click(); await settle();
  assert.equal(bundleView.elements['capture-type'].value, 'bundle');
  assert.equal(bundleView.elements['capture-path'].value, 'bundle-selection.json');
  assert.match(bundleView.elements['dialog-content'].textContent, /Bundle selection file/);
  assert.match(bundleView.elements['dialog-content'].textContent, /Maximum revision size: 100 MiB/);
  assert.match(bundleView.elements['dialog-content'].textContent, /Revisions with media are limited to 10 MiB/);
  assert.equal(bundleView.elements['capture-description'].value, '', 'An old revision must not prefill its newer revision description');
  bundleView.elements['capture-producer'].value = 't1';
  bundleView.elements['capture-description'].value = '  Explore the bundled dashboard.  ';
  await bundleView.elements['capture-confirm'].click(); await settle();
  const bundleCapture = bundleRequests.find(body => body.operation === 'capture');
  assert.equal(bundleCapture.bundle, 'bundle-selection.json');
  assert.equal(bundleCapture.previous, 'r2');
  assert.equal(bundleCapture.description, 'Explore the bundled dashboard.');
  assert(!Object.hasOwn(bundleCapture, 'path'));
  const sqliteRequests = [];
  const sqliteRevision = id => ({ ...bundleRevision(id), delivery_type: 'sqlite_application', database: 'data/items.sqlite', profile: 'sqlite-standalone-v1', runtime: 'sqljs-asm-1.14.2', runtime_sha256: 'a5709877905821fcae8aeb6789c53302eb0ffc50473f81dc2bfbdd93a16f9b38' });
  const sqliteView = await reload({ artifact: 'bundle-artifact', fetch: async (url, options) => {
    const body = JSON.parse(options.body); sqliteRequests.push(body);
    const result = body.operation === 'capabilities'
      ? { ...caps, delivery_types: { ...caps.delivery_types, sqlite_application: { supported: true, formats: ['html', 'css', 'js', 'sqlite3'], max_bytes: 10485760 } }, execution: { ...caps.execution, bundle_preview: true } }
      : body.operation === 'list' ? { artifacts: [sqliteRevision('r2')], total: 1, next_offset: null }
      : body.operation === 'show' ? { revisions: body.revision ? [sqliteRevision(body.revision)] : [sqliteRevision('r1'), sqliteRevision('r2')], latest_revision: 'r2', total: 2, next_offset: null, members: [{ path: 'data/items.sqlite', bytes: 8192, sha256: 'b'.repeat(64), media_type: 'application/vnd.sqlite3' }], total_members: 1, members_next_offset: null }
      : body.operation === 'capture' ? { delivery: sqliteRevision('r3') } : {};
    return { ok: true, json: async () => ({ result }) };
  } });
  await settle();
  assert.match(sqliteView.elements.rows.textContent, /SQLite application.*pages\/index.html.*data\/items.sqlite/);
  assert.match(sqliteView.elements['revision-content'].textContent, /sqlite-standalone-v1/);
  assert.match(sqliteView.elements['revision-content'].textContent, /logical export.*not a byte-for-byte copy/i);
  assert.match(sqliteView.elements['revision-content'].textContent, /Runtime pinned to this revision: sqljs-asm-1\.14\.2/);
  assert.match(sqliteView.elements['revision-content'].textContent, /Runtime SHA-256: a5709877905821fcae8aeb6789c53302eb0ffc50473f81dc2bfbdd93a16f9b38/);
  assert.match(sqliteView.elements['revision-content'].textContent, /Updating vivac does not replace this revision’s runtime/);
  assert.match(sqliteView.elements['revision-content'].textContent, /Changes made in the preview stay in browser memory/);
  assert.match(sqliteView.elements['revision-content'].textContent, /data\/items.sqlite.*application\/vnd.sqlite3/);
  assert(previewForm(sqliteView.elements.rows, 'Preview latest revision'));
  assert(previewForm(sqliteView.elements['revision-content'], 'Preview selected revision'));
  assert(!previewForm(sqliteView.elements['revision-content'], 'Download selected revision'));
  await all(sqliteView.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Capture a new revision').click(); await settle();
  assert.equal(sqliteView.elements['capture-type'].value, 'sqlite_application');
  assert.equal(sqliteView.elements['capture-path'].value, 'bundle-selection.json');
  sqliteView.elements['capture-producer'].value = 't1';
  sqliteView.elements['capture-description'].value = 'Explore the inventory with its contained database.';
  await sqliteView.elements['capture-confirm'].click(); await settle();
  const sqliteCapture = sqliteRequests.find(body => body.operation === 'capture');
  assert.equal(sqliteCapture.bundle, 'bundle-selection.json');
  assert.equal(sqliteCapture.previous, 'r2');
  assert(!Object.hasOwn(sqliteCapture, 'path'));
  const twoRows = await reload({ fetch: async (url, options) => {
    if (JSON.parse(options.body).operation !== 'list') return fetch(url, options);
    return { ok: true, json: async () => ({ result: { artifacts: [latest, { ...revision('other-head'), artifact: 'other-artifact' }], total: 2, next_offset: null } }) };
  } });
  const rowForms = all(twoRows.elements.rows).filter(e => e.tagName === 'FORM');
  assert.equal(rowForms.length, 2);
  assert.equal(rowForms[1].children.find(e => e.name === 'artifact').value, 'other-artifact');
  assert.equal(rowForms[1].children.find(e => e.name === 'revision').value, 'other-head', 'Each row must preview its own exact latest revision');
  const htmlSource = latest.source; latest.source = 'layout.txt';
  const textOnly = await reload({ revision: 'r2' });
  assert(!previewForm(textOnly.elements.rows, 'Preview latest revision'));
  assert(!previewForm(textOnly.elements['revision-content'], 'Preview selected revision'));
  assert.match(textOnly.elements['revision-content'].textContent, /Only retained HTML/);
  latest.source = htmlSource;
  const reloaded = await reload();
  assert.match(reloaded.elements['revision-content'].textContent, /Last checked 2026-10-13T03:30:04Z/);
  assert.match(reloaded.elements['revision-content'].textContent, /Not rechecked since/);
  assert.equal(requests.filter(r => r.body.operation === 'check').length, checksBeforeReload, 'Reload must not read bytes automatically');
  const originalStored = new Map(stored);
  assert(![...stored.values()].join('').includes('different-token'));
  assert(![...stored.values()].join('').includes('layout.html'));
  for (const overrides of [{ project: '/p/other' }, { revision: 'r2' }, { storage: { getItem() { throw Error('blocked'); }, setItem() { throw Error('blocked'); } } }]) {
    const isolated = await reload(overrides);
    assert.match(isolated.elements['revision-content'].textContent, /Not checked/);
  }
  for (const value of ['not json', JSON.stringify({ version: 99, entries: [] }), JSON.stringify({ version: 1, entries: [{ retained: 'matches', source: 'matches', checked_at: 'bad' }] })]) {
    for (const key of stored.keys()) stored.set(key, value);
    assert.match((await reload()).elements['revision-content'].textContent, /Not checked/);
  }
  stored.clear(); originalStored.forEach((value, key) => stored.set(key, value));
  const cacheKey = [...stored.keys()][0];
  const checked = JSON.parse(stored.get(cacheKey)).entries[0];
  const put = entries => stored.set(cacheKey, JSON.stringify({ version: 1, entries }));
  for (const change of [{ site: 'site-b' }, { artifact: 'other' }, { digest: 'b'.repeat(64) }, { size: 51 }]) {
    put([{ ...checked, ...change }]);
    assert.match((await reload()).elements['revision-content'].textContent, /Not checked/, 'Pinned metadata must isolate observations');
  }
  for (const change of [{ retained: 'approved' }, { source: 'unknown' }, { checked_at: '2026-02-30T03:30:04Z' }, { size: -1 }, { digest: 'invalid' }, { token: 'must-not-load' }, { site: 'C:/absolute/path' }]) {
    put([{ ...checked, ...change }]);
    assert.match((await reload()).elements['revision-content'].textContent, /Not checked/, 'Malformed cached observations must be ignored');
  }
  for (const suffix of ['Z', '.000Z', '.1Z', '.12Z', '.123Z']) {
    const checkedAt = '2026-10-13T03:30:04' + suffix;
    put([{ ...checked, checked_at: checkedAt }]);
    assert((await reload()).elements['revision-content'].textContent.includes('Last checked ' + checkedAt), 'Valid fractional timestamps must remain historical observations');
  }
  for (const status of ['changed', 'missing', 'read_failed']) {
    put([{ ...checked, retained: status, source: status }]);
    assert.match((await reload()).elements['revision-content'].textContent, new RegExp('Retained: ' + status.replace('_', ' ') + '.*Last checked'));
  }
  put(Array.from({ length: 200 }, (_, i) => ({ ...checked, revision: 'cached-' + i })));
  const bounded = await reload();
  await all(bounded.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
  const boundedEntries = JSON.parse(stored.get(cacheKey)).entries;
  assert.equal(boundedEntries.length, 200);
  assert.equal(boundedEntries[0].revision, 'cached-1', 'Oldest observations must be evicted');
  assert.equal(boundedEntries.at(-1).revision, 'r1');
  for (const status of ['changed', 'missing', 'read_failed']) {
    stored.clear();
    const observed = await reload({ fetch: async (url, options) => {
      const body = JSON.parse(options.body);
      if (body.operation !== 'check') return fetch(url, options);
      return { ok: false, json: async () => ({ result: { artifact: body.artifact, revision: body.revision, retained: { status }, source: { status }, checked_at: '2026-10-13T03:30:04Z' } }) };
    } });
    await all(observed.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
    assert.match((await reload()).elements['revision-content'].textContent, new RegExp('Retained: ' + status.replace('_', ' ') + '.*Last checked'), 'Non-success check responses must retain observed failures');
  }
  put([checked]);
  const unreachable = await reload({ fetch: async (url, options) => {
    if (JSON.parse(options.body).operation === 'check') throw Error('offline');
    return fetch(url, options);
  } });
  await all(unreachable.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
  assert.match(unreachable.elements['revision-content'].textContent, /Last checked.*response was not received/);
  assert.deepEqual(JSON.parse(stored.get(cacheKey)).entries, [checked], 'Network errors must not invent an observation');
  let finishCheck;
  const delayed = await reload({ fetch: async (url, options) => {
    if (JSON.parse(options.body).operation === 'check') return new Promise(resolve => { finishCheck = resolve; });
    return fetch(url, options);
  } });
  all(delayed.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click();
  await settle(); await delayed.elements.back.click(); await settle();
  finishCheck({ ok: true, json: async () => ({ result: { artifact: 'artifact', revision: 'r1', retained: { status: 'missing' }, source: { status: 'missing' }, checked_at: '2026-10-13T03:30:05Z' } }) });
  await settle();
  assert.deepEqual(JSON.parse(stored.get(cacheKey)).entries, [checked], 'Late results after navigation must preserve generation guards');
  const blocked = await reload({ storage: { getItem() { throw Error('blocked'); }, setItem() { throw Error('blocked'); } } });
  await all(blocked.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
  await blocked.elements.back.click(); await settle();
  await all(blocked.elements.rows).find(e => e.tagName === 'BUTTON').click(); await settle();
  await all(blocked.elements.revisions).find(e => e.tagName === 'BUTTON' && e.textContent.includes('r1')).click(); await settle();
  assert.match(blocked.elements['revision-content'].textContent, /Last checked/, 'Blocked storage must preserve in-view observations');
  const inventoryChecked = await reload({ revision: 'r2' });
  await all(inventoryChecked.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Check selected revision').click(); await settle();
  await inventoryChecked.elements.back.click(); await settle();
  assert.match(inventoryChecked.elements.rows.textContent, /Last checked.*Not rechecked since/, 'Inventory must show its exact revision observation');
  const restoredObservation = document.elements['revision-content'].textContent;
  const form = all(document.elements['revision-content']).find(e => e.tagName === 'FORM');
  assert.equal(form.method, 'post'); assert.equal(form.target, '_blank');
  assert.equal(form.children.find(e => e.name === 'revision').value, 'r1');
  form.listeners.submit?.();
  assert.equal(document.elements['revision-content'].textContent, restoredObservation, 'Download must preserve historical observations');
  await button('Restore selected revision').click();
  assert.equal(document.elements['revision-content'].textContent, restoredObservation, 'Restore must preserve historical observations');
  document.elements['restore-path'].value = 'occupied.html';
  await document.elements['restore-confirm'].click(); await settle();
  assert.equal(requests.at(-1).body.revision, 'r1');
  assert.equal(document.elements['restore-path'].value, 'occupied.html');
  assert.match(document.elements['dialog-content'].textContent, /already exists/);
  assert.equal(document.activeElement, document.elements['restore-path']);
  document.elements.dialog.close();
  await button('Capture a new revision').click(); await settle();
  document.elements['capture-path'].value = 'new.html'; document.elements['capture-producer'].value = 't1';
  const description = document.elements['capture-description'];
  assert.equal(description.required, true);
  assert.equal(description.maxLength, 560, 'Native input limits must leave room for supplementary Unicode characters');
  assert.match(document.elements['dialog-content'].textContent, /Briefly describe what this delivery does \(up to 280 characters\)\./);
  const capturesBeforeValidation = requests.filter(r => r.body.operation === 'capture').length;
  for (const invalid of ['', '   ', 'x'.repeat(281), '😀'.repeat(281), '\nleading', 'trailing\t', 'line\nbreak', 'line\rbreak', 'tab\there', 'control\u007fhere', 'control\u0085here', 'line\u2028break', 'line\u2029break']) {
    description.value = invalid;
    await document.elements['capture-confirm'].click(); await settle();
    assert.equal(requests.filter(r => r.body.operation === 'capture').length, capturesBeforeValidation, 'Invalid descriptions must never submit capture');
    assert.equal(document.activeElement, description);
    assert.match(document.elements['dialog-content'].textContent, /A description is required\.|Description must be a single line of up to 280 characters\./);
  }
  description.value = '😀'.repeat(280);
  document.elements['capture-type'].value = 'bundle';
  const before = requests.length;
  await document.elements['capture-confirm'].click(); await settle();
  assert.equal(requests.length, before, 'Unsupported structures must never submit capture');
  captureNetworkFailure = true;
  document.elements['capture-type'].value = 'file';
  await document.elements['capture-confirm'].click(); await settle();
  assert.match(document.elements['dialog-content'].textContent, /may have been recorded/);
  assert.equal(document.elements['capture-path'].value, 'new.html');
  assert.equal(document.activeElement, document.elements['capture-path']);
  const attempts = requests.filter(request => request.body.operation === 'capture').length;
  await settle();
  assert.equal(requests.filter(request => request.body.operation === 'capture').length, attempts, 'An uncertain capture must not retry automatically');
  captureNetworkFailure = 'parse';
  await document.elements['capture-confirm'].click(); await settle();
  assert.match(document.elements['dialog-content'].textContent, /may have been recorded/);
  assert.equal(document.activeElement, document.elements['capture-path']);
  captureNetworkFailure = false;
  await document.elements['capture-confirm'].click(); await settle();
  const capture = requests.find(request => request.body.operation === 'capture');
  assert.equal(capture.body.previous, 'r3', 'An old selection must continue the authoritative latest head');
  assert.equal(capture.body.artifact, 'artifact');
  assert.equal(capture.body.description, '😀'.repeat(280), 'The limit must count Unicode characters rather than UTF-16 units');
  assert.match(document.elements['revision-content'].textContent, /r3.*Selected/);
  assert(!document.elements['revision-content'].textContent.includes('2026-10-13T03:30:04Z'), 'A new revision must not inherit another revision check');
  caps.delivery_types.file.max_bytes = 1572864;
  await button('Capture a new revision').click(); await settle();
  assert.equal(document.elements['capture-description'].value, revision('r3').description, 'Continuing a selected revision may prefill only that revision description');
  document.elements['capture-path'].value = 'next.html'; document.elements['capture-producer'].value = 't2';
  captureConflict = true;
  await document.elements['capture-confirm'].click(); await settle();
  assert.equal(document.elements['capture-confirm'].disabled, true, 'A changed head requires explicit acceptance before retry');
  assert.equal(document.elements['capture-path'].value, 'next.html');
  assert.equal(document.elements['capture-producer'].value, 't2');
  assert.equal(document.activeElement, document.elements['capture-path']);
  assert.match(document.elements['revision-content'].textContent, /r3.*Selected/, 'Conflict handling must preserve the selected revision');
  const conflictAttempts = requests.filter(request => request.body.operation === 'capture').length;
  const acceptHead = all(document.elements['dialog-content']).find(e => e.tagName === 'BUTTON' && e.textContent === 'Continue from latest revision r4');
  assert(acceptHead, 'The authoritative refreshed head must be available for review');
  await acceptHead.click(); await settle();
  assert.equal(requests.filter(request => request.body.operation === 'capture').length, conflictAttempts, 'Accepting a head must never submit capture');
  assert.match(document.elements['dialog-content'].textContent, /UTF-8 text/);
  assert.match(document.elements['dialog-content'].textContent, /1.5 MiB.*1572864 bytes/);
  captureConflict = false; deliveryHead = 'r5';
  await document.elements['capture-confirm'].click(); await settle();
  const lastCapture = requests.filter(request => request.body.operation === 'capture').at(-1);
  assert.equal(lastCapture.body.previous, 'r4', 'The explicit retry must use the accepted current head');
  assert.equal(lastCapture.body.path, 'next.html');
  await document.elements.back.click(); await settle();
  pendingList = [];
  document.elements.scope.value = 'focus'; document.elements.scope.listeners.change();
  document.elements.scope.value = 'all'; document.elements.scope.listeners.change();
  assert.equal(pendingList.length, 2);
  pendingList[1]({ ok: true, json: async () => ({ exit_code: 0, result: { artifacts: [revision('new', '<img onerror=attack>.html')], total: 1, next_offset: null } }) });
  await settle();
  pendingList[0]({ ok: true, json: async () => ({ exit_code: 0, result: { artifacts: [revision('stale', 'stale.html')], total: 1, next_offset: 20 } }) });
  await settle();
  assert.match(document.elements.rows.textContent, /<img onerror=attack>/);
  assert(!document.elements.rows.textContent.includes('stale.html'), 'A late old scope must not replace the current page');
  assert.equal(document.elements['next-page'].disabled, true);
  console.log('Artifact revision custody interaction checks passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
