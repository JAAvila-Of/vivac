const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
class Node {
  constructor(tag = 'div') { this.tagName = tag; this.children = []; this.events = {}; this.attributes = {}; this.hidden = false; this.disabled = false; this.checked = false; this._value = ''; this.textContent = ''; }
  append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(event, handler) { (this.events[event] ||= []).push(handler); }
  get value() { return this._value; }
  set value(value) { this._value = String(value); }
  get selectedIndex() { return this.children.findIndex(child => child.value === this.value); }
  set selectedIndex(index) { this.value = this.children[index]?.value || ''; }
  focus() {}
  fire(event) { for (const handler of this.events[event] || []) handler({target:this}); this['on' + event]?.({target:this}); }
}
const ids = Object.fromEntries(['bootstrap','inventory','editor','editor-fields','agent-list','agent-context','stages','source-stage','assignment-stage','review','reason','preview','apply','plan','message','show-all','back','previous','next'].map(id => [id,new Node()]));
ids.bootstrap.textContent = JSON.stringify({base:'/agents',token:'token'});
ids.reason.value = 'Review scoped agents';
const native = (name,harness='claude-code') => ({name,harness,path:`.${harness}/agents/${name}`,digest:name+'-digest',model:'sonnet',effort:'high',settings:{},unsupported:[]});
const source = native('reviewer');
source.settings = {tools:'Read'};
const other = native('writer');
const existing = native('reviewer','codex'); existing.model = 'native-model';
const inventory = {agents:[{agent:'a1',name:'reviewer',sources:[source,existing],mappings:[]}],unmanaged:[other],harnesses:[{harness:'claude-code',configured:true,capabilities:[],models:['inherit','sonnet'],efforts:['inherit','high'],directory:'.claude/agents',extension:'md'},{harness:'codex',configured:true,capabilities:['sandbox_mode'],models:['inherit'],efforts:['inherit','high'],directory:'.codex/agents',extension:'toml'}],errors:[]};
inventory.harnesses[1].model_catalog={source:'local harness cache',status:'cached',models:[{id:'deep-model',label:'Deep model',efforts:['high','max']},{id:'swift-model',label:'Swift model',efforts:['low'],default_effort:'low'}],note:'Local metadata does not verify account availability.'};
inventory.harnesses[1].models.push('configured-only');
const calls = [];
let resolvePlan;
const context = {document:{getElementById:id=>ids[id] || all(ids.inventory).find(node=>node.id===id),createElement:tag=>new Node(tag),createTextNode:text=>({textContent:text})},fetch:async(url,options)=>{calls.push({url,body:options?.body && JSON.parse(options.body)}); if(url.endsWith('/plan')) return new Promise(resolve => {resolvePlan=()=>resolve({ok:true,json:async()=>({plan_digest:'reviewed-digest',items:JSON.parse(options.body).items.map(item=>({...item,original:source,destinations:item.destinations.map(destination=>({...destination,...destination.assignment,state:'replace_reviewed'}))}))})});}); return {ok:true,json:async()=> url.endsWith('/inventory')?inventory:{sources:[]}};},console};
vm.runInNewContext(fs.readFileSync('src/web/agents.js','utf8'),context);
const tick = () => new Promise(resolve=>setImmediate(resolve));
const all = node => [node,...node.children.flatMap(child=>child.children?all(child):[child])];
const find = (root,tag,text) => all(root).find(node=>node.tagName===tag && node.textContent===text);
const field = (root,title) => all(root).find(node=>node.tagName==='label' && (node.textContent || node.children.filter(child=>!child.tagName).map(child=>child.textContent).join('')).trim()===title)?.children.find(node=>['input','select'].includes(node.tagName));
async function run() {
  await tick();
  assert(find(ids.inventory,'table','') || all(ids.inventory).some(node=>node.tagName==='table'),'Inventory is a table, not editing forms');
  find(ids.inventory,'button','Configure').onclick();
  assert.equal(ids.editor.hidden,false);
  assert.match(ids['agent-context'].textContent,/reviewer/);
  const select = field(ids['source-stage'],'Source version');
  assert(select);
  field(ids['source-stage'],'Synchronize this agent').checked = true;
  field(ids['source-stage'],'Destination: codex').checked = true;
  ids.next.onclick();
  const model = field(ids['assignment-stage'],'Model');
  assert(model.children.some(option=>option.value==='swift-model' && /Swift model.*swift-model/.test(option.textContent)),'Catalog model identifiers and labels are available');
  assert(model.children.some(option=>option.value==='configured-only' && /support unverified/.test(option.textContent)),'Configured identifiers absent from catalog remain selectable');
  assert(all(ids['assignment-stage']).some(node=>/local harness cache.*cached/.test(node.textContent)),'Catalog provenance is visible');
  assert.equal(model.value,'Enter model identifier');
  assert.equal(field(ids['assignment-stage'],'Custom model identifier').value,'native-model','Unknown existing models are preserved explicitly');
  model.value='inherit'; model.fire('change');
  find(ids['agent-list'],'button','writer').onclick();
  find(ids['agent-list'],'button','reviewer').onclick();
  assert.equal(model.value,'inherit','Switching agents retains assignment draft');
  select.selectedIndex=1; select.fire('change');
  select.selectedIndex=0; select.fire('change');
  assert.equal(field(ids['assignment-stage'],'Model').value,'inherit','Returning to a source retains its draft');
  await ids.plan.onclick();
  assert.equal(calls.filter(call=>call.url.endsWith('/plan')).length,0,'Existing destination requires explicit overwrite approval');
  field(ids['assignment-stage'],'Replace this exact existing version after comparison').checked=true;
  await ids.plan.onclick();
  assert.equal(calls.filter(call=>call.url.endsWith('/plan')).length,0,'Unsupported source settings require explicit acknowledgment');
  field(ids['assignment-stage'],'Keep settings only in the source: tools. Destination settings below are independent.').checked=true;
  const effort=field(ids['assignment-stage'],'Effort');
  model.value='configured-only'; model.fire('change');
  assert.equal(effort.value,'high');
  assert.equal(effort.attributes['aria-invalid'],'false','Configured model absent from catalog is not retroactively blocked');
  model.value='deep-model'; model.fire('change');
  assert.deepEqual(effort.children.map(option=>option.value),['inherit','high','max']);
  model.value='swift-model'; model.fire('change');
  assert.equal(effort.value,'high','Changing a model preserves the old effort until corrected');
  assert.equal(effort.attributes['aria-invalid'],'true');
  await ids.plan.onclick();
  assert.equal(calls.filter(call=>call.url.endsWith('/plan')).length,0,'Incompatible effort blocks plan without silently changing it');
  effort.value='low'; effort.fire('change');
  assert.equal(effort.attributes['aria-invalid'],'false');
  assert.deepEqual(effort.children.map(option=>option.value),['inherit','low']);
  model.value='Enter model identifier'; model.fire('change');
  field(ids['assignment-stage'],'Custom model identifier').value='external-model';
  field(ids['assignment-stage'],'Custom model identifier').fire('input');
  assert.equal(effort.value,'low');
  assert.equal(effort.attributes['aria-invalid'],'false','Custom model support is unverified, not blocked');
  const planning=ids.plan.onclick(); await tick();
  ids.reason.value='Changed during plan request'; ids.reason.fire('input');
  resolvePlan(); await planning;
  assert.equal(ids.apply.disabled,true,'Changing selection while request is in flight cannot authorize stale apply');
  const reviewed=ids.plan.onclick(); await tick(); resolvePlan(); await reviewed;
  assert.equal(ids.apply.disabled,false);
  const planned=calls.filter(call=>call.url.endsWith('/plan')).at(-1).body;
  assert.equal(planned.items.length,2,'Configuring multiple agents retains a single batch');
  assert.equal(planned.items[0].destinations[0].digest,existing.digest);
  assert.match(all(ids.preview).map(node=>node.textContent).join(' '),/Complete prompt/);
  model.value='configured-only'; model.fire('change');
  assert.equal(ids.apply.disabled,true,'Editing a reviewed assignment revokes apply');
  assert.equal(calls.filter(call=>call.url.endsWith('/apply')).length,0);
  const again=ids.plan.onclick(); await tick(); resolvePlan(); await again;
  inventory.harnesses[0].configured=false;
  const applying=ids.apply.onclick();
  assert.equal(ids['editor-fields'].disabled,true,'Apply disables editing during request');
  await applying;
  assert.equal(calls.find(call=>call.url.endsWith('/apply')).body.plan_digest,'reviewed-digest');
  assert.equal(ids['editor-fields'].disabled,false);
  assert(find(ids.inventory,'th','claude-code · not set up'),'Native source harness remains visible without setup');
  find(ids.inventory,'button','Configure').onclick();
  const nativeSource=field(ids['source-stage'],'Source version');
  nativeSource.selectedIndex=1; nativeSource.fire('change');
  assert.equal(field(ids['source-stage'],'Destination: claude-code'),undefined,'Unconfigured harness is not offered as a destination');
  console.log('Agents web interaction checks passed.');
}
run().catch(error=>{console.error(error);process.exitCode=1;});
