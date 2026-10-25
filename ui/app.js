import {arcPath,sectors,weight,formatBytes,fraction} from './sunburst.mjs';
import {createEntryMenu} from './context-menu.mjs';
import {machineView,phaseLabel} from './machine.mjs';
const invoke=(name,args={})=>window.__TAURI__.core.invoke(name,args);
const $=s=>document.querySelector(s);
const el=(tag,cls,text)=>{const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n;};
const icons={folder:'M3 7h6l2 2h10v11H3z M3 7V4h6l2 3',disk:'M4 3h16v18H4z M8 17h.01 M12 17h4',arrow:'m9 5 7 7-7 7',up:'m6 14 6-6 6 6',search:'M21 21l-5-5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',file:'M6 3h8l4 4v14H6z M14 3v5h5',back:'m14 6-6 6 6 6',clock:'M12 8v5l3 2 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0'};
function icon(name){const s=document.createElementNS('http://www.w3.org/2000/svg','svg');s.setAttribute('viewBox','0 0 24 24');s.setAttribute('aria-hidden','true');s.classList.add('icon');const p=document.createElementNS(s.namespaceURI,'path');p.setAttribute('d',icons[name]??icons.file);s.append(p);return s;}
const state={view:null,scan:null,metric:'allocated',scans:[],volumes:[],store:'',rows:[],cursor:null,colors:new Map(),request:0,listRequest:0,selected:null,scanning:false,mode:'files',compareBefore:null,compareCursor:null,searchTimer:null};
state.machine=null;state.overview=false;
$('#app').innerHTML=`
<aside class="sidebar">
 <div class="brand"><img src="icon.svg" alt="" width="34" height="34"><div>Macscan<small>PLADS TIL OVERBLIK</small></div><span class="local-dot" title="Kun lokalt"></span></div>
 <section class="locations"><h2>PLACERINGER</h2><button id="whole-mac" class="volume-card whole-mac"><span class="volume-info"><strong>Hele Mac’en</strong><small>Scan opstartsdisken · alle volumener</small></span></button><div id="volumes"></div><button id="choose-folder" class="sidebar-action"><span>＋</span> Scan en mappe…</button></section>
 <section class="history"><div class="section-heading"><h2>MÅLINGER</h2><button id="choose-history" title="Åbn en mappe med gemte målinger" aria-label="Åbn målemappe">↗</button></div><div id="history-list"></div></section>
 <div class="sidebar-footer"><span class="local-dot"></span> Dine data bliver på din Mac<button id="history-location">Målemappe ↗</button><button id="privacy-help">Fuld diskadgang…</button></div>
</aside>
<main>
 <header class="toolbar"><div class="view-heading"><span class="eyebrow">DISK ANALYSE</span><h1 id="view-title">Find det, der fylder.</h1><div id="scan-stamp" class="muted">Lokal scanning. Ingen automatisk oprydning.</div></div><div class="toolbar-actions"><div class="metric-toggle" role="group" aria-label="Størrelsesbegreb"><button id="allocated" aria-pressed="true">Allokeret</button><button id="logical" aria-pressed="false">Logisk</button></div><button id="rescan" class="primary">Scan hele Mac’en</button></div></header>
 <div id="error" class="error hidden" role="alert"></div>
 <div id="progress" class="progress-panel hidden" role="status"><div class="spinner"></div><div><strong id="progress-title">Scanner…</strong><div id="progress-detail"></div></div><button id="stop" class="plain">Stop</button></div>
 <section id="branch-progress" class="branch-progress hidden" aria-label="Løbende scanresultater"><div id="volume-progress"></div><p>Foreløbige filtal er før hardlink-korrektion. Afventende områder er ikke målt som nul. Små filer tælles med.</p><div id="branch-rows"></div></section>
 <nav id="breadcrumbs" class="breadcrumbs" aria-label="Mappesti"></nav>
 <div id="coverage" class="coverage hidden"></div>
 <section class="workspace">
  <div class="chart-panel"><div class="chart-top"><span id="chart-context">Pladsfordeling</span><span class="read-only-pill">Kun analyse</span></div>
   <div class="chart-wrap"><svg id="sunburst" viewBox="0 0 600 600" role="group" aria-label="Interaktiv pladsfordeling"></svg><button id="chart-center" class="chart-center" disabled><span id="center-label">Et klart overblik</span><strong id="center-size">Din disk.</strong><span id="center-note">Vælg en måling eller scan en mappe</span><span id="center-up"></span></button></div>
   <div id="hover-caption" class="hover-caption">Klik på en mappe for at åbne den</div>
   <div class="chart-legend"><span><i class="legend-circle"></i> Hver ring er et mappeniveau</span><span><i class="legend-other"></i> Øvrige poster</span></div>
  </div>
  <div class="listing"><div class="list-tabs" role="group" aria-label="Visning"><button id="files-tab" aria-pressed="true">Indhold <span id="child-count"></span></button><button id="changes-tab" aria-pressed="false">Ændringer</button></div>
   <div id="file-controls" class="search-row"><label for="search">${''}<span>Søg i denne mappe</span></label><input id="search" type="search" placeholder="Fil- eller mappenavn…" autocomplete="off"></div>
   <div id="compare-controls" class="compare-controls hidden"><label for="before-scan">Sammenlign med</label><select id="before-scan"></select></div>
   <div class="list-head"><span id="list-label">Navn</span><span id="size-heading">Størrelse ↓</span></div><div id="rows" class="rows" tabindex="-1" role="list" aria-label="Filer og mapper"></div><button id="more" class="more hidden">Vis flere poster</button><div id="list-message" class="list-message">Her kommer din mappeoversigt.</div>
  </div>
 </section>
 <section id="inspector" class="inspector"><div class="inspector-symbol">${''}</div><div class="inspector-main"><strong id="inspect-name">Ingen måling valgt</strong><div id="inspect-path">Vælg en mappe eller en eksisterende måling i venstre side.</div></div><div class="inspector-numbers"><span id="inspect-logical"></span><span id="inspect-allocated"></span></div><div class="inspector-actions"><button id="copy" class="plain" disabled>Kopiér sti</button><button id="reveal" class="plain" disabled>Vis i Finder ↗</button></div></section>
 <footer><span id="provenance">Rust · Parquet · DuckDB</span><span id="measure-note">Allokeret er ikke nødvendigvis frigørbar APFS-plads.</span></footer>
</main><div id="toast" class="toast hidden" role="status"></div><dialog id="coverage-dialog" aria-labelledby="coverage-title"><h2 id="coverage-title">Områder, der ikke kunne måles</h2><p>Macscan kan have andre macOS-rettigheder end Codex. Vælg en mindre mappe, eller giv selv appen den nødvendige adgang i macOS.</p><p id="coverage-count"></p><ul id="coverage-issues"></ul><form method="dialog"><button class="primary">Luk</button></form></dialog>`;
$('.inspector-symbol').append(icon('folder'));
function error(e){const s=String(e);$('#error').title=s;$('#error').textContent=s.includes('Out of Memory')?'Beregningen nåede hukommelsesgrænsen. Målingen kunne ikke færdiggøres; dine data er ikke ændret.':s;$('#error').classList.remove('hidden');}
function clearError(){$('#error').classList.add('hidden');}
function toast(text){$('#toast').textContent=text;$('#toast').classList.remove('hidden');setTimeout(()=>$('#toast').classList.add('hidden'),2500);}
async function runFileAction(action,target){await invoke('file_action',{...target,action});if(action==='copy')toast('Stien er kopieret.');if(action==='copy_info')toast('Informationen er kopieret fra den gemte måling.');}
const entryMenu=createEntryMenu(window.__TAURI__.menu.Menu,runFileAction,error);
function showEntryMenu(event,entry){
 event.preventDefault();
 if(state.overview)return;
 const rect=event.currentTarget.getBoundingClientRect();
 const position=new window.__TAURI__.dpi.LogicalPosition(event.type==='keydown'?rect.left+rect.width/2:event.clientX,event.type==='keydown'?rect.top+rect.height/2:event.clientY);
 return entryMenu({scanId:state.scan,entry},position);
}
function date(s){return new Intl.DateTimeFormat('da-DK',{day:'numeric',month:'short',hour:'2-digit',minute:'2-digit'}).format(new Date(s));}
function rootName(p){return p==='/'?'Macintosh HD':p==='/System/Volumes/Data'?'Macintosh HD · data':p?.split('/').filter(Boolean).at(-1)||'Mappe';}
function renderHistory(){
 const box=$('#history-list');box.replaceChildren();
 if(!state.scans.length){box.append(el('p','sidebar-empty','Ingen gemte målinger endnu.'));return;}
 for(const scan of state.scans){const b=el('button','history-item'+(state.scan===scan.scan_id?' active':''));b.disabled=state.scanning;b.title=scan.root;
  const main=el('span','history-name',rootName(scan.root));main.prepend(icon('folder'));b.append(main,el('span','history-date',date(scan.started_at)+(scan.status==='partial'?' · delvis':'')));
  b.addEventListener('click',()=>openScan(scan.scan_id));box.append(b);
 }
}
function renderVolumes(){
 const box=$('#volumes');box.replaceChildren();const records=state.volumes;
 const relevant=records.filter(v=>v.mount_display==='/System/Volumes/Data'||(v.mount_display.startsWith('/Volumes/')&&v.supported_local_scan===true));
 if(!relevant.length&&records[0]) relevant.push(records[0]);
 for(const v of relevant){const b=el('button','volume-card');b.disabled=state.scanning;b.title='Scan '+v.mount_display;b.append(icon('disk'));
  const text=el('span','volume-info');text.append(el('strong','',rootName(v.mount_display)),el('small','',formatBytes(v.free_bytes)+' ledigt af '+formatBytes(v.total_bytes)));
  const bar=el('span','capacity-bar'),fill=el('i');fill.style.width=Math.max(0,Math.min(100,(1-Number(v.free_bytes)/Number(v.total_bytes))*100))+'%';bar.append(fill);text.append(bar);b.append(text);
  b.addEventListener('click',()=>beginScan(v.mount_display));box.append(b);
 }
}
async function refresh(){const r=await invoke('bootstrap');state.scans=r.scans;state.volumes=r.volumes.records;state.store=r.store;renderHistory();renderVolumes();$('#history-location').title=state.store;return r;}
async function openScan(id,keepMachine=false){
 if(state.scans.find(s=>s.scan_id===id)?.kind==='machine')return showMachine(await invoke('machine_session',{sessionId:id}));
 if(!keepMachine)state.machine=null;state.overview=false;$('#search').disabled=false;$('#changes-tab').disabled=false;
 for(const m of ['allocated','logical'])$('#'+m).disabled=false;
 state.scan=id;state.selected=null;state.colors.clear();state.mode='files';$('#search').value='';await navigate('0');renderHistory();
}
function showMachine(session){
 ++state.request;++state.listRequest;clearError();state.machine=session;state.overview=true;state.scan=session.session_id;state.mode='files';state.colors.clear();
 const view=machineView(session);state.view=view;state.selected=view.selected;state.rows=view.children.records;state.cursor=null;state.compareCursor=null;
 $('#view-title').textContent='Hele Mac’en';$('#scan-stamp').textContent='Målt '+date(session.started_at)+' · '+new Intl.NumberFormat('da-DK').format(session.entry_count??0)+' poster · '+(session.status==='cancelled'?'stoppet':'gemt lokalt');
 $('#rescan').textContent='Scan hele Mac’en';$('#child-count').textContent=view.child_count;$('#search').value='';$('#search').disabled=true;$('#changes-tab').disabled=true;
 for(const m of ['allocated','logical'])$('#'+m).disabled=true;
 $('#coverage').classList.remove('hidden');$('#coverage').textContent=session.coverage_note+(view.accountingOverlap?' Volumenallokering overstiger containerforbruget; grafen er normaliseret, ikke en frigørbar sum.':'');
 renderCrumbs();renderChart();renderRows();renderInspector();renderMode();renderHistory();
 $('#center-size').textContent=formatBytes(session.used_bytes);$('#center-note').textContent='Diskforbrug fra macOS';$('#chart-context').textContent='APFS-volumener · ikke filsummer';
 $('#provenance').textContent=formatBytes(session.free_bytes)+' ledigt · eksterne drev og diskimages tælles ikke oveni';
}
async function navigate(entry){
 const token=++state.request;++state.listRequest;clearTimeout(state.searchTimer);clearError();$('.workspace').classList.add('loading');
 try{const view=await invoke('view_scan',{scanId:state.scan,entry:Number(entry),metric:state.metric});if(token!==state.request)return;
  state.view=view;state.selected=view.selected;state.rows=view.children.records;state.cursor=view.children.next_cursor;state.mode='files';state.compareCursor=null;$('#search').value='';
  $('#view-title').textContent=view.selected.name_display||rootName(view.scope.root_display);$('#scan-stamp').textContent='Målt '+date(view.started_at)+' · '+new Intl.NumberFormat('da-DK').format(view.entry_count)+' poster · ikke live';
  $('#rescan').textContent='Scan igen';$('#child-count').textContent=view.child_count;renderCrumbs();renderChart();renderRows();renderInspector();renderMode();renderCompareOptions();
  const partial=view.status!=='complete';$('#coverage').classList.toggle('hidden',!partial);
  $('#coverage').textContent=partial?'Delvist scan. Grafen viser kun det observerede minimum — manglende adgang og udeladte områder er ikke nul bytes.':'';
  if(partial){const details=el('button','coverage-button','Se detaljer om dækning');details.addEventListener('click',()=>{
    $('#coverage-count').textContent=view.error_count+' registrerede afvigelser. Viser op til otte eksempler.';
    const list=$('#coverage-issues');list.replaceChildren();for(const issue of view.issues??[])list.append(el('li','',(issue.path_display??'Post '+issue.entry_id)+' — '+issue.detail));
    if(!list.children.length)list.append(el('li','','Se scanbegrænsningen ovenfor; manglende adgang eller udeladte områder er ikke målt som nul.'));
    $('#coverage-dialog').showModal();});$('#coverage').append(details);}
  $('#provenance').textContent=(view.status==='complete'?'Komplet':'Delvis')+' måling · '+(state.metric==='logical'?'Logiske filstørrelser':'Hardlink-tilskrevet allokering');
 }catch(e){if(token===state.request)error(e);}finally{if(token===state.request)$('.workspace').classList.remove('loading');}
}
function renderCrumbs(){const box=$('#breadcrumbs');box.replaceChildren();if(state.machine){const b=el('button','crumb','Hele Mac’en');b.addEventListener('click',()=>showMachine(state.machine));box.append(b);}
 for(const [i,a]of state.view.ancestors.entries()){if(i||state.machine)box.append(el('span','crumb-separator','›'));const b=el('button','crumb',i===0?rootName(state.view.scope.root_display):a.name_display);b.title=a.path_display;b.addEventListener('click',()=>navigate(a.entry_id));box.append(b);}}
