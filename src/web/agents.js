(() => {
  'use strict';
  const boot = JSON.parse(document.getElementById('bootstrap').textContent);
  const $ = id => document.getElementById(id);
  let inventory, cards = [], reviewed = null;
  function element(tag, text, parent, className) {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    if (parent) parent.append(node);
    return node;
  }
  function invalidate() { reviewed = null; $('apply').disabled = true; $('preview').textContent = ''; }
  function message(text) { $('message').textContent = text; }
  async function request(operation, value) {
    const response = await fetch(boot.base + '/' + operation, value === undefined ? {cache:'no-store'} : {
      method:'POST', cache:'no-store', headers:{'Content-Type':'application/json','X-Vivac-Token':boot.token}, body:JSON.stringify(value)
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'The operation was refused.');
    return result;
  }
  function check(parent, text, checked = false) {
    const label = element('label', undefined, parent);
    const input = element('input', undefined, label); input.type = 'checkbox'; input.checked = checked;
    label.append(document.createTextNode(' ' + text)); input.addEventListener('change', invalidate); return input;
  }
  function input(parent, title, value = '') {
    const label = element('label',title,parent), field = element('input', undefined,label);
    field.value = value; field.addEventListener('input', invalidate); return field;
  }
  function choice(parent, title, options, initial) {
    const label = element('label',title,parent), select = element('select',undefined,label);
    for (const value of [...new Set(options)]) { const option = element('option', value, select); option.value = value; }
    select.value = options.includes(initial) ? initial : options[0]; select.addEventListener('change', invalidate); return select;
  }
  function nativeRef(source) { return {harness:source.harness,path:source.path,digest:source.digest}; }
  function settingsFields(parent, harness, values) {
    const fields = {};
    if (harness.harness === 'codex') {
      fields.sandbox_mode = choice(parent,'Sandbox',['','read-only','workspace-write','danger-full-access'],values.sandbox_mode || '');
    } else {
      for (const key of ['tools','disallowedTools','permissionMode','maxTurns','background','omitClaudeMd','isolation']) {
        if (!harness.capabilities.includes(key)) continue;
        if (['background','omitClaudeMd'].includes(key)) fields[key] = choice(parent,key,['','true','false'],key in values ? String(values[key]) : '');
        else fields[key] = input(parent,key,Array.isArray(values[key]) ? values[key].join(', ') : values[key] === undefined ? '' : String(values[key]));
      }
    }
    return fields;
  }
  function readSettings(fields) {
    const values = {};
    for (const [key,field] of Object.entries(fields)) {
      if (!field.value.trim()) continue;
      if (['background','omitClaudeMd'].includes(key)) values[key] = field.value === 'true';
      else if (key === 'maxTurns') values[key] = Number(field.value);
      else if (['tools','disallowedTools'].includes(key)) values[key] = field.value.trim();
      else values[key] = field.value.trim();
    }
    return values;
  }
  async function compare(card) {
    try {
      const result = await request('compare',{references:card.sources.map(nativeRef)});
      card.comparison.replaceChildren();
      const rows = Array.isArray(result) ? result : result.references || result.sources || result.files || result.variants || [];
      if (!rows.length) { element('p','No comparable native prompt is available.',card.comparison); return; }
      const prompts = rows.map(row=>row.prompt || row.body || '');
      rows.forEach((row,index)=> {
        const column = element('div',undefined,card.comparison);
        element('h3',row.harness + ' / ' + row.path,column);
        element('pre',JSON.stringify(row.metadata || {model:row.model,effort:row.effort,settings:row.settings},null,2),column);
        const text = element('textarea',undefined,column); text.readOnly = true; text.value = prompts[index]; text.setAttribute('aria-label','Native prompt for ' + row.harness);
        const others = prompts.filter((_,i)=>i!==index).map(prompt=>new Set(prompt.split('\n')));
        const changed = prompts[index].split('\n').filter(line=>others.some(lines=>!lines.has(line)));
        if (changed.length) element('pre','Lines absent from another version:\n' + changed.join('\n'),column,'warning');
      });
      message('Current versions shown. No historical merge base is inferred.');
    } catch (error) { message(error.message); }
  }
  function buildCard(agent, sources, managed) {
    const mappings = agent.mappings || [];
    const pending = !managed || mappings.some(row=>row.configured !== 'current' && row.configured !== 'retired') || inventory.harnesses.some(harness=>harness.configured && !sources.some(source=>source.harness===harness.harness));
    const card = {agent:managed ? agent.agent : null,sources,destinations:[],pending};
    const root = element('section',undefined,$('inventory'),'agent-card' + (pending ? ' attention' : '')); card.root = root;
    element('h2',agent.name || sources[0]?.name || 'Unnamed agent',root);
    element('p',managed ? 'Managed by vivac · ' + agent.agent : 'Not managed by vivac',root);
    if (agent.retired || sources.some(source=>source.detached)) { element('p','Retired or explicitly detached; synchronization is disabled.',root); return; }
    card.enabled = check(root,'Synchronize this agent');
    const sourceLabels = sources.map(source=>source.harness + ' / ' + source.path + ' · model: ' + (source.model || 'inherit') + ' · effort: ' + (source.effort || 'inherit'));
    card.source = choice(root,'Source version',sourceLabels,sourceLabels[0]);
    card.source.addEventListener('change',()=> { refreshTargets(card); invalidate(); });
    for (const mapping of mappings) element('p',mapping.harness + ': ' + mapping.configured + ' · runtime: ' + (mapping.observed?.state || 'unverified'),root,pending ? 'warning' : 'muted');
    for (const source of sources) if (source.problem || source.unsupported?.length) element('p',source.harness + ': ' + (source.problem || 'Unsupported fields: ' + source.unsupported.join(', ')),root,'warning');
    const button = element('button','Compare configurations',root); button.type='button'; button.onclick=()=>compare(card);
    card.comparison = element('div',undefined,root,'compare-grid');
    card.targets = element('div',undefined,root,'agent-grid');
    refreshTargets(card); cards.push(card);
  }
  function refreshTargets(card) {
    card.targets.replaceChildren(); card.destinations=[];
    const source = card.sources[card.source.selectedIndex];
    if (!source) return;
    for (const harness of inventory.harnesses.filter(row=>row.configured)) {
      if (harness.harness === source.harness) continue;
      const parent=element('div',undefined,card.targets);
      const owned = card.sources.find(row=>row.harness===harness.harness);
      const candidates = [...(inventory.unmanaged || []),...(inventory.agents || []).flatMap(agent=>(agent.sources || []).map(source=>({...source,owner:agent.agent})))].filter(source=>source.harness===harness.harness);
      const destinationChoice=choice(parent,'Destination file',['Create a new file',...candidates.map(source=>source.path + (source.owner ? ' · managed: ' + source.owner : ' · unmanaged'))],owned ? candidates.find(source=>source.path===owned.path)?.path + ' · managed: ' + card.agent : 'Create a new file');
      const existing=owned;
      const target = {harness,existing};
      target.enabled=check(parent,'Destination: ' + harness.harness);
      element('p','Original: ' + (source.model || 'inherit') + ' / ' + (source.effort || 'inherit'),parent,'muted');
      element('pre','Original settings:\n' + JSON.stringify(source.settings || {},null,2),parent);
      const unsupportedSettings=Object.keys(source.settings || {}).filter(key=>!harness.capabilities.includes(key));
      if (unsupportedSettings.length) target.settingsAcknowledged=check(parent,'Keep settings only in the source: ' + unsupportedSettings.join(', ') + '. Destination settings below are independent.');
      target.name=input(parent,'Agent name',existing?.name || source.name || '');
      target.model=choice(parent,'Model',[...(harness.models || []),'inherit','Enter model identifier'],existing?.model || 'inherit');
      target.custom=input(parent,'Custom model identifier'); target.custom.hidden=target.model.value!=='Enter model identifier';
      target.model.onchange=()=>{ target.custom.hidden=target.model.value!=='Enter model identifier'; invalidate(); };
      target.effort=choice(parent,'Effort',harness.efforts || ['inherit'],existing?.effort || 'inherit');
      target.settings=settingsFields(parent,harness,existing?.settings || {});
      target.replace=check(parent,'Replace this exact existing version after comparison');
      target.replace.parentElement.hidden=!existing;
      destinationChoice.onchange=()=> {
        target.existing=destinationChoice.selectedIndex ? candidates[destinationChoice.selectedIndex-1] : null;
        const selected=target.existing;
        if (selected) { target.name.value=selected.name || ''; target.model.value=selected.model || 'inherit'; target.effort.value=selected.effort || 'inherit'; }
        target.replace.checked=false; target.replace.parentElement.hidden=!selected;
        for (const [key,field] of Object.entries(target.settings)) { const value=selected?.settings?.[key]; field.value=Array.isArray(value) ? value.join(', ') : value===undefined ? '' : String(value); }
        invalidate();
      };
      const compareTarget=element('button','Compare selected source and destination',parent);
      compareTarget.onclick=()=>compare({...card,sources:target.existing ? [source,target.existing] : [source]});
      card.destinations.push(target);
    }
  }
  function selection() {
    const items=[];
    for (const card of cards.filter(card=>card.enabled?.checked)) {
      const source=card.sources[card.source.selectedIndex];
      if (source.problem || source.unsupported?.length) throw new Error('The source has unsupported or unsafe metadata.');
      const destinations=[];
      for (const target of card.destinations.filter(row=>row.enabled.checked)) {
        if (target.existing && !target.replace.checked) throw new Error('Compare and explicitly approve each existing destination.');
        if (target.settingsAcknowledged && !target.settingsAcknowledged.checked) throw new Error('Acknowledge source settings that have no destination representation.');
        const name=target.name.value.trim();
        destinations.push({assignment:{harness:target.harness.harness,name,model:target.model.value==='Enter model identifier' ? target.custom.value.trim() : target.model.value,effort:target.effort.value,settings:readSettings(target.settings)},path:target.existing?.path || target.harness.directory + '/' + name + '.' + target.harness.extension,digest:target.existing?.digest || null});
      }
      items.push({agent:card.agent,source:nativeRef(source),destinations});
    }
    if (!items.length) throw new Error('Select at least one agent.');
    return {why:$('reason').value.trim(),items};
  }
  function visibility() { for (const card of cards) card.root.hidden=!$('show-all').checked && !card.pending; }
  async function load() {
    invalidate(); inventory=await request('inventory'); cards=[]; $('inventory').replaceChildren();
    for (const agent of inventory.agents || []) buildCard(agent,agent.sources || [],true);
    for (const source of inventory.unmanaged || []) buildCard(source,[source],false);
    for (const row of inventory.errors || []) element('p',JSON.stringify(row),$('inventory'),'warning');
    if (!cards.length) element('p','No eligible project agents found.', $('inventory'));
    if (!inventory.harnesses.some(row=>row.configured)) message('Configure a harness first with vivac setup.');
    visibility();
  }
  $('show-all').onchange=visibility; $('reason').oninput=invalidate;
  $('plan').onclick=async()=> { try { const selected=selection(), plan=await request('plan',selected); reviewed={selection:selected,plan_digest:plan.plan_digest}; $('preview').textContent=JSON.stringify(plan,null,2); $('apply').disabled=false; message('Review the entire plan before applying.'); } catch(error) { invalidate(); message(error.message); } };
  $('apply').onclick=async()=> { if (!reviewed) return; const selected=reviewed; $('apply').disabled=true; try { const result=await request('apply',selected); await load(); message(result.exit_code ? 'Application reported remaining work. Review the inventory.' : 'Reviewed changes applied. Runtime loading remains unverified.'); } catch(error) { invalidate(); message(error.message); } };
  load().catch(error=>message(error.message));
})();
