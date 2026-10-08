(() => {
  'use strict';
  const boot = JSON.parse(document.getElementById('bootstrap').textContent);
  const $ = id => document.getElementById(id);
  let inventory, visibleHarnesses = [], cards = [], reviewed = null, active = null, stage = 0, revision = 0, applying = false;
  function element(tag, text, parent, className) {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    if (parent) parent.append(node);
    return node;
  }
  function invalidate() { revision++; reviewed = null; $('apply').disabled = true; $('preview').replaceChildren(); }
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
  function modelOptions(parent, harness, initial) {
    const catalog=harness.model_catalog?.models || [];
    const values=[...new Set([...(harness.models || []),...catalog.map(model=>model.id),'inherit','Enter model identifier'])];
    const select=choice(parent,'Model',values,values.includes(initial) ? initial : initial?'Enter model identifier':'inherit');
    for (const option of select.children) {
      const model=catalog.find(row=>row.id===option.value);
      if (model?.label && model.label!==model.id) option.textContent=model.label + ' · ' + model.id;
      else if (!model && !['inherit','Enter model identifier'].includes(option.value)) option.textContent=option.value + ' · configured, support unverified';
    }
    return select;
  }
  function refreshEfforts(target) {
    const identifier=target.model.value==='Enter model identifier' ? target.custom.value.trim() : target.model.value;
    const model=target.harness.model_catalog?.models?.find(row=>row.id===identifier);
    const known=Array.isArray(model?.efforts) && model.efforts.length>0;
    const allowed=[...new Set(['inherit',...(known ? model.efforts : target.harness.efforts || [])])];
    const current=target.effort.value || 'inherit';
    target.invalidEffort=known && !allowed.includes(current);
    target.effort.replaceChildren();
    for (const value of [...new Set([...allowed,current])]) {
      const option=element('option',value + (target.invalidEffort && value===current ? ' · unsupported for selected model' : ''),target.effort);
      option.value=value;
    }
    target.effort.value=current;
    target.effort.setAttribute('aria-invalid',target.invalidEffort?'true':'false');
    target.effortNotice.className=target.invalidEffort?'warning':'muted';
    target.effortNotice.textContent=target.invalidEffort
      ? 'The selected model does not list effort ' + current + '. Choose a supported effort before reviewing changes.'
      : known ? 'Efforts listed by the model catalog.' + (model.default_effort ? ' Harness default: ' + model.default_effort + '. Inherit leaves the choice to the harness.' : '')
      : 'Effort support is unverified for this model. Generic harness choices and the current value remain available.';
  }
  function details(parent, values) {
    const list = element('dl',undefined,parent,'agent-details');
    for (const [key,value] of Object.entries(values)) {
      element('dt',key,list); element('dd',Array.isArray(value) ? value.join(', ') : typeof value === 'object' && value !== null ? Object.entries(value).map(([name,item])=>name + ': ' + String(item)).join('; ') : String(value ?? 'inherit'),list);
    }
  }
  function sourceInfo(parent, source) {
    details(parent,{Harness:source.harness,Path:source.path,Model:source.model || 'inherit',Effort:source.effort || 'inherit',...(source.settings || {})});
  }
  function showStage(value) {
    stage=value;
    $('source-stage').hidden=stage!==0; $('assignment-stage').hidden=stage!==1; $('review').hidden=stage!==2;
    $('previous').disabled=stage===0; $('next').hidden=stage===2;
    $('stages').replaceChildren();
    ['Source & destinations','Assignments','Review'].forEach((title,index)=> {
      const button=element('button',(index+1) + '. ' + title,$('stages'));
      button.type='button'; button.setAttribute('aria-current',index===stage?'step':'false'); button.onclick=()=>showStage(index);
    });
    if (active) {
      const source=active.sources[active.source?.selectedIndex];
      const targets=active.destinations.filter(target=>target.enabled.checked).map(target=>target.harness.harness);
      const scope=stage===2 ? 'Review batch · ' + cards.filter(card=>card.enabled?.checked).length + ' agent(s) · ' : '';
      $('agent-context').textContent=scope + active.name + ' · ' + (source?.harness || 'No source') + ' → ' + (targets.join(', ') || 'Choose destinations');
    }
  }
  function edit(card) {
    if (!card.enabled.checked) { card.enabled.checked=true; markSelection(card); invalidate(); }
    active=card; $('inventory').hidden=true; $('editor').hidden=false;
    $('source-stage').replaceChildren(card.root); $('assignment-stage').replaceChildren(card.assignmentSource,card.targets,card.comparison);
    for (const row of cards) row.side.setAttribute('aria-current',row===card?'true':'false');
    showStage(stage);
  }
  function markSelection(card) {
    card.side.setAttribute('aria-label',card.name + (card.enabled.checked ? ', selected for synchronization' : ', not selected'));
    card.side.setAttribute('data-selected',card.enabled.checked?'true':'false');
  }
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
      const references=card.sources.map(nativeRef), version=revision;
      const result = await request('compare',{references});
      if (version!==revision || applying) return;
      card.comparison.replaceChildren();
      const rows = Array.isArray(result) ? result : result.references || result.sources || result.files || result.variants || [];
      if (!rows.length) { element('p','No comparable native prompt is available.',card.comparison); return; }
      const prompts = rows.map(row=>row.prompt || row.body || '');
      rows.forEach((row,index)=> {
        const column = element('div',undefined,card.comparison);
        element('h3',row.harness + ' / ' + row.path,column);
        sourceInfo(column,{...(row.metadata || row),harness:row.harness,path:row.path});
        const text = element('textarea',undefined,column); text.readOnly = true; text.value = prompts[index]; text.setAttribute('aria-label','Native prompt for ' + row.harness);
        const others = prompts.filter((_,i)=>i!==index).map(prompt=>new Set(prompt.split('\n')));
        const changed = prompts[index].split('\n').filter(line=>others.some(lines=>!lines.has(line)));
        if (changed.length) element('pre','Different lines in this version (+):\n' + changed.map(line=>'+ ' + line).join('\n'),column,'warning');
      });
      if (active?.comparison===card.comparison) showStage(1);
      message('Current versions shown. No historical merge base is inferred.');
    } catch (error) { message(error.message); }
  }
  function buildCard(agent, sources, managed) {
    const mappings = agent.mappings || [];
    const pending = !managed || mappings.some(row=>row.configured !== 'current' && row.configured !== 'retired') || inventory.harnesses.some(harness=>harness.configured && !sources.some(source=>source.harness===harness.harness));
    const name=agent.name || sources[0]?.name || 'Unnamed agent';
    const card = {agent:managed ? agent.agent : null,name,sources,destinations:[],pending};
    const row=element('tr',undefined,$('agent-rows')); card.row=row;
    const identity=element('th',undefined,row); identity.setAttribute('scope','row'); element('strong',name,identity);
    if (agent.definition?.contract?.purpose) element('p',agent.definition.contract.purpose,identity,'muted');
    for (const harness of visibleHarnesses) {
      const cell=element('td',undefined,row), source=sources.find(item=>item.harness===harness.harness), mapping=mappings.find(item=>item.harness===harness.harness);
      element('strong',source?'Present':'Not present',cell);
      if (source) { element('p',(source.model || 'inherit') + ' / ' + (source.effort || 'inherit'),cell); element('p',source.path,cell,'muted'); }
      element('p',mapping?.configured || (source?'Native only':'No assignment'),cell,pending?'warning':'muted');
      if (mapping) element('p','Runtime: ' + (mapping.observed?.state || 'unverified'),cell,'muted');
    }
    const custody=element('td',managed?'Managed by vivac':'Not managed',row);
    const blocked=agent.retired || agent.definition?.retired || sources.some(source=>source.detached) || !sources.length;
    if (blocked) element('p','Retired, detached or no native source. Synchronization is disabled.',custody,'warning');
    const action=element('td',undefined,row), configure=element('button','Configure',action); configure.disabled=!!blocked; configure.onclick=()=>edit(card);
    card.side=element('button',name,$('agent-list')); card.side.disabled=!!blocked; card.side.onclick=()=>edit(card);
    const root = element('section',undefined,undefined,'agent-card'); card.root = root;
    card.comparison=element('div',undefined,undefined,'compare-grid'); card.targets=element('div',undefined,undefined,'agent-grid');
    card.assignmentSource=element('section',undefined,undefined,'source-details');
    cards.push(card);
    if (blocked) return;
    element('h2','Source version',root);
    element('p',managed ? 'Managed by vivac · ' + agent.agent : 'Not managed by vivac',root);
    card.enabled = check(root,'Synchronize this agent');
    card.enabled.addEventListener('change',()=>markSelection(card)); markSelection(card);
    const sourceLabels = sources.map(source=>source.harness + ' / ' + source.path + ' · model: ' + (source.model || 'inherit') + ' · effort: ' + (source.effort || 'inherit'));
    card.source = choice(root,'Source version',sourceLabels,sourceLabels[0]);
    card.source.addEventListener('change',()=> { refreshTargets(card); card.comparison.replaceChildren(); invalidate(); showStage(stage); });
    card.sourceDetails=element('div',undefined,root,'source-details');
    for (const mapping of mappings) element('p',mapping.harness + ': ' + mapping.configured + ' · runtime: ' + (mapping.observed?.state || 'unverified'),root,pending ? 'warning' : 'muted');
    for (const source of sources) if (source.problem || source.unsupported?.length) element('p',source.harness + ': ' + (source.problem || 'Unsupported fields: ' + source.unsupported.join(', ')),root,'warning');
    const button = element('button','Compare configurations',root); button.type='button'; button.onclick=()=>compare(card);
    card.destinationChecks=element('div',undefined,root,'destination-checks');
    refreshTargets(card);
  }
  function refreshTargets(card) {
    card.drafts ||= new Map();
    const selected=card.sources[card.source.selectedIndex];
    const sourceKey=selected && selected.harness + '/' + selected.path;
    if (card.targetSource===sourceKey) return;
    if (card.targetSource) card.drafts.set(card.targetSource,{nodes:[...card.targets.children],checks:[...card.destinationChecks.children],destinations:card.destinations});
    card.targetSource=sourceKey;
    card.targets.replaceChildren(); card.destinationChecks.replaceChildren(); card.sourceDetails.replaceChildren(); card.assignmentSource.replaceChildren(); card.destinations=[];
    const source = card.sources[card.source.selectedIndex];
    if (!source) return;
    sourceInfo(card.sourceDetails,source);
    element('h2','Original source · read-only',card.assignmentSource); sourceInfo(card.assignmentSource,source);
    const draft=card.drafts.get(sourceKey);
    if (draft) { card.targets.append(...draft.nodes); card.destinationChecks.append(...draft.checks); card.destinations=draft.destinations; return; }
    for (const harness of inventory.harnesses.filter(row=>row.configured)) {
      if (harness.harness === source.harness) continue;
      const parent=element('section',undefined,card.targets,'destination-panel');
      element('h2',harness.harness,parent);
      const owned = card.sources.find(row=>row.harness===harness.harness);
      const candidates = [...(inventory.unmanaged || []),...(inventory.agents || []).flatMap(agent=>(agent.sources || []).map(source=>({...source,owner:agent.agent})))].filter(source=>source.harness===harness.harness);
      const destinationChoice=choice(parent,'Destination file',['Create a new file',...candidates.map(source=>source.path + (source.owner ? ' · managed: ' + source.owner : ' · unmanaged'))],owned ? candidates.find(source=>source.path===owned.path)?.path + ' · managed: ' + card.agent : 'Create a new file');
      const existing=owned;
      const target = {harness,existing,parent};
      target.enabled=check(card.destinationChecks,'Destination: ' + harness.harness);
      parent.hidden=true; target.enabled.addEventListener('change',()=> {parent.hidden=!target.enabled.checked; showStage(stage);});
      element('p','Original: ' + (source.model || 'inherit') + ' / ' + (source.effort || 'inherit'),parent,'muted');
      details(parent,source.settings || {});
      const unsupportedSettings=Object.keys(source.settings || {}).filter(key=>!harness.capabilities.includes(key));
      if (unsupportedSettings.length) target.settingsAcknowledged=check(parent,'Keep settings only in the source: ' + unsupportedSettings.join(', ') + '. Destination settings below are independent.');
      target.name=input(parent,'Agent name',existing?.name || source.name || '');
      const catalog=harness.model_catalog;
      const provenance=element('p',catalog
        ? 'Model catalog: ' + catalog.source + ' · ' + catalog.status + '. Local catalog entries and configured models do not verify account access or runtime loading.'
        : 'No model catalog is available. Configured identifiers remain available; model support is unverified.',parent,'catalog-note muted');
      if (catalog?.note) element('p',catalog.note,parent,'catalog-note muted');
      if (catalog?.fetched_at) element('p','Cached catalog updated: ' + catalog.fetched_at,parent,'catalog-note muted');
      target.model=modelOptions(parent,harness,existing?.model);
      target.model.setAttribute('aria-describedby',provenance.id='model-catalog-' + cards.length + '-' + harness.harness);
      target.custom=input(parent,'Custom model identifier',existing?.model || ''); target.custom.parentElement.hidden=target.model.value!=='Enter model identifier';
      target.model.onchange=()=>{ target.custom.parentElement.hidden=target.model.value!=='Enter model identifier'; refreshEfforts(target); invalidate(); };
      const initialEffort=existing?.effort || 'inherit';
      target.effort=choice(parent,'Effort',[...(harness.efforts || ['inherit']),initialEffort],initialEffort);
      target.effortNotice=element('p',undefined,parent,'muted'); target.effortNotice.setAttribute('role','status');
      target.effort.setAttribute('aria-describedby',target.effortNotice.id='effort-support-' + cards.length + '-' + harness.harness);
      target.effort.onchange=()=>refreshEfforts(target);
      target.custom.addEventListener('input',()=>refreshEfforts(target));
      refreshEfforts(target);
      target.settings=settingsFields(parent,harness,existing?.settings || {});
      target.replace=check(parent,'Replace this exact existing version after comparison');
      target.replace.parentElement.hidden=!existing;
      destinationChoice.onchange=()=> {
        target.existing=destinationChoice.selectedIndex ? candidates[destinationChoice.selectedIndex-1] : null;
        const selected=target.existing;
        const models=[...target.model.children].map(option=>option.value);
        target.name.value=selected?.name || source.name || ''; target.model.value=models.includes(selected?.model) ? selected.model : selected?.model?'Enter model identifier':'inherit'; target.custom.value=selected?.model || ''; target.custom.parentElement.hidden=target.model.value!=='Enter model identifier';
        const effort=selected?.effort || 'inherit';
        if (![...target.effort.children].some(option=>option.value===effort)) { const option=element('option',effort,target.effort); option.value=effort; }
        target.effort.value=effort; refreshEfforts(target);
        target.replace.checked=false; target.replace.parentElement.hidden=!selected;
        for (const [key,field] of Object.entries(target.settings)) { const value=selected?.settings?.[key]; field.value=Array.isArray(value) ? value.join(', ') : value===undefined ? '' : String(value); }
        card.comparison.replaceChildren(); invalidate();
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
        if (target.invalidEffort) throw new Error(card.name + ': choose an effort supported by the selected destination model.');
        const name=target.name.value.trim(), model=target.model.value==='Enter model identifier' ? target.custom.value.trim() : target.model.value;
        target.name.setAttribute('aria-invalid',name?'false':'true');
        target.custom.setAttribute('aria-invalid',model?'false':'true');
        if (!name || !model) throw new Error(card.name + ': choose a destination name and model identifier.');
        destinations.push({assignment:{harness:target.harness.harness,name,model,effort:target.effort.value,settings:readSettings(target.settings)},path:target.existing?.path || target.harness.directory + '/' + name + '.' + target.harness.extension,digest:target.existing?.digest || null});
      }
      items.push({agent:card.agent,source:nativeRef(source),destinations});
    }
    if (!items.length) throw new Error('Select at least one agent.');
    return {why:$('reason').value.trim(),items};
  }
  function visibility() { for (const card of cards) { const hidden=!$('show-all').checked && !card.pending; card.row.hidden=hidden; card.side.hidden=hidden; } }
  async function load() {
    invalidate(); inventory=await request('inventory'); cards=[]; active=null; stage=0; $('inventory').hidden=false; $('editor').hidden=true; $('inventory').replaceChildren(); $('agent-list').replaceChildren();
    const nativeSources=[...(inventory.unmanaged || []),...(inventory.agents || []).flatMap(agent=>agent.sources || [])];
    visibleHarnesses=inventory.harnesses.filter(harness=>harness.configured || nativeSources.some(source=>source.harness===harness.harness));
    const wrapper=element('div',undefined,$('inventory'),'inventory-scroll'), table=element('table',undefined,wrapper,'agents-table');
    element('caption','Native agents and synchronization status',table);
    const head=element('tr',undefined,element('thead',undefined,table));
    for (const title of ['Agent',...visibleHarnesses.map(row=>row.harness + (row.configured?'':' · not set up')),'Custody','Action']) { const th=element('th',title,head); th.setAttribute('scope','col'); }
    const body=element('tbody',undefined,table); body.id='agent-rows';
    for (const agent of inventory.agents || []) buildCard(agent,agent.sources || [],true);
    for (const source of inventory.unmanaged || []) buildCard(source,[source],false);
    for (const row of inventory.errors || []) element('p',(row.harness || 'Inventory') + ': ' + (row.error || row.problem || row.message || 'Native metadata could not be read.'),$('inventory'),'warning');
    if (!cards.length) element('p','No eligible project agents found.', $('inventory'));
    if (!inventory.harnesses.some(row=>row.configured)) message('Configure a harness first with vivac setup.');
    visibility();
  }
  $('show-all').onchange=visibility; $('reason').oninput=invalidate;
  $('back').onclick=()=> { $('editor').hidden=true; $('inventory').hidden=false; };
  $('previous').onclick=()=>showStage(Math.max(0,stage-1)); $('next').onclick=()=>showStage(Math.min(2,stage+1));
  $('plan').onclick=async()=> {
    const version=revision;
    try {
      const selected=selection(), plan=await request('plan',selected);
      if(version!==revision || applying) { message('The selection changed. Review changes again.'); return; }
      reviewed={selection:selected,plan_digest:plan.plan_digest};
      $('preview').replaceChildren();
      element('p','Reason: ' + selected.why,$('preview'));
      element('p','Complete prompt preserved from each selected source. Original source files remain unchanged. Runtime loading remains unverified.',$('preview'));
      for (const item of plan.items || []) {
        const panel=element('section',undefined,$('preview'),'destination-panel');
        element('h3',item.original?.name || item.agent || 'Native agent',panel);
        sourceInfo(panel,item.original || item.source);
        if (!item.destinations.length) element('p','Import into custody; no destination files selected.',panel);
        for (const destination of item.destinations) {
          element('h4',(destination.state==='create'?'Create: ':'Replace reviewed version: ') + destination.harness,panel);
          details(panel,{Path:destination.path,Model:destination.model,Effort:destination.effort,...destination.settings});
          if(destination.before) { element('p','Existing version before replacement:',panel); sourceInfo(panel,destination.before); }
        }
      }
      $('apply').disabled=false; showStage(2); message('Review the entire plan before applying.');
    } catch(error) { if(version===revision) invalidate(); message(error.message); }
  };
  $('apply').onclick=async()=> {
    if (!reviewed || applying) return;
    const selected=reviewed;
    applying=true; $('editor-fields').disabled=true; $('show-all').disabled=true; $('apply').disabled=true;
    try {
      const result=await request('apply',selected);
      await load();
      message(result.exit_code ? 'Application reported remaining work. Review the inventory.' : 'Reviewed changes applied. Runtime loading remains unverified.');
    } catch(error) { invalidate(); message(error.message); }
    finally { applying=false; $('editor-fields').disabled=false; $('show-all').disabled=false; }
  };
  load().catch(error=>message(error.message));
})();