function colorFor(n){return `hsl(${state.colors.get(n.entry_id)??210} 70% 65%)`;}
function renderChart(){
 const svg=$('#sunburst');svg.replaceChildren();const view=state.view;
 if(!view){for(let i=0;i<3;i++){const c=document.createElementNS(svg.namespaceURI,'circle');c.setAttribute('cx',300);c.setAttribute('cy',300);c.setAttribute('r',124+i*54);c.setAttribute('class','empty-ring');svg.append(c);}return;}
 const arcs=sectors(view.nodes,view.selected.entry_id,state.colors);
 for(const a of arcs){const p=document.createElementNS(svg.namespaceURI,'path');p.setAttribute('d',arcPath(102+(a.depth-1)*54,150+(a.depth-1)*54,a.start,a.end));p.setAttribute('fill',a.fill);p.classList.add('sector');if(a.aggregate)p.classList.add('aggregate');if(!a.aggregate&&a.node.coverage_complete===false)p.classList.add('partial-sector');
  p.dataset.entry=a.node.entry_id;p.setAttribute('role','button');p.setAttribute('tabindex','-1');p.setAttribute('aria-label',a.node.name_display+', '+formatBytes(a.node.bytes??a.node.observed_bytes));const title=document.createElementNS(svg.namespaceURI,'title');title.textContent=a.node.name_display+' · '+formatBytes(a.node.bytes??a.node.observed_bytes);p.append(title);
  p.addEventListener('contextmenu',e=>{if(a.aggregate){e.preventDefault();toast('Øvrige poster er en samleblok. Vælg den konkrete fil eller mappe i listen.');}else showEntryMenu(e,a.node.entry_id);});
  p.addEventListener('pointerenter',()=>highlight(a.node,a.aggregate));p.addEventListener('pointerleave',()=>highlight(null));p.addEventListener('click',()=>{if(a.aggregate){if(a.node.entry_id!==state.view.selected.entry_id)navigate(a.node.entry_id).then(()=>{$('#rows').focus();toast('De øvrige poster står i listen. Brug Vis flere ved behov.');});else{$('#rows').focus();toast('Alle poster kan åbnes i listen. Brug Vis flere ved behov.');}}else chooseNode(a.node);});svg.append(p);
 }
 $('#center-label').textContent=view.selected.name_display||rootName(view.scope.root_display);
 $('#center-size').textContent=formatBytes(view.selected.bytes??view.selected.observed_bytes);
 $('#center-note').textContent=view.selected.bytes===null?'Mindst observeret':state.metric==='allocated'?'Allokeret plads':'Logisk størrelse';
 $('#chart-center').disabled=view.selected.parent_id===null;$('#chart-center').title='Gå ét mappeniveau op';$('#center-up').textContent=view.selected.parent_id===null?'':'↑ Tilbage';
 $('#chart-context').textContent=view.selected.kind==='file'?'Valgt fil':'Pladsfordeling · 3 niveauer';
 if(!arcs.length)$('#hover-caption').textContent=weight(view.selected)===0n?'Ingen målbare bytes i dette udvalg.':'Dette udvalg har ingen målte underposter.';
 else $('#hover-caption').textContent='Klik på en mappe for at åbne · midten går ét niveau op';
}
function highlight(node,aggregate=false){
 for(const p of document.querySelectorAll('.sector'))p.classList.toggle('dim',!!node&&p.dataset.entry!==node.entry_id&&!aggregate);
 for(const r of document.querySelectorAll('.file-row'))r.classList.toggle('hovered',!!node&&r.dataset.entry===node.entry_id);
 $('#hover-caption').textContent=node?node.name_display+' · '+formatBytes(node.bytes??node.observed_bytes??node.lower_bound_bytes)+(aggregate?' · se listen':''):'Klik på en mappe for at åbne · midten går ét niveau op';
}
async function chooseNode(n){if(state.overview){const row=state.rows.find(r=>r.entry_id===n.entry_id)??n;if(row.scan_id)return openScan(row.scan_id,true);toast(row.error??(row.phase==='unmounted'?'Volumen er ikke monteret. Dets forbrug vises fra macOS.':'Ingen filscanning for dette område.'));return;}
 if(n.kind==='directory')return navigate(n.entry_id);try{const scan=state.scan;const data=await invoke('view_scan',{scanId:scan,entry:Number(n.entry_id),metric:state.metric});if(scan===state.scan){state.selected=data.selected;renderInspector();}}catch(e){error(e);}}
