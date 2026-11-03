import {APFS,metrics,metricLabel,apfsSupported,apfsUnavailable,unavailableNote,apfsNote,apfsContribution,apfsZeroNonowner} from './metrics.mjs';
import {createFocusedRescan,rescanLabel,sameRootBaseline} from './focused-rescan.mjs';
import {createCoverageDialog} from './coverage.mjs';
import {createProfileState,renderProfile} from './file-profile.mjs';
import {createGrowthState,exactBytes,signedDelta,knownBytes,growthTarget,growthCounts,elapsedLabel} from './growth.mjs';
import {createInsightsState,insightLabel,insightBytes,contextLabel,insightTarget,insightGuidance} from './insights.mjs';
import {categoryPresentation} from './categories.mjs';
import {arcPath,sectors,weight,formatBytes,fraction} from './sunburst.mjs';
import {createEntryMenu} from './context-menu.mjs';
import {machineView,phaseLabel} from './machine.mjs';
const invoke=(name,args={})=>window.__TAURI__.core.invoke(name,args);
const $=s=>document.querySelector(s);
const el=(tag,cls,text)=>{const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n;};
const icons={folder:'M3 7h6l2 2h10v11H3z M3 7V4h6l2 3',disk:'M4 3h16v18H4z M8 17h.01 M12 17h4',arrow:'m9 5 7 7-7 7',up:'m6 14 6-6 6 6',search:'M21 21l-5-5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',file:'M6 3h8l4 4v14H6z M14 3v5h5',back:'m14 6-6 6 6 6',clock:'M12 8v5l3 2 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0'};
function icon(name){const s=document.createElementNS('http://www.w3.org/2000/svg','svg');s.setAttribute('viewBox','0 0 24 24');s.setAttribute('aria-hidden','true');s.classList.add('icon');const p=document.createElementNS(s.namespaceURI,'path');p.setAttribute('d',icons[name]??icons.file);s.append(p);return s;}
const state={view:null,scan:null,metric:'allocated',scans:[],volumes:[],store:'',rows:[],cursor:null,colors:new Map(),request:0,listRequest:0,selected:null,scanning:false,mode:'files',compareBefore:null,searchTimer:null};
const profile=createProfileState();
const insights=createInsightsState();
const growth=createGrowthState();
state.machine=null;state.overview=false;
$('#app').innerHTML=`
<aside class="sidebar">
 <div class="brand"><img src="icon.svg" alt="" width="34" height="34"><div>Macscan<small>PLADS TIL OVERBLIK</small></div><span class="local-dot" title="Kun lokalt"></span></div>
 <section class="locations"><h2>PLACERINGER</h2><button id="whole-mac" class="volume-card whole-mac"><span class="volume-info"><strong>Hele Mac’en</strong><small>Scan opstartsdisken · alle volumener</small></span></button><div id="volumes"></div><button id="choose-folder" class="sidebar-action"><span>＋</span> Scan en mappe…</button></section>
 <section class="history"><div class="section-heading"><h2>MÅLINGER</h2><button id="choose-history" title="Åbn en mappe med gemte målinger" aria-label="Åbn målemappe">↗</button></div><p class="muted" id="history-retention">Seneste 3 scanninger · Hele Mac’en tæller én gang</p><div id="history-list"></div></section>
 <div class="sidebar-footer"><span class="local-dot"></span> Dine data bliver på din Mac<button id="history-location">Målemappe ↗</button><button id="privacy-help">Fuld diskadgang…</button></div>
</aside>
<main>
 <header class="toolbar"><div class="view-heading"><span class="eyebrow">DISK ANALYSE</span><h1 id="view-title">Find det, der fylder.</h1><div id="scan-stamp" class="muted">Lokal scanning. Ingen automatisk oprydning.</div></div><div class="toolbar-actions"><div class="metric-toggle" role="group" aria-label="Størrelsesbegreb"><button id="allocated" aria-pressed="true">Allokeret</button><button id="logical" aria-pressed="false">Logisk</button><button id="apfs-data-clone" aria-pressed="false" aria-label="Begrænset DATA-klonkorrigeret allokering" title="Begrænset DATA-klonkorrigeret allokering">APFS DATA</button></div><button id="rescan" class="primary">Scan hele Mac’en</button></div></header>
 <label class="capture-option"><input id="capture-clones" type="checkbox"> APFS-klonmetadata til næste nye scanning <small>Tilvalg · eksisterende målinger ændres ikke</small></label><p id="metric-note" class="muted" role="status"></p>
 <div id="error" class="error hidden" role="alert"></div>
 <div id="progress" class="progress-panel hidden" role="status"><div class="spinner"></div><div><strong id="progress-title">Scanner…</strong><div id="progress-detail"></div></div><button id="stop" class="plain">Stop</button></div>
 <section id="branch-progress" class="branch-progress hidden" aria-label="Løbende scanresultater"><div id="volume-progress"></div><p>Foreløbige filtal er før hardlink-korrektion. Afventende områder er ikke målt som nul. Små filer tælles med.</p><div id="branch-rows"></div></section>
 <nav id="breadcrumbs" class="breadcrumbs" aria-label="Mappesti"></nav>
 <div id="coverage" class="coverage hidden"></div>
 <section class="workspace">
  <div class="chart-panel"><div class="chart-top"><span id="chart-context">Pladsfordeling</span><span class="read-only-pill">Kun analyse</span></div>
   <div class="chart-wrap"><svg id="sunburst" viewBox="0 0 600 600" role="group" aria-label="Interaktiv pladsfordeling"></svg><button id="chart-center" class="chart-center" disabled><span id="center-label">Et klart overblik</span><strong id="center-size">Din disk.</strong><span id="center-note" role="note">Vælg en måling eller scan en mappe</span><span id="center-up"></span></button></div>
   <div id="hover-caption" class="hover-caption">Klik på en mappe for at åbne den</div>
   <div class="chart-legend"><span><i class="legend-circle"></i> Hver ring er et mappeniveau</span><span><i class="legend-other"></i> Øvrige poster</span></div>
  </div>
  <div class="listing"><div class="list-tabs" role="group" aria-label="Visning"><button id="files-tab" aria-pressed="true">Indhold <span id="child-count"></span></button><button id="changes-tab" aria-pressed="false">Ændringer</button><button id="insights-tab" aria-pressed="false" disabled>Indsigter</button></div>
   <p id="insights-hint" class="insights-hint">Vælg en gemt måling for at se indsigter.</p>
   <section id="insights-panel" class="insights-panel hidden" aria-label="Indsigter fra gemt måling"></section>
   <div id="profile-entry"><button id="find-files" class="more" disabled>Find filer…</button><p id="profile-hint" class="muted">Vælg en gemt måling for filanalyse.</p></div>
   <section id="profile-panel" tabindex="-1" class="insights-panel hidden" aria-label="Find filer i gemt måling"></section>
   <div id="file-controls" class="search-row"><label for="search">${''}<span>Søg i denne mappe</span></label><input id="search" type="search" placeholder="Fil- eller mappenavn…" autocomplete="off"></div>
   <div id="compare-controls" class="compare-controls hidden"><label for="before-scan">Sammenlign med</label><select id="before-scan"></select></div>
   <section id="growth-panel" tabindex="-1" class="insights-panel hidden" aria-label="Vækst mellem gemte målinger"></section>
   <div class="list-head"><span id="list-label">Navn</span><span id="size-heading">Størrelse ↓</span></div><div id="rows" class="rows" tabindex="-1" role="list" aria-label="Filer og mapper"></div><button id="more" class="more hidden">Vis flere poster</button><div id="list-message" class="list-message">Her kommer din mappeoversigt.</div>
  </div>
 </section>
 <section id="inspector" class="inspector"><div class="inspector-symbol">${''}</div><div class="inspector-main"><strong id="inspect-name">Ingen måling valgt</strong><div id="inspect-category" role="note"></div><div id="inspect-path">Vælg en mappe eller en eksisterende måling i venstre side.</div></div><div class="inspector-numbers"><span id="inspect-logical"></span><span id="inspect-allocated"></span></div><div class="inspector-actions"><button id="copy" class="plain" disabled>Kopiér sti</button><button id="reveal" class="plain" disabled>Vis i Finder ↗</button></div></section>
 <footer><span id="provenance">Rust · Parquet · DuckDB</span><span id="measure-note">Allokeret er ikke nødvendigvis frigørbar APFS-plads.</span></footer>
</main><dialog id="rescan-dialog" aria-label="Genmål valgt område"></dialog><div id="toast" class="toast hidden" role="status"></div><dialog id="coverage-dialog" aria-labelledby="coverage-title"><header class="coverage-heading"><h2 id="coverage-title">Dækning fra gemte målinger</h2><form method="dialog"><button class="primary" autofocus>Luk</button></form></header><div id="coverage-body"></div></dialog>`;

