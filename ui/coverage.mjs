// Whole saved measurement counters never depend on the retained example page.
export const reasons=Object.freeze({access_denied:'Manglende adgang',provider_excluded:'Cloud-/providerområde udeladt',volume_boundary:'Volumengrænse',firmlink_boundary:'Firmlink-grænse',scanner_output:'Scannerens egne måledata',disappeared:'Forsvundet under målingen',explicit_limit:'Eksplicit grænse eller udeladelse',io_error:'Anden I/O-fejl',volume_changed:'Volumen ændret'});
export const count=value=>value==null?'Ukendt':String(value);
export function volumeStatus(row){
 if(row.phase==='done')return row.scan_status==='complete'?'Gennemgang afsluttet':row.scan_status==='partial'?'Gennemgang afsluttet · delvis dækning':'Gennemgang afsluttet · dækning ukendt';
 return ({error:'Udførelse mislykkedes',unmounted:'Ikke monteret',cancelled:'Stoppet',pending:'Ikke gennemgået',not_scanned:'Ikke gennemgået'})[row.phase]??'Ukendt gemt tilstand';
}
export const reportStatus=status=>({complete:'Gennemgang afsluttet.',partial:'Delvis dækning af den gemte måling. Ukendte områder er ikke nul.',cancelled:'Målingen blev stoppet. Ukendte områder er ikke nul.',error:'Udførelse mislykkedes. Ukendte områder er ikke nul.'})[status]??'Gemt målingstilstand er ukendt. Ukendte områder er ikke nul.';
export const sampleTarget=(report,row)=>row.path_available&&row.saved_entry_id!=null?{scanId:report.scan_id,entry:row.saved_entry_id}:null;
export function createCoverageState(){
 let revision=0;
 return {scanId:null,reason:null,data:null,cursor:null,busy:false,error:null,
  reset(values={}){++revision;Object.assign(this,{scanId:null,reason:null,data:null,cursor:null,busy:false,error:null},values);},
  async load(fetch,next=false){const token=++revision,cursor=next?this.cursor:null;this.busy=true;this.error=null;this.data=null;
   try{const data=await fetch({scanId:this.scanId,reason:this.reason,cursor});if(token!==revision)return false;this.data=data;this.cursor=data.next_cursor??null;}
   catch(e){if(token!==revision)return false;this.error=String(e);this.cursor=null;}
   finally{if(token===revision)this.busy=false;}return token===revision;
  }
 };
}
const node=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
const stamp=value=>value?new Date(value).toLocaleString('da-DK',{dateStyle:'medium',timeStyle:'long'}):'Ukendt';
export function createCoverageDialog(dialog,{fetch,openEntry,privacy}){
 const state=createCoverageState();let machine=null,root='',opener=null,focusControl=null,machineOffset=0;
 const body=dialog.querySelector('#coverage-body');
 const button=(label,fn)=>{const b=node('button',label,'more');b.addEventListener('click',fn);return b;};
 const p=text=>body.append(node('p',text));
 function close(){state.reset();machine=null;if(dialog.open)dialog.close();}
 dialog.addEventListener('close',()=>{state.reset();machine=null;opener?.focus({preventScroll:true});});
 dialog.addEventListener('cancel',()=>state.reset());
 dialog.addEventListener('keydown',event=>{
  if(event.key!=='Tab')return;
  const controls=[...dialog.querySelectorAll('button,select,input,a[href],[tabindex]:not([tabindex="-1"])')].filter(n=>!n.disabled&&n.getClientRects().length);
  const index=controls.indexOf(document.activeElement);
  if(controls.length&&(index<0||event.shiftKey&&index===0||!event.shiftKey&&index===controls.length-1)){
   event.preventDefault();controls[event.shiftKey?controls.length-1:0].focus();
  }
 });
 async function load(next=false){const promise=state.load(fetch,next);render();if(await promise&&dialog.open)render();}
 function report(scanId,rootDisplay){focusControl=null;root=rootDisplay??'Ukendt gemt rod';state.reset({scanId});load();}
 function render(){
  if(body.contains(document.activeElement)&&document.activeElement.id)focusControl=document.activeElement.id;body.replaceChildren();body.setAttribute('aria-busy',String(state.busy));
  if(machine){body.append(button('← Tilbage til volumener',()=>{state.reset();renderMachine();}));}
  p('Hele den gemte måling: '+root+' · '+state.scanId);
  const status=node('p',undefined,'coverage-status');status.setAttribute('role','status');body.append(status);
  if(state.busy){status.textContent='Henter gemt dækningsrapport…';return;}
  if(state.error){status.textContent='Dækningsrapporten kunne ikke hentes: '+state.error;body.append(button('Prøv igen',()=>load()));return;}
  const data=state.data;if(!data)return;
  p('Start: '+stamp(data.started_at)+' · Slut: '+stamp(data.ended_at)+' · ikke live');
  if(data.read_time_global_invalidation){const warning=node('p',data.coverage_note_da??'Gemt volumenidentitet ændrede sig; hele målingens dækning er ukendt.','insights-notice');warning.setAttribute('role','note');body.append(warning);}
  p(reportStatus(data.status));
  p('Registrerede hændelser: '+count(data.error_count)+' · heraf udeladelser: '+count(data.excluded_count)+'. Hændelser er ikke unikke områder eller filer; samme post kan optræde flere gange.');
  const legacy=data.quality!=='event_counts';
  if(legacy)p('Ældre evidens: årsagsfordelingen er ukendt. Kun gemte eksempler er tilgængelige; de klassificeres ikke bagefter. Delvis dækning og evidensens alder er separate oplysninger.');
  const table=node('table');table.className='coverage-counts';const caption=node('caption','Årsager · hele målingen, uafhængigt af eksempeludvalg');table.append(caption);const tbody=node('tbody');
  for(const [key,label]of Object.entries(reasons)){const tr=node('tr'),th=node('th',label),td=node('td',legacy?'Ukendt':count(data.reason_counts?.[key]));th.scope='row';tr.append(th,td);tbody.append(tr);}table.append(tbody);body.append(table);
  if(!legacy){const label=node('label','Filtrér gemte eksempler');label.htmlFor='coverage-reason';const select=node('select');select.id='coverage-reason';select.append(new Option('Alle årsager',''));for(const [key,text]of Object.entries(reasons))select.append(new Option(text,key));select.value=state.reason??'';select.addEventListener('change',()=>{state.reset({scanId:data.scan_id,reason:select.value||null});load();});body.append(label,select);}
  if(data.samples_truncated)p('Kun et begrænset antal eksempler blev gemt. Flere registrerede hændelser har ingen gemte eksempler; næste side kan kun vise bevarede eksempler.');
  const list=node('ul');list.className='coverage-examples';body.append(list);
  for(const row of data.records??[]){const item=node('li');item.append(node('strong',reasons[row.reason]??'Ukendt årsag · ældre eksempel'),node('p',row.path_available?row.path_display:'Sti utilgængelig i gemte data'));
   if(row.path_context==='saved_parent_and_raw_child')item.append(node('p','Fejlet underpost; ID henviser til den gemte overordnede mappe.'));
   if(row.path_truncated)item.append(node('p','Stien er forkortet. Navigation bruger den gemte post, aldrig den forkortede sti.'));
   if(row.child_identity_invalid||row.child_identity_truncated)item.append(node('p','Underpostens præcise navn er utilgængeligt eller forkortet.'));
   item.append(node('p',[row.raw_code??row.code,row.operation,row.detail].filter(Boolean).join(' · ')));
   if(row.detail_truncated)item.append(node('p','Detaljen er forkortet.'));
   if(row.next_step_da)item.append(node('p',row.next_step_da));
   const target=sampleTarget(data,row);if(target)item.append(button('Åbn gemt post/overordnet mappe',()=>{close();openEntry(target);}));list.append(item);
  }
  status.textContent=data.records?.length?'Gemte eksempler · denne side erstatter den forrige.':!legacy&&data.error_count==='0'?'Ingen registrerede hændelser i denne måling. Det beviser ikke, at alt på Mac’en er dækket.':'Ingen gemte eksempler i dette udvalg. Ukendt eller manglende evidens er ikke nul hændelser.';
  if(data.truncated&&state.cursor)body.append(button('Næste side med gemte eksempler',()=>load(true)));
  if(!legacy&&data.reason_counts?.access_denied!=='0'&&data.reason_counts?.access_denied!=null)body.append(button('Åbn Fuld diskadgang…',privacy));
  if(focusControl)body.querySelector('#'+focusControl)?.focus({preventScroll:true});
 }
 function renderMachine(){
  body.replaceChildren();body.setAttribute('aria-busy','false');p('Gemte volumener · start: '+stamp(machine.started_at)+' · slut: '+stamp(machine.ended_at)+' · ikke live');p('Volumentilstande er gemt med denne oversigt. Ukendte eller ældre årsagstal lægges ikke sammen.');
  const list=node('ul');body.append(list);
  const volumes=machine.volumes??[];p('Gemte volumenrækker '+(volumes.length?machineOffset+1:0)+'–'+Math.min(machineOffset+10,volumes.length)+' af '+volumes.length+'. Hver side erstatter den forrige.');
  for(const row of volumes.slice(machineOffset,machineOffset+10)){const item=node('li');item.append(node('strong',row.name??row.device),node('p',(row.root??'Ingen gemt monteringssti')+' · '+volumeStatus(row)));if(row.error)item.append(node('p',row.error));if(row.phase==='done'&&row.scan_id)item.append(button('Se dækning · '+(row.name??row.device),()=>report(row.scan_id,row.root)));list.append(item);}
  if(machineOffset>0)body.append(button('Forrige side med volumener',()=>{machineOffset=Math.max(0,machineOffset-10);renderMachine();dialog.scrollTop=0;}));
  if(machineOffset+10<volumes.length)body.append(button('Næste side med volumener',()=>{machineOffset+=10;renderMachine();dialog.scrollTop=0;}));
 }
 return {close,open(scanId,rootDisplay){opener=document.activeElement;machine=null;report(scanId,rootDisplay);if(!dialog.open)dialog.showModal();dialog.scrollTop=0;},openMachine(session){opener=document.activeElement;state.reset();focusControl=null;machineOffset=0;machine=session;renderMachine();if(!dialog.open)dialog.showModal();dialog.scrollTop=0;}};
}