function renderRows(){
 const box=$('#rows');box.replaceChildren();const changed=state.mode==='changes';
 for(const r of state.rows){const id=changed?r.after_entry_id:r.entry_id,b=el('button','file-row');b.dataset.entry=id??'';
  const swatch=el('span','file-symbol');swatch.style.color=changed?(BigInt(r.delta_bytes??0)>0n?'#f2a382':'#65d5bb'):colorFor(r);swatch.append(icon(r.kind==='directory'?'folder':'file'));
  const content=el('span','file-label'),name=changed?r.path_display.split('/').filter(Boolean).at(-1):r.name_display;content.append(el('span','file-name',name));
  if(state.overview)content.append(el('span','file-subtitle',r.kind==='capacity'?'Ikke klassificeret som junk':phaseLabel(r)));
  if(changed){content.append(el('span','file-subtitle',r.delta_bytes===null?'Ukendt dækning':formatBytes(r.before_bytes)+' → '+formatBytes(r.after_bytes)));if(id===null)b.disabled=true;}
  const value=changed?r.delta_bytes:r.bytes??r.lower_bound_bytes;const bytes=el('span','file-bytes',(changed&&value!==null&&BigInt(value)>0n?'+':'')+formatBytes(value));
  if(!changed&&r.bytes===null)bytes.prepend(el('small','lower-label','mindst '));b.title=(r.path_display??state.view.selected.path_display+'/'+name)+' · '+bytes.textContent;
  const arrow=el('span','row-arrow',r.kind==='directory'?'›':'');b.append(swatch,content,bytes,arrow);
  b.addEventListener('click',()=>{if(id!==null){if(changed)navigate(id);else chooseNode({...r,path_display:r.path_display??state.view.selected.path_display+'/'+r.name_display});}});
  b.addEventListener('contextmenu',e=>{if(id!==null)showEntryMenu(e,id);});
  b.addEventListener('keydown',e=>{if(id!==null&&(e.key==='ContextMenu'||e.shiftKey&&e.key==='F10'))showEntryMenu(e,id);});
  b.addEventListener('pointerenter',()=>{if(!changed)highlight(r);});b.addEventListener('pointerleave',()=>highlight(null));const item=el('div');item.setAttribute('role','listitem');item.append(b);box.append(item);
 }
 $('#more').classList.toggle('hidden',!(changed?state.compareCursor:state.cursor));$('#list-message').classList.toggle('hidden',state.rows.length>0);
 $('#list-message').textContent=state.mode==='changes'?'Ingen ændringer i dette udvalg.':state.view?.selected.kind==='file'?'Dette er en fil, ikke en mappe.':$('#search').value?'Ingen navne matcher søgningen.':'Mappen har ingen viste underposter.';
 $('#size-heading').textContent=changed?'Ændring ↓':'Størrelse ↓';
}
function renderInspector(){
 const n=state.selected;if(!n)return;$('#inspect-name').textContent=n.name_display||rootName(n.path_display);$('#inspect-path').textContent=n.path_display||'';$('#inspect-path').title=n.path_display||'';
 $('#inspect-logical').textContent='Logisk '+formatBytes(n.logical_bytes);$('#inspect-allocated').textContent='Allokeret '+formatBytes(n.accounted_allocated_bytes??n.bytes);
 if(state.overview){$('#inspect-logical').textContent='APFS-kapacitet '+formatBytes(state.machine.capacity_bytes);$('#inspect-allocated').textContent='Ledigt '+formatBytes(state.machine.free_bytes);}
 $('#copy').disabled=state.overview;$('#reveal').disabled=state.overview;
}
function renderMode(){const changes=state.mode==='changes';$('#files-tab').setAttribute('aria-pressed',String(!changes));$('#changes-tab').setAttribute('aria-pressed',String(changes));$('#file-controls').classList.toggle('hidden',changes);$('#compare-controls').classList.toggle('hidden',!changes);}
function renderCompareOptions(){const select=$('#before-scan');select.replaceChildren();const policy=state.scans.find(s=>s.scan_id===state.scan)?.policy;const eligible=state.scans.filter(s=>s.scan_id!==state.scan&&s.root===state.view.scope.root_display&&s.policy===policy&&s.started_at<state.view.started_at);
 if(!eligible.length){select.append(new Option('Ingen sammenlignelig tidligere måling',''));state.compareBefore=null;return;}
 for(const s of eligible)select.append(new Option(date(s.started_at)+(s.status==='partial'?' · delvis':''),s.scan_id));state.compareBefore=select.value;
}
async function loadRows(append=false){if(state.overview)return;const token=++state.listRequest;clearError();try{
 let r;if(state.mode==='changes'){if(!state.compareBefore){state.rows=[];state.compareCursor=null;renderRows();$('#list-message').textContent='Scan mappen igen for at få to sammenlignelige målinger. Ældre prototype-målinger kan stadig åbnes.';return;}
  r=await invoke('compare_scans',{before:state.compareBefore,after:state.scan,metric:state.metric,cursor:append?state.compareCursor:null});if(token!==state.listRequest)return;state.compareCursor=r.next_cursor;
 }else{r=await invoke('list_children',{scanId:state.scan,entry:Number(state.view.selected.entry_id),metric:state.metric,cursor:append?state.cursor:null,search:$('#search').value});if(token!==state.listRequest)return;state.cursor=r.next_cursor;}
 state.rows=append?[...state.rows,...r.records]:r.records;renderRows();
 }catch(e){if(token!==state.listRequest)return;state.rows=[];state.cursor=null;state.compareCursor=null;renderRows();$('#list-message').textContent='Visningen kunne ikke beregnes. Vælg en anden måling eller mappe.';error(e);}}