const focusedRescan=createFocusedRescan($('#rescan-dialog'),{invoke,isScanning:()=>state.scanning,onError:error,onStarted:()=>{setScanning(true);pollJob();}});
function renderOrigin(view){
 const old=$('#source-origin');if(old)old.remove();
 if(!view.origin)return;
 const box=el('div','muted');box.id='source-origin';
 box.append(el('span','','Selvstændig genmåling · kilde: '+date(view.origin.source_started_at)+' · '));
 const exists=state.scans.some(s=>s.scan_id===view.origin.source_scan_id);
 const link=el('button','crumb','Åbn oprindelig måling/post');
 link.addEventListener('click',async()=>{const request=++state.request;try{
  const source=await preloadScan(view.origin.source_scan_id,view.origin.source_entry_id,request);
  if(request!==state.request||!source)return;
  await openScan(view.origin.source_scan_id,false,view.origin.source_entry_id,source);
 }catch(e){if(request!==state.request)return;box.textContent='Den oprindelige måling/post er ikke tilgængelig: '+String(e);}});
 box.append(link);if(!exists)box.append(el('span','',' (kilden er ikke i den seneste historik; åbnes fra gemt ID)'));
 $('#scan-stamp').after(box);
}
const coverageDialog=createCoverageDialog($('#coverage-dialog'),{
 fetch:args=>invoke('coverage',args),
 openEntry:target=>openScan(target.scanId,Boolean(state.machine),target.entry).catch(error),
 privacy:()=>$('#privacy-help').click()
});
function coverageButton(action){const details=el('button','coverage-button','Se detaljer om dækning');details.addEventListener('click',action);$('#coverage').append(details);}
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
 for(const scan of state.scans.filter(s=>!s.grouped_volume)){const b=el('button','history-item'+(state.scan===scan.scan_id?' active':''));b.disabled=state.scanning;b.title=scan.root;
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
function setMetric(metric){state.metric=metric;for(const m of metrics)$('#'+m).setAttribute('aria-pressed',String(m===metric));}
async function preloadScan(id,entry,token){
 let metric=state.metric,note='';
 const known=state.scans.find(s=>s.scan_id===id);
 if(metric===APFS&&known&&!apfsSupported(known)){metric='allocated';note=unavailableNote;}
 let view;
 try{view=await invoke('view_scan',{scanId:id,entry:Number(entry),metric});}
 catch(e){
  if(token!==state.request)return null;
  if(metric!==APFS||!apfsUnavailable(e))throw e;
  metric='allocated';note=unavailableNote;
  view=await invoke('view_scan',{scanId:id,entry:Number(entry),metric});
 }
 if(token!==state.request)return null;
 return {view,metric,note};
}
async function openScan(id,keepMachine=false,entry='0',loaded=null){
 const token=++state.request;
 if(state.scans.find(s=>s.scan_id===id)?.kind==='machine'){
  const session=await invoke('machine_session',{sessionId:id});if(token===state.request)showMachine(session);return;
 }
 const prepared=loaded??await preloadScan(id,entry,token);if(token!==state.request||!prepared)return;
 setMetric(prepared.metric);state.metricNotice=prepared.note;
 focusedRescan.close();
 coverageDialog.close();
 profile.reset({active:false,scope:'current',age:'',size:'',typeKey:null,typeLabel:null,types:false,sortCount:false});
 if(!keepMachine)state.machine=null;state.overview=false;$('#search').disabled=false;$('#changes-tab').disabled=false;
 for(const m of metrics)$('#'+m).disabled=false;
 insights.reset({scope:'all',category:null});growth.reset({scope:'all',group:null,files:false});state.compareBefore=null;$('#insights-tab').disabled=false;
 state.scan=id;state.selected=null;state.colors.clear();state.mode='files';$('#search').value='';await navigate(entry,prepared.view);renderHistory();
}
function showMachine(session){
 focusedRescan.close();
 coverageDialog.close();
 profile.reset({active:false});
 insights.reset({scope:'all',category:null});growth.reset();$('#insights-tab').disabled=true;
 ++state.request;++state.listRequest;clearError();state.machine=session;state.overview=true;state.scan=session.session_id;state.mode='files';state.colors.clear();
 const view=machineView(session);state.view=view;state.selected=view.selected;state.rows=view.children.records;state.cursor=null;
 $('#source-origin')?.remove();$('#metric-note').textContent='Kapacitet og diskforbrug fra macOS · uafhængigt af filmetrik';$('#measure-note').textContent='Diskforbrug fra macOS er ikke en filsum eller frigørbar plads.';
 $('#view-title').textContent='Hele Mac’en';$('#scan-stamp').textContent='Målt '+date(session.started_at)+' · '+new Intl.NumberFormat('da-DK').format(session.entry_count??0)+' poster · '+(session.status==='cancelled'?'stoppet':'gemt lokalt');
 $('#rescan').textContent='Scan hele Mac’en';$('#child-count').textContent=view.child_count;$('#search').value='';$('#search').disabled=true;$('#changes-tab').disabled=true;
 for(const m of metrics)$('#'+m).disabled=true;
 $('#coverage').classList.remove('hidden');$('#coverage').textContent=session.coverage_note+(view.accountingOverlap?' Volumenallokering overstiger containerforbruget; grafen er normaliseret, ikke en frigørbar sum.':'');
 coverageButton(()=>coverageDialog.openMachine(session));
 renderCrumbs();renderChart();renderRows();renderInspector();renderMode();renderHistory();
 $('#center-size').textContent=formatBytes(session.used_bytes);$('#center-note').textContent='Diskforbrug fra macOS';$('#chart-context').textContent='APFS-volumener · ikke filsummer';
 $('#provenance').textContent=formatBytes(session.free_bytes)+' ledigt · eksterne drev og diskimages tælles ikke oveni';
}
async function navigate(entry,loaded=null){
 profile.reset();if(profile.active&&state.mode==='files'){profile.busy=true;renderFileProfile();}
 insights.reset({category:null});growth.reset({group:null,files:false});
 const token=++state.request;++state.listRequest;clearTimeout(state.searchTimer);clearError();$('.workspace').classList.add('loading');
 try{const view=loaded??await invoke('view_scan',{scanId:state.scan,entry:Number(entry),metric:state.metric});if(token!==state.request)return;
  state.view=view;state.selected=view.selected;
  $('#'+APFS).disabled=!apfsSupported(view);$('#'+APFS).title=apfsSupported(view)?metricLabel(APFS):unavailableNote;
  $('#metric-note').textContent=state.metricNotice||(state.metric===APFS?apfsNote(view.apfs_data_clone):!apfsSupported(view)?unavailableNote.replace('Allokeret vises. ',''):'');
  $('#measure-note').textContent=state.metric===APFS?apfsNote(view.apfs_data_clone):'Allokeret er ikke nødvendigvis frigørbar APFS-plads.';state.rows=view.children.records;state.cursor=view.children.next_cursor;$('#search').value='';
  $('#view-title').textContent=view.selected.name_display||rootName(view.scope.root_display);$('#scan-stamp').textContent='Målt '+date(view.started_at)+' · '+new Intl.NumberFormat('da-DK').format(view.entry_count)+' poster · ikke live';
  $('#rescan').textContent=rescanLabel(view.selected);renderOrigin(view);$('#child-count').textContent=view.child_count;renderCrumbs();renderChart();renderRows();renderInspector();renderMode();renderCompareOptions();
  const partial=view.status!=='complete';$('#coverage').classList.remove('hidden');
  $('#coverage').textContent=partial?'Delvis måling. Grafen viser det observerede minimum; ukendte områder er ikke nul bytes.':'Gemt måling · se registrerede hændelser og evidens om dækning.';
  const coverageScan=state.scan;coverageButton(()=>coverageDialog.open(coverageScan,view.scope.root_display));
  if(state.mode==='files'&&profile.active)loadProfile();
  if(state.mode==='insights')loadInsights();
  if(state.mode==='changes')loadGrowth();
  $('#provenance').textContent=(view.status==='complete'?'Komplet':'Delvis')+' måling · '+metricLabel(state.metric);
 }catch(e){if(token===state.request){error(e);if(profile.active){profile.busy=false;profile.error=String(e);renderFileProfile();}}}finally{if(token===state.request)$('.workspace').classList.remove('loading');}
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
 const fullNote=(state.overview?'Diskforbrug fra macOS':metricLabel(state.metric))+(view.selected.bytes===null?' · Mindst observeret':'');
 $('#center-note').textContent=view.selected.bytes===null?'Mindst observeret':state.metric===APFS&&!state.overview?'APFS DATA':metricLabel(state.metric);
 $('#center-note').title=fullNote;$('#center-note').setAttribute('aria-label',fullNote);
 if(state.metric===APFS&&!state.overview)$('#chart-center').setAttribute('aria-label',($('#center-label').textContent+' · '+$('#center-size').textContent+' · '+fullNote)+(view.selected.parent_id===null?'':' · Gå ét mappeniveau op'));else $('#chart-center').removeAttribute('aria-label');
 $('#chart-center').disabled=view.selected.parent_id===null;$('#chart-center').title=state.metric===APFS&&!state.overview?fullNote+(view.selected.parent_id===null?'':' · Gå ét mappeniveau op'):'Gå ét mappeniveau op';$('#center-up').textContent=view.selected.parent_id===null?'':'↑ Tilbage';
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
 if(n.kind==='directory')return navigate(n.entry_id);const scan=state.scan,request=state.request,metric=state.metric;try{const data=await invoke('view_scan',{scanId:scan,entry:Number(n.entry_id),metric});if(scan===state.scan&&request===state.request&&metric===state.metric){state.selected=data.selected;renderInspector();}}catch(e){if(scan===state.scan&&request===state.request&&metric===state.metric)error(e);}}
// This renderer consumes ordinary children only. Growth owns its separate panel
// and never changes the schema of state.rows, including during graph refreshes.
function renderRows(){
 const box=$('#rows');box.replaceChildren();
 for(const r of state.rows){const id=r.entry_id,b=el('button','file-row');b.dataset.entry=id??'';
  const swatch=el('span','file-symbol');swatch.style.color=colorFor(r);swatch.append(icon(r.kind==='directory'?'folder':'file'));
  const content=el('span','file-label'),name=r.name_display;content.append(el('span','file-name',name));
  if(!state.overview){const c=categoryPresentation(r),hint=el('span','file-subtitle',c.label+' · '+c.coverage);hint.title=c.reason+' '+c.detail;content.append(hint);}
  if(state.overview)content.append(el('span','file-subtitle',r.kind==='capacity'?'Ikke klassificeret som junk':phaseLabel(r)));
  const bytes=el('span','file-bytes',formatBytes(r.bytes??r.lower_bound_bytes));
  if(r.bytes===null)bytes.prepend(el('small','lower-label','mindst '));b.title=(r.path_display??state.view.selected.path_display+'/'+name)+' · '+bytes.textContent;
  const arrow=el('span','row-arrow',r.kind==='directory'?'›':'');b.append(swatch,content,bytes,arrow);
  b.addEventListener('click',()=>{if(id!=null)chooseNode({...r,path_display:r.path_display??state.view.selected.path_display+'/'+r.name_display});});
  b.addEventListener('contextmenu',e=>{if(id!=null)showEntryMenu(e,id);});
  b.addEventListener('keydown',e=>{if(id!=null&&(e.key==='ContextMenu'||e.shiftKey&&e.key==='F10'))showEntryMenu(e,id);});
  b.addEventListener('pointerenter',()=>highlight(r));b.addEventListener('pointerleave',()=>highlight(null));const item=el('div');item.setAttribute('role','listitem');item.append(b);box.append(item);
 }
 $('#more').classList.toggle('hidden',!state.cursor);$('#list-message').classList.toggle('hidden',state.rows.length>0);
 $('#list-message').textContent=state.view?.selected.kind==='file'?'Dette er en fil, ikke en mappe.':$('#search').value?'Ingen navne matcher søgningen.':'Mappen har ingen viste underposter.';
 $('#size-heading').textContent='Størrelse ↓';
}
function renderInspector(){
 const n=state.selected;if(!n)return;$('#inspect-name').textContent=n.name_display||rootName(n.path_display);$('#inspect-path').textContent=n.path_display||'';$('#inspect-path').title=n.path_display||'';
 $('#inspect-logical').textContent='Logisk '+formatBytes(n.logical_bytes);$('#inspect-allocated').textContent='Allokeret '+formatBytes(n.accounted_allocated_bytes??n.bytes);
 if(state.metric===APFS&&!state.overview){$('#inspect-allocated').textContent='APFS DATA '+formatBytes(n.bytes??n.observed_bytes);}
 const c=categoryPresentation(n);$('#inspect-category').textContent=state.overview?'':`${c.label} · ${c.reason} ${c.coverage}`;$('#inspect-category').title=c.detail;
 if(state.metric===APFS&&!state.overview){$('#inspect-category').textContent+=' · '+apfsContribution(n)+(n.apfs_data_clone_owner!=null?' · Global ejerpost '+n.apfs_data_clone_owner:'');if(apfsZeroNonowner(n))$('#inspect-category').textContent+=' · Nul eget bidrag skyldes global tilskrivning; filen er ikke dermed tom eller frigørbar.';}
 if(state.metric===APFS&&!state.overview){const explanation=[$('#inspect-category').textContent,c.detail].filter(Boolean).join(' · ');$('#inspect-category').title=explanation;$('#inspect-category').setAttribute('aria-label',explanation);}else $('#inspect-category').removeAttribute('aria-label');
 if(state.overview){$('#inspect-logical').textContent='APFS-kapacitet '+formatBytes(state.machine.capacity_bytes);$('#inspect-allocated').textContent='Ledigt '+formatBytes(state.machine.free_bytes);}
 $('#copy').disabled=state.overview;$('#reveal').disabled=state.overview;
}
function renderMode(){
 const changes=state.mode==='changes',active=state.mode==='insights',profiling=state.mode==='files'&&profile.active;
 $('#profile-panel').classList.toggle('hidden',!profiling);$('#profile-entry').classList.toggle('hidden',changes||active||profiling);
 $('#find-files').disabled=!state.view||state.overview;$('#profile-hint').textContent=state.overview?'Vælg en scannet volumen i grafen eller listen for at finde filer.':!state.view?'Vælg en gemt måling for filanalyse.':'Filstørrelse, alder og endelser fra den gemte måling.';
 for(const mode of ['files','changes','insights'])$('#'+mode+'-tab').setAttribute('aria-pressed',String(state.mode===mode));
 $('#file-controls').classList.toggle('hidden',changes||active||profiling);$('#compare-controls').classList.toggle('hidden',!changes);
 $('#insights-panel').classList.toggle('hidden',!active);
 $('#growth-panel').classList.toggle('hidden',!changes);
 for(const selector of ['.list-head','#rows','#more','#list-message'])$(selector).classList.toggle('hidden',active||changes||profiling||selector==='#more'&&!state.cursor||selector==='#list-message'&&state.rows.length>0);
 $('#insights-hint').classList.toggle('hidden',!!state.view&&!state.overview);
 $('#insights-hint').textContent=state.overview?'Indsigter gælder én gemt filscanning. Vælg en scannet volumen i grafen eller listen.':'Vælg en gemt måling for at se indsigter.';
}

async function loadProfile(next=false){
 if(state.overview||!state.view||state.mode!=='files'||!profile.active)return;
 ++state.listRequest;clearTimeout(state.searchTimer);
 const pending=profile.load(args=>invoke('file_profile',args),{scanId:state.scan,entry:Number(profile.scope==='all'?'0':state.view.selected.entry_id),metric:state.metric},next);
 renderFileProfile();if(await pending&&state.mode==='files'&&profile.active)renderFileProfile();
}
function renderFileProfile(){
 if(!state.view)return;
 renderProfile($('#profile-panel'),profile,{selected:state.view.selected,load:loadProfile,close:()=>{profile.reset({active:false});renderMode();renderRows();$('#find-files').focus();},open:row=>{
  profile.reset({active:false});state.mode='files';renderMode();
  if(row.drilldown_scan_id===state.scan)navigate(row.drilldown_entry_id);else openScan(row.drilldown_scan_id,false,row.drilldown_entry_id);
 }});
}
$('#find-files').addEventListener('click',()=>{if(!state.view||state.overview)return;profile.reset({active:true});renderMode();loadProfile();$('#profile-scope')?.focus();});

async function loadInsights(next=false){
 if(state.overview||!state.view||state.mode!=='insights')return;
 ++state.listRequest;clearTimeout(state.searchTimer);
 const pending=insights.load(args=>invoke('insights',args),{scanId:state.scan,entry:Number(insights.scope==='all'?'0':state.view.selected.entry_id),metric:state.metric},next);
 renderInsights();if(await pending)renderInsights();
}
function renderInsights(){
 const panel=$('#insights-panel');panel.replaceChildren();panel.setAttribute('aria-busy',String(insights.busy));
 const label=el('label','','Område for indsigter');label.htmlFor='insights-scope';
 const scope=el('select');scope.id='insights-scope';scope.append(new Option('Hele målingen','all'),new Option('Aktuel mappe / projekt: '+(state.view.selected.name_display||rootName(state.view.scope.root_display)),'current'));scope.value=insights.scope;
 scope.addEventListener('change',()=>{insights.reset({scope:scope.value,category:null});loadInsights();});panel.append(label,scope);
 if(insights.category){const back=el('button','more','← Alle kategorier');back.addEventListener('click',()=>{insights.reset({category:null});loadInsights();});panel.append(back,el('h2','',insightLabel(insights.category)));}
 const status=el('p','insights-status');status.setAttribute('role','status');panel.append(status);
 if(insights.busy){status.textContent='Beregner indsigter fra den gemte måling…';return;}
 if(insights.error){status.textContent='Indsigter kunne ikke beregnes. Ved hukommelsesgrænsen: vælg en mindre mappe i grafen og vælg Aktuel mappe / projekt ovenfor. Du kan også prøve igen.';status.title=insights.error;const retry=el('button','more','Prøv igen');retry.addEventListener('click',()=>loadInsights());panel.append(retry);return;}
 const data=insights.data;if(!data)return;
 status.textContent='Målt '+date(data.measured_at??data.started_at)+' · ikke live · '+metricLabel(data.metric)+' · '+(data.coverage?.complete?'Komplet dækning':'Delvis dækning');
 panel.append(el('p','insights-scope','Område: '+data.scope.path_display),el('p','insights-total','Alle almindelige filer i området: '+insightBytes(data.totals)+' · '+data.totals.file_path_count+' filstier'));
 if(data.context_coverage==='context_unavailable')panel.append(el('p','insights-notice','Denne ældre måling mangler gemt kontekst. Formål kan derfor ikke underbygges.'));
 panel.append(el('p','insights-notice','Størrelser er ikke frigørbar APFS-plads. Ukendt dækning vises som et observeret minimum. Ingen forslag udføres, og formål er ikke en godkendelse til sletning.'));
 if(data.mode==='candidates')panel.append(el('p','insights-notice','Bidraget kan udelade indlejrede områder med en anden kategori. Den almindelige mappevisning kan derfor vise en anden størrelse. Stiens evidens kan ligge over det valgte område; filanalysen åbner inden for området.'));
 const list=el('div','insight-rows');list.setAttribute('role','list');panel.append(list);
 for(const row of data.records){
  const item=el('article','insight-row');item.setAttribute('role','listitem');
  const button=el('button','insight-open',data.mode==='categories'?insightLabel(row.category):row.path_display);button.title=data.mode==='categories'?'Se bidragende stier':'Åbn filanalyse';
  button.addEventListener('click',()=>{const target=insightTarget(data.mode,row);if(target.category){insights.reset({category:target.category});loadInsights();}else{insights.reset();state.mode='files';navigate(target.entry);}});
  item.append(button,el('p','insight-size',insightBytes(row)+' · '+row.file_path_count+' filstier'),el('p','',row.explanation),el('p','',insightGuidance(row.category,row.guidance)),el('p','muted',contextLabel(row.context_coverage)));
  if(data.mode==='candidates'&&row.drilldown_entry_id!=null){const measure=el('button','more focused-rescan','Genmål dette område…');measure.disabled=state.scanning;measure.addEventListener('click',()=>focusedRescan.open(data.scan_id??state.view.scan_id,row.drilldown_entry_id));item.append(measure);}
  const details=el('details'),summary=el('summary','','Om klassifikationen');details.append(summary,el('p','',row.rule_id+' · '+row.rule_version+' · '+row.evidence));item.append(details);list.append(item);
 }
 if(!data.records.length)panel.append(el('p','insights-notice','Ingen observerede almindelige filer i dette udvalg. Ved delvis dækning beviser dette ikke, at området er tomt.'));
 if(insights.cursor){const more=el('button','more','Næste side');more.addEventListener('click',()=>{loadInsights(true);panel.scrollTop=0;});panel.append(more);}
 const details=el('details'),summary=el('summary','','Om optælling og fortolkning');details.append(summary,el('p','',data.count_note),el('p','',data.attribution_note));panel.append(details);
}
function renderCompareOptions(){
 const select=$('#before-scan');select.replaceChildren();
 const eligible=state.scans.filter(s=>sameRootBaseline(s,state.view,state.scan,state.metric));
 if(!eligible.length){select.append(new Option('Ingen tidligere kandidat til sammenligning',''));state.compareBefore=null;return;}
 for(const s of eligible)select.append(new Option(date(s.started_at)+(s.status==='partial'?' · delvis':'')+' · '+s.scan_id,s.scan_id));
 state.compareBefore=eligible.some(s=>s.scan_id===state.compareBefore)?state.compareBefore:eligible[0].scan_id;select.value=state.compareBefore;
 select.title='Tidligere målinger med samme rod og politik. API’en kontrollerer også OS, volumen og gemt format.';
}
async function loadRows(append=false){
 if(state.overview||state.mode==='insights'||state.mode==='files'&&profile.active)return;
 if(state.mode==='changes')return loadGrowth(append);
 const token=++state.listRequest;clearError();try{
 const r=await invoke('list_children',{scanId:state.scan,entry:Number(state.view.selected.entry_id),metric:state.metric,cursor:append?state.cursor:null,search:$('#search').value});
 if(token!==state.listRequest||profile.active||state.mode!=='files')return;state.cursor=r.next_cursor;state.rows=append?[...state.rows,...r.records]:r.records;renderRows();
 }catch(e){if(token!==state.listRequest)return;state.rows=[];state.cursor=null;renderRows();error(e);}
}
async function loadGrowth(next=false){
 if(state.overview||!state.view||state.mode!=='changes')return;
 ++state.listRequest;clearTimeout(state.searchTimer);
 if(!state.compareBefore){growth.reset();renderGrowth();return;}
 const pending=growth.load(args=>invoke('growth',args),{before:state.compareBefore,after:state.scan,entry:Number(growth.scope==='all'?'0':state.view.selected.entry_id),metric:state.metric},next);
 renderGrowth();if(await pending&&state.mode==='changes')renderGrowth();
}
function renderGrowth(){
 const panel=$('#growth-panel'),focused=panel.contains(document.activeElement),activeId=focused?document.activeElement.id:null;panel.replaceChildren();panel.setAttribute('aria-busy',String(growth.busy));
 const selectControl=(id,label,options,value,onchange)=>{
  const l=el('label','',label);l.htmlFor=id;const select=el('select');select.id=id;
  for(const [text,key] of options)select.append(new Option(text,key));select.value=value;select.addEventListener('change',()=>onchange(select.value));panel.append(l,select);
 };
 selectControl('growth-scope','Område for ændringer',[['Hele målingen','all'],['Aktuel mappe / projekt: '+(state.view.selected.name_display||rootName(state.view.scope.root_display)),'current']],growth.scope,value=>{growth.reset({scope:value,group:null,files:false});loadGrowth();});
 selectControl('growth-grouping','Fordel bidrag efter',[['Område','area'],['Kategori','category']],growth.grouping,value=>{growth.reset({grouping:value,group:null,files:false});loadGrowth();});
 selectControl('growth-change','Filændringer',[['Alle ændringer','all'],['Vokset','grown'],['Krympet','shrunk'],['Nye','added'],['Fjernede','removed'],['Ukendte','unknown']],growth.change,value=>{growth.reset({change:value});loadGrowth();});
 if(focused)(activeId?document.getElementById(activeId):panel)?.focus({preventScroll:true});
 if(growth.files){const back=el('button','more','← Tilbage til grupper');back.addEventListener('click',()=>{growth.reset({files:false,group:null});loadGrowth();});panel.append(back);}
 else{const all=el('button','more','Se alle ændrede filer');all.addEventListener('click',()=>{growth.reset({files:true,group:null});loadGrowth();});panel.append(all);}
 const status=el('p','insights-status');status.setAttribute('role','status');panel.append(status);
 if(!state.compareBefore){status.textContent=(state.metric===APFS?'APFS DATA kræver to komplette målinger med understøttede versioner og sammenligningsklar klonmetadata uden fallback. Brug Allokeret eller Logisk, eller genmål samme rod med APFS-klonmetadata. ':'')+'Vælg to gemte målinger af samme rod. Ingen tidligere kandidat er tilgængelig. En oprindelig måling af en bredere rod kan ikke sammenlignes direkte; gentag denne rod for at få en sammenlignelig måling.';return;}
 if(growth.busy){status.textContent='Beregner ændringsbidrag fra de gemte målinger…';return;}
 if(growth.error){status.textContent='Ændringer kunne ikke beregnes. Parret kan være inkompatibelt eller mangle gemt kontekst. Ved hukommelsesgrænsen: vælg en mindre mappe i grafen, vælg Aktuel mappe / projekt og prøv igen.';panel.append(el('p','insights-notice',growth.error));const retry=el('button','more','Prøv igen');retry.addEventListener('click',()=>loadGrowth());panel.append(retry);return;}
 const data=growth.data;if(!data)return;
 const savedDate=value=>value?new Date(value).toLocaleString('da-DK',{dateStyle:'medium',timeStyle:'long'}):'Ukendt afslutningstid';
 status.textContent='Før: '+savedDate(data.interval.before_measured_at)+' · Efter: '+savedDate(data.interval.after_measured_at)+' · '+metricLabel(data.metric);
 panel.append(el('p','insights-scope','Interval: '+elapsedLabel(data.interval.elapsed_ns)),el('p','insights-scope','Område: '+data.scope.after.path_display));
 panel.append(el('p',data.coverage.complete?'insights-status':'insights-notice',data.coverage.complete?'Komplet dækning af det valgte område i begge målinger.':'Delvis dækning. Ukendte sider og ændringer er ikke nul eller en sikker nedgang. Komplette undermapper kan stadig undersøges.'));
 const before=data.totals.before,after=data.totals.after;
 panel.append(el('p','insights-total','Alle almindelige filer i området (uafhængigt af filter):'),el('p','','Før: '+knownBytes(before.bytes,before.observed_lower_bound_bytes)+' · '+before.file_path_count+' observerede filstier'),el('p','','Efter: '+knownBytes(after.bytes,after.observed_lower_bound_bytes)+' · '+after.file_path_count+' observerede filstier'));
 const contribution=(row,target)=>{
  target.append(el('p','insight-size','Ændringsbidrag: '+signedDelta(row.delta_bytes)),el('p','','Bidragende ændrede filer før: '+knownBytes(row.changed_file_before_bytes,row.observed_changed_file_before_bytes)),el('p','','Bidragende ændrede filer efter: '+knownBytes(row.changed_file_after_bytes,row.observed_changed_file_after_bytes)),el('p','muted','Observerede bidragende filstier: '+growthCounts(row)));
 };
 panel.append(el('h2','','Valgte ændringsbidrag · '+(growth.files?'filer':growth.grouping==='area'?'områder':'kategorier')));contribution(data.contributions,panel);
 if(growth.group)panel.append(el('p','insights-scope','Valgt gruppe: '+(data.grouping==='category'?insightLabel(growth.group):growth.groupLabel??growth.group)));
 const list=el('div','insight-rows');list.setAttribute('role','list');panel.append(list);
 for(const row of data.records){
  const item=el('article','insight-row');item.setAttribute('role','listitem');const files=data.mode==='files';
  const button=el('button','insight-open',files||data.grouping==='area'?row.path_display:insightLabel(row.category));button.title=files?'Åbn filanalyse fra den gemte måling':'Se gruppens ændrede filer';
  button.addEventListener('click',()=>{
   if(!files){growth.reset({files:true,group:row.group_key,groupLabel:data.grouping==='category'?insightLabel(row.category):row.path_display});loadGrowth();return;}
   const target=growthTarget(row);if(!target)return;
   if(target.scanId===state.scan){growth.reset();state.mode='files';navigate(target.entry);}else openScan(target.scanId,false,target.entry);
  });item.append(button);
  if(files){item.append(el('p','insight-size',signedDelta(row.delta_bytes)),el('p','','Før: '+knownBytes(row.before_bytes,row.observed_before_bytes)+' · Efter: '+knownBytes(row.after_bytes,row.observed_after_bytes)),el('p','muted',({grown:'Vokset',shrunk:'Krympet',added:'Ny filsti',removed:'Fjernet filsti',unknown:'Ukendt ændring'})[row.change]));button.disabled=!growthTarget(row);item.append(el('p','muted','Filanalyse fra '+(row.drilldown_scan_id===data.before_scan_id?'førmålingen':'eftermålingen')+' · '+row.drilldown_scan_id));}
  else contribution(row,item);
  item.append(el('p','muted',contextLabel(row.context_coverage)));if(data.grouping==='category')item.append(el('p','',insightGuidance(row.category??row.group_key)));list.append(item);
 }
 if(!data.records.length)panel.append(el('p','insights-notice','Ingen observerede ændrede almindelige filer i dette udvalg. Delvis dækning beviser ikke, at der ingen ændringer er.'));
 if(growth.cursor){const more=el('button','more','Næste side');more.addEventListener('click',()=>{loadGrowth(true);panel.scrollTop=0;});panel.append(more);}
 const details=el('details'),summary=el('summary','','Om optælling og tilskrivning');details.append(summary,el('p','',data.accounting_note),el('p','',data.attribution_note),el('p','',data.growth_version+' · '+data.rule_version+' · gemte metadata, ikke live'));panel.append(details);
}
function setScanning(active){state.scanning=active;$('#progress').classList.toggle('hidden',!active);if(!active)$('#branch-progress').classList.add('hidden');
 for(const button of document.querySelectorAll('.focused-rescan'))button.disabled=active;
 for(const id of ['rescan','choose-folder','choose-history','history-location','whole-mac','capture-clones'])$('#'+id).disabled=active;
 $('#stop').disabled=false;$('#stop').textContent='Stop';renderHistory();renderVolumes();}
async function beginScan(root){if(state.scanning)return;clearError();try{await invoke(root===null?'start_machine':'start_scan',root===null?{captureClones:$('#capture-clones').checked}:{root,captureClones:$('#capture-clones').checked});$('#capture-clones').checked=false;setScanning(true);pollJob();}catch(e){error(e);}}
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
 if(['done','error','cancelled'].includes(s.phase)){setScanning(false);await refresh();if(s.phase==='done'){await openScan(s.scan_id);toast(s.retention_warning||(s.whole_mac?'Samlet oversigt gemt. Klik på en volumen for filerne.':'Målingen er gemt lokalt.'));}else if(s.phase==='cancelled')toast('Scanningen er stoppet. Tidligere målinger er bevaret.');else error(s.error);return;}
 setTimeout(pollJob,500);
 }catch(e){setScanning(false);error(e);}}
