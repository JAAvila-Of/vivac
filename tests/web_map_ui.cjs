const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const browsers = [
  process.env.CHROME_BIN,
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
];
const browser = browsers.find(candidate => candidate && fs.existsSync(candidate));
assert(browser, 'A local Chromium browser is required; set CHROME_BIN to its executable.');

const marker = '<img src=x onerror="window.injected=true">';
const parent = {
  a: 'g1', t: 'Parent', w: 'Root', p: null, r: 0, d: 0,
  k: 'goal', s: 'open', nt: [], rf: [], gv: [], bl: [], bn: 0,
  c: 1, tb: 1, ob: 1, op: '2026-10-11',
};
const child = {
  ...parent, a: 't2', t: 'Child', p: 0, r: 1, d: 25,
  ln: `Line ${marker} & "quoted"`,
  op: `Opened ${marker}`, cl: `Closed ${marker}`,
  rf: [`Ref ${marker}`, 'Second <svg onload="window.injected=true"></svg> & literal &lt;tag&gt;'],
  ap: [{artifact: 'a'.repeat(26), revision: 'r'.repeat(26), source: `Source ${marker}`}],
  ar: [{artifact: 'a'.repeat(26), revision: 's'.repeat(26), source: `Older ${marker}`}],
  gv: [`Rule ${marker}`, 'Other <script>window.injected=true</script>'],
};
// Parent aliases and counts also pass through the card's metadata rendering.
parent.a = `Parent ${marker}`;
child.c = `Children ${marker}`;
child.tb = `Below ${marker}`;
child.ob = `Open ${marker}`;
const payload = JSON.stringify({ project: 'fixture', stops: [parent, child] }).replace(/</g, '\\u003c');
const script = fs.readFileSync(path.join(__dirname, '../src/web/map.js'), 'utf8');
const css = fs.readFileSync(path.join(__dirname, '../src/web/web.css'), 'utf8');
const check = function (expected) {
  try {
    document.querySelector('[data-stop="1"]').click();
    const panel = document.getElementById('detail');
    const viewport = document.querySelector('.rail-viewport');
    const visibleRail = Array.from(document.querySelectorAll('svg.rails')).find(rail => rail.getBoundingClientRect().width > 0);
    const targetX = +visibleRail.dataset.origin + 25 * +visibleRail.dataset.step + 10;
    if (viewport.scrollLeft <= 0 || viewport.scrollLeft + viewport.clientWidth < targetX) throw new Error('Selection at level 26 is outside the rail window: ' + viewport.scrollLeft + ',' + viewport.clientWidth + ',' + viewport.scrollWidth + ',' + window.innerWidth);
    const row = document.querySelector('[data-stop="1"]');
    const rail = visibleRail;
    if (Math.abs(row.getBoundingClientRect().top + 16 - rail.getBoundingClientRect().top - 48) > 1) throw new Error('Rail and title rows are misaligned');
    if (viewport.clientWidth > 220) throw new Error('Initial rail window exceeds ten levels');
    if (location.hash !== '#' + encodeURIComponent('t2')) throw new Error('Selection hash was lost');
    const hash = location.hash;
    const earlier = panel.querySelector('[data-pan=earlier]');
    const later = panel.querySelector('[data-pan=later]');
    for (let i = 0; i < 4; i++) later.click();
    if (!later.disabled) throw new Error('Later-level control does not stop at the boundary');
    for (let i = 0; i < 4; i++) earlier.click();
    if (!earlier.disabled || viewport.scrollLeft !== 0) throw new Error('Earlier-level control does not stop at the boundary');
    if (location.hash !== hash) throw new Error('Panning changed the selected node');
    if (panel.querySelector('img, svg, script, iframe, object, embed')) {
      throw new Error('Metadata created active HTML elements');
    }
    if (!panel.textContent.includes('Produced here') || !panel.textContent.includes('Referenced here')) throw new Error('Artifact provenance is absent');
    const artifactLinks = panel.querySelectorAll('.artifact-links a');
    if (artifactLinks.length !== 2 || !artifactLinks[0].href.endsWith('#artifact=' + 'a'.repeat(26) + '&revision=' + 'r'.repeat(26)) || !artifactLinks[1].href.endsWith('&revision=' + 's'.repeat(26))) throw new Error('Exact artifact revisions were lost');
    if (!artifactLinks[0].textContent.includes(expected.artifactSource)) throw new Error('Artifact source text was changed');
    if (window.injected) throw new Error('Metadata executed script');
    const values = Object.fromEntries(Array.from(panel.querySelectorAll('dt')).map(term => [term.textContent, term.nextElementSibling]));
    for (const [name, text] of Object.entries(expected)) {
      if (name === 'artifactSource') continue;
      if (values[name].textContent !== text) throw new Error(name + ' is not displayed as its original text');
    }
    if (values.refs.querySelectorAll('br').length !== 1) throw new Error('Reference separation was lost');
    if (Array.from(values.refs.childNodes).filter(node => node.nodeType === Node.TEXT_NODE).length !== 2) {
      throw new Error('References are not two text nodes separated by a break');
    }
    document.body.textContent = 'Map metadata checks passed.';
  } catch (error) {
    document.body.textContent = 'Map metadata checks failed: ' + error.message;
  }
};
const expected = {
  line: child.ln, under: parent.a, children: child.c,
  below: child.tb + ' nodes, ' + child.ob + ' open',
  opened: child.op, closed: child.cl,
  artifactSource: child.ap[0].source,
  refs: child.rf.join(''), governs: child.gv.join(', '),
};
const html = '<!doctype html><html><head><style>' + css + '</style></head><body><div class="map"><div class="rail-viewport"><div class="rail-content"><div class="heads wide"></div><div class="gutter"><svg class="rails wide" width="570" height="64" data-origin="34" data-step="18" data-row="32"><g class="route"></g></svg><svg class="rails narrow" width="280" height="64" data-origin="24" data-step="8" data-row="32"><g class="route"></g></svg></div></div></div><ol class="stops">' +
  '<li class="stop" data-stop="0">Parent</li><li class="stop" data-stop="1">Child</li>' +
  '</ol><aside id="detail" class="detail">Choose a node</aside></div>' +
  '<script type="application/json" id="map-data">' + payload + '</script>' +
  '<script>' + script + '</script><script>(' + check.toString() + ')(' +
  JSON.stringify(expected).replace(/</g, '\\u003c') + ')</script></body></html>';
const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'vivac-map-ui-'));
try {
  const file = path.join(folder, 'map.html');
  fs.writeFileSync(file, html);
  for (const width of [1280, 600]) {
  const result = spawnSync(browser, [
    '--headless', '--window-size=' + width + ',800', '--disable-gpu', '--no-sandbox', '--no-first-run',
    '--disable-background-networking', '--disable-component-update',
    '--host-resolver-rules=MAP * ~NOTFOUND',
    '--user-data-dir=' + path.join(folder, 'profile-' + width), '--dump-dom', pathToFileURL(file).href,
  ], { encoding: 'utf8', timeout: 30000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /<body>Map metadata checks passed\.<\/body>/, (result.stdout.match(/<body>[\s\S]*?<\/body>/) || [result.stdout])[0]);
  }
  console.log('Map metadata checks passed at desktop and mobile widths.');
} finally {
  fs.rmSync(folder, { recursive: true, force: true });
}