function setScanning(active){state.scanning=active;$('#progress').classList.toggle('hidden',!active);if(!active)$('#branch-progress').classList.add('hidden');
 for(const id of ['rescan','choose-folder','choose-history','history-location','whole-mac'])$('#'+id).disabled=active;
 $('#stop').disabled=false;$('#stop').textContent='Stop';renderHistory();renderVolumes();}
async function beginScan(root){if(state.scanning)return;clearError();try{await invoke(root===null?'start_machine':'start_scan',root===null?{}:{root});setScanning(true);pollJob();}catch(e){error(e);}}
function renderProgress(s){
 const volumes=s.machine?.volumes??[],active=volumes.find(v=>v.phase==='scanning'||v.phase==='saving'),branches=active?.branches??s.branches??[];
 const panel=$('#branch-progress');panel.classList.toggle('hidden',!volumes.length&&!branches.length);
 const volumeBox=$('#volume-progress');volumeBox.replaceChildren();
 for(const v of volumes){const b=el('span','volume-phase',String(v.name??v.device)+' · '+phaseLabel(v));if(v.error)b.title=v.error;volumeBox.append(b);}
 const box=$('#branch-rows');box.replaceChildren();
 const sorted=[...branches].sort((a,b)=>BigInt(a.raw_allocated_bytes)>BigInt(b.raw_allocated_bytes)?-1:BigInt(a.raw_allocated_bytes)<BigInt(b.raw_allocated_bytes)?1:0);
 for(const row of sorted){const item=el('div','branch-row');item.append(el('span','',row.name),el('span','muted',phaseLabel(row)),el('span','',new Intl.NumberFormat('da-DK').format(row.entries)+' poster'),el('span','',row.phase==='pending'?'Ikke målt':formatBytes(row.raw_allocated_bytes)));box.append(item);}
}
async function pollJob(){try{const s=await invoke('job_status');$('#progress-title').textContent=s.phase==='planning'?'Læser Mac’ens volumener…':s.phase==='saving'?'Beregner størrelser og gemmer måling…':'Scanner '+(s.whole_mac?'Hele Mac’en · ':'')+rootName(s.root)+'…';$('#progress-detail').textContent=new Intl.NumberFormat('da-DK').format(s.entries_seen??0)+' poster · '+Math.round((s.elapsed_ms??0)/1000)+' sek. · ingen fast post- eller tidsgrænse';renderProgress(s);
 if(['done','error','cancelled'].includes(s.phase)){setScanning(false);await refresh();if(s.phase==='done'){await openScan(s.scan_id);toast(s.whole_mac?'Samlet oversigt gemt. Klik på en volumen for filerne.':'Målingen er gemt lokalt.');}else if(s.phase==='cancelled')toast('Scanningen er stoppet. Tidligere målinger er bevaret.');else error(s.error);return;}
 setTimeout(pollJob,500);
 }catch(e){setScanning(false);error(e);}}