async function folderScan(){try{const root=await invoke('choose_root');if(root)beginScan(root);}catch(e){error(e);}}
$('#whole-mac').addEventListener('click',()=>beginScan(null));
$('#choose-folder').addEventListener('click',folderScan);$('#rescan').addEventListener('click',()=>state.overview?beginScan(null):state.view?focusedRescan.open(state.view.scan_id??state.scan,state.view.selected.entry_id):beginScan(null));
$('#stop').addEventListener('click',async()=>{await invoke('stop_scan');$('#stop').textContent='Stopper…';$('#stop').disabled=true;});
$('#chart-center').addEventListener('click',()=>{if(state.view?.selected.parent_id!==null)navigate(state.view.selected.parent_id);});
for(const metric of metrics)$('#'+metric).addEventListener('click',()=>{setMetric(metric);state.metricNotice='';if(state.view)navigate(state.view.selected.entry_id);});
$('#files-tab').addEventListener('click',()=>{if(!state.view)return;insights.reset();growth.reset();profile.reset({active:false});state.mode='files';renderMode();loadRows();});$('#changes-tab').addEventListener('click',()=>{if(!state.view)return;insights.reset();growth.reset();profile.reset({active:false});state.mode='changes';renderMode();loadGrowth();});
$('#insights-tab').addEventListener('click',()=>{if(!state.view||state.overview)return;insights.reset({category:null});growth.reset();profile.reset({active:false});state.mode='insights';renderMode();loadInsights();});
$('#before-scan').addEventListener('change',()=>{state.compareBefore=$('#before-scan').value;growth.reset({group:null,files:false});loadGrowth();});$('#more').addEventListener('click',()=>loadRows(true));
$('#search').addEventListener('input',()=>{clearTimeout(state.searchTimer);state.searchTimer=setTimeout(()=>loadRows(),200);});
for(const [id,action]of [['copy','copy'],['reveal','reveal']])$('#'+id).addEventListener('click',()=>runFileAction(action,{scanId:state.scan,entry:Number(state.selected.entry_id)}).catch(error));
async function chooseHistory(){try{const r=await invoke('choose_history');if(!r)return;localStorage.setItem('macscan.history',r.store);state.scans=r.scans;state.store=r.store;$('#history-location').title=r.store;renderHistory();const first=r.scans.find(s=>!s.grouped_volume);if(first)openScan(first.scan_id);else toast('Denne mappe indeholder ingen målinger.');}catch(e){error(e);}}
$('#choose-history').addEventListener('click',chooseHistory);$('#history-location').addEventListener('click',chooseHistory);
$('#privacy-help').addEventListener('click',async()=>{
 try{await invoke('open_privacy');}catch(e){error(e);}
 toast('Tilføj denne Macscan.app i Fuld diskadgang. Efter en lokal genbygning kan du skulle fjerne og tilføje den igen. Genstart appen bagefter.');
});
document.addEventListener('keydown',e=>{if($('#coverage-dialog').open||$('#rescan-dialog').open)return;if(e.metaKey&&e.key==='o'){e.preventDefault();folderScan();}if((e.altKey&&e.key==='ArrowLeft'||e.key==='Backspace')&&!['INPUT','SELECT','TEXTAREA'].includes(e.target.tagName)&&state.view?.selected.parent_id!==null&&state.view){e.preventDefault();navigate(state.view.selected.parent_id);}});
renderChart();
try{const r=await refresh();if(r.explicit_store)localStorage.setItem('macscan.history',r.store);if(!r.explicit_store&&localStorage.getItem('macscan.history')){try{const h=await invoke('restore_history',{path:localStorage.getItem('macscan.history')});state.scans=h.scans;state.store=h.store;$('#history-location').title=h.store;renderHistory();}catch{localStorage.removeItem('macscan.history');}}
 const initial=state.scans.find(s=>!s.grouped_volume);if(initial)await openScan(initial.scan_id);
}catch(e){error(e);}
