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
  ...parent, a: 't2', t: 'Child', p: 0, r: 1, d: 1,
  ln: `Line ${marker} & "quoted"`,
  op: `Opened ${marker}`, cl: `Closed ${marker}`,
  rf: [`Ref ${marker}`, 'Second <svg onload="window.injected=true"></svg> & literal &lt;tag&gt;'],
  gv: [`Rule ${marker}`, 'Other <script>window.injected=true</script>'],
};
// Parent aliases and counts also pass through the card's metadata rendering.
parent.a = `Parent ${marker}`;
child.c = `Children ${marker}`;
child.tb = `Below ${marker}`;
child.ob = `Open ${marker}`;
const payload = JSON.stringify({ project: 'fixture', stops: [parent, child] }).replace(/</g, '\\u003c');
const script = fs.readFileSync(path.join(__dirname, '../src/web/map.js'), 'utf8');
const check = function (expected) {
  try {
    document.querySelector('[data-stop="1"]').click();
    const panel = document.getElementById('detail');
    if (panel.querySelector('img, svg, script, iframe, object, embed')) {
      throw new Error('Metadata created active HTML elements');
    }
    if (window.injected) throw new Error('Metadata executed script');
    const values = Object.fromEntries(Array.from(panel.querySelectorAll('dt')).map(term => [term.textContent, term.nextElementSibling]));
    for (const [name, text] of Object.entries(expected)) {
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
  refs: child.rf.join(''), governs: child.gv.join(', '),
};
const html = '<!doctype html><html><body><div class="map"><ul>' +
  '<li class="stop" data-stop="0">Parent</li><li class="stop" data-stop="1">Child</li>' +
  '</ul></div><aside id="detail">Choose a node</aside>' +
  '<script type="application/json" id="map-data">' + payload + '</script>' +
  '<script>' + script + '</script><script>(' + check.toString() + ')(' +
  JSON.stringify(expected).replace(/</g, '\\u003c') + ')</script></body></html>';
const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'vivac-map-ui-'));
try {
  const file = path.join(folder, 'map.html');
  fs.writeFileSync(file, html);
  const result = spawnSync(browser, [
    '--headless', '--disable-gpu', '--no-sandbox', '--no-first-run',
    '--disable-background-networking', '--disable-component-update',
    '--host-resolver-rules=MAP * ~NOTFOUND',
    '--user-data-dir=' + path.join(folder, 'profile'), '--dump-dom', pathToFileURL(file).href,
  ], { encoding: 'utf8', timeout: 30000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /<body>Map metadata checks passed\.<\/body>/, result.stdout);
  console.log('Map metadata checks passed.');
} finally {
  fs.rmSync(folder, { recursive: true, force: true });
}