async function folderScan(){try{const root=await invoke('choose_root');if(root)beginScan(root);}catch(e){error(e);}}
$('#whole-mac').addEventListener('click',()=>beginScan(null));
$('#choose-folder').addEventListener('click',folderScan);$('#rescan').addEventListener('click',()=>state.overview?beginScan(null):state.view?beginScan(state.view.scope.root_display):beginScan(null));
$('#stop').addEventListener('click',async()=>{await invoke('stop_scan');$('#stop').textContent='Stopper…';$('#stop').disabled=true;});
$('#chart-center').addEventListener('click',()=>{if(state.view?.selected.parent_id!==null)navigate(state.view.selected.parent_id);});
for(const metric of ['allocated','logical'])$('#'+metric).addEventListener('click',()=>{state.metric=metric;for(const m of ['allocated','logical'])$('#'+m).setAttribute('aria-pressed',String(m===metric));if(state.view)navigate(state.view.selected.entry_id);});
$('#files-tab').addEventListener('click',()=>{if(!state.view)return;state.mode='files';renderMode();loadRows();});$('#changes-tab').addEventListener('click',()=>{if(!state.view)return;state.mode='changes';renderMode();loadRows();});
$('#before-scan').addEventListener('change',()=>{state.compareBefore=$('#before-scan').value;loadRows();});$('#more').addEventListener('click',()=>loadRows(true));
$('#search').addEventListener('input',()=>{clearTimeout(state.searchTimer);state.searchTimer=setTimeout(()=>loadRows(),200);});
for(const [id,action]of [['copy','copy'],['reveal','reveal']])$('#'+id).addEventListener('click',()=>runFileAction(action,{scanId:state.scan,entry:Number(state.selected.entry_id)}).catch(error));
async function chooseHistory(){try{const r=await invoke('choose_history');if(!r)return;localStorage.setItem('macscan.history',r.store);state.scans=r.scans;state.store=r.store;$('#history-location').title=r.store;renderHistory();const first=r.scans[0];if(first)openScan(first.scan_id);else toast('Denne mappe indeholder ingen målinger.');}catch(e){error(e);}}
$('#choose-history').addEventListener('click',chooseHistory);$('#history-location').addEventListener('click',chooseHistory);
$('#privacy-help').addEventListener('click',async()=>{
 try{await invoke('open_privacy');}catch(e){error(e);}
 toast('Tilføj denne Macscan.app i Fuld diskadgang. Efter en lokal genbygning kan du skulle fjerne og tilføje den igen. Genstart appen bagefter.');
});
document.addEventListener('keydown',e=>{if(e.metaKey&&e.key==='o'){e.preventDefault();folderScan();}if((e.altKey&&e.key==='ArrowLeft'||e.key==='Backspace')&&!['INPUT','SELECT','TEXTAREA'].includes(e.target.tagName)&&state.view?.selected.parent_id!==null&&state.view){e.preventDefault();navigate(state.view.selected.parent_id);}});
renderChart();
try{const r=await refresh();if(r.explicit_store)localStorage.setItem('macscan.history',r.store);if(!r.explicit_store&&localStorage.getItem('macscan.history')){try{const h=await invoke('restore_history',{path:localStorage.getItem('macscan.history')});state.scans=h.scans;state.store=h.store;$('#history-location').title=h.store;renderHistory();}catch{localStorage.removeItem('macscan.history');}}
 const initial=state.scans[0];if(initial)await openScan(initial.scan_id);
}catch(e){error(e);}
