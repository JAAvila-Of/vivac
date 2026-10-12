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
const revision = (id, source = 'layout.html') => ({ artifact: 'artifact', revision: id, source, producer: 'node', size: 50, digest: 'abc', decision_references: [] });
const latest = revision('r2');
latest.artifact_decision_references = [{ revision: 'r1', node: 'decision', alias: 'd1', kind: 'decision', state: 'closed' }];
latest.artifact_decision_references_total = 1;
let head = 'r2', captureNetworkFailure = false, captureConflict = false, deliveryHead = 'r3';
const caps = { delivery_types: { file: { supported: true, formats: ['utf8_text'], max_bytes: 99 }, bundle: { supported: false, reason: 'Not supported' } }, execution: { provided: false } };
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
  if (body.operation === 'check') result = { retained: { status: 'matches' }, source: { status: 'foreign_site' }, checked_at: 'actual-time' };
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
const context = { document, fetch, URLSearchParams, location: { hash: '#artifact=artifact&revision=r1' }, history: { replaceState() {} }, console };
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/web/artifacts.js'), 'utf8'), context);
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const all = root => [root, ...root.children.flatMap(all)];
const button = text => all(document.elements['revision-content']).find(e => e.tagName === 'BUTTON' && e.textContent === text);
(async () => {
  await settle();
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
  document.elements.dialog.close();
  assert.equal(document.activeElement, document.elements.capture, 'Cancelling toolbar capture must restore its focus');
  assert.match(document.elements['revision-content'].textContent, /r1.*Selected/, 'Cancelling toolbar capture must preserve the old revision selection');
  await button('Check selected revision').click(); await settle();
  assert.equal(requests.at(-1).body.revision, 'r1');
  assert.match(document.elements['revision-content'].textContent, /unknown.*other site/i);
  const form = all(document.elements['revision-content']).find(e => e.tagName === 'FORM');
  assert.equal(form.method, 'post'); assert.equal(form.target, '_blank');
  assert.equal(form.children.find(e => e.name === 'revision').value, 'r1');
  await button('Restore selected revision').click();
  document.elements['restore-path'].value = 'occupied.html';
  await document.elements['restore-confirm'].click(); await settle();
  assert.equal(requests.at(-1).body.revision, 'r1');
  assert.equal(document.elements['restore-path'].value, 'occupied.html');
  assert.match(document.elements['dialog-content'].textContent, /already exists/);
  assert.equal(document.activeElement, document.elements['restore-path']);
  document.elements.dialog.close();
  await button('Capture a new revision').click(); await settle();
  document.elements['capture-path'].value = 'new.html'; document.elements['capture-producer'].value = 't1';
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
  assert.match(document.elements['revision-content'].textContent, /r3.*Selected/);
  assert(!document.elements['revision-content'].textContent.includes('actual-time'), 'A capture must clear transient checks');
  caps.delivery_types.file.max_bytes = 1572864;
  await button('Capture a new revision').click(); await settle();
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
