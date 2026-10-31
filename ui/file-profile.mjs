// Saved metadata only. Exact API integers stay decimal strings at the boundary.
const U64_MAX=18446744073709551615n;
export function integerFilter(value,multiplier=1n){
 if(value==='')return null;
 if(!/^\d+$/.test(value))throw Error('Brug et ikke-negativt heltal; tøm feltet for alle værdier.');
 const n=BigInt(value)*multiplier;if(n>U64_MAX)throw Error('Filteret er større end den understøttede u64-grænse.');return n.toString();
}
export function profileBytes(row){
 const exact=row.bytes,observed=row.observed_lower_bound_bytes;
 const label=n=>BigInt(n).toLocaleString('da-DK')+' B';
 return exact!=null?label(exact):observed!=null?'Ukendt · mindst observeret '+label(observed):'Ukendt størrelse';
}
export function savedTime(seconds,future=null){
 if(seconds==null)return 'Ukendt ændringstid';
 const n=BigInt(seconds),ms=n*1000n;
 const time=ms>=-8640000000000000n&&ms<=8640000000000000n?new Date(Number(ms)).toISOString():seconds+' Unix-sekunder (uden for datointerval)';
 return time+(future===true?' · fremtidig i målingen':future===null?' · forhold til referencetid ukendt':'');
}
export function referenceTime(ns,exact=true){
 if(ns==null)return 'Ukendt';
 return savedTime((BigInt(ns)/1000000000n).toString(),false)+(exact?' · '+ns+' ns':'');
}
export function createProfileState(){
 let revision=0;
 return {active:false,scope:'current',age:'',size:'',unit:'1048576',types:false,sortCount:false,typeKey:null,typeLabel:null,data:null,cursor:null,busy:false,error:null,
  reset(patch={}){revision++;Object.assign(this,{data:null,cursor:null,busy:false,error:null},patch);},
  async load(fetch,query,next=false){
   const token=++revision,cursor=next?this.cursor:null;this.busy=true;this.error=null;this.data=null;this.cursor=null;
   try{const minAgeDays=integerFilter(this.age),minBytes=integerFilter(this.size,BigInt(this.unit));
    const data=await fetch({...query,minAgeDays,minBytes,typeKey:this.typeKey,types:this.types,sortCount:this.types&&this.sortCount,cursor});
    if(token!==revision)return false;this.data=data;this.cursor=data.next_cursor??null;
   }catch(e){if(token!==revision)return false;this.error=String(e);}
   finally{if(token===revision)this.busy=false;}return token===revision;
  }
 };
}

export function renderProfile(panel,profile,{selected,load,close,open}){
 const focus=panel.contains(document.activeElement)?document.activeElement.id||'profile-panel':null;
 const selection=focus&&document.activeElement.selectionStart;
 const signature=JSON.stringify([selected.entry_id,selected.name_display,selected.kind,profile.types,profile.typeKey,profile.typeLabel]);
 const existing=panel.querySelector('.profile-controls'),stable=existing?.dataset.signature===signature;
 for(const child of [...panel.children])if(!stable||child!==existing)child.remove();
 panel.setAttribute('aria-busy',String(profile.busy));
 const el=(tag,text,cls)=>{const n=document.createElement(tag);if(text!=null)n.textContent=text;if(cls)n.className=cls;return n;};
 let controls=stable?existing:el('div',null,'profile-controls');controls.dataset.signature=signature;
 if(!stable)panel.append(controls);
 const button=(id,label,action)=>{const b=el('button',label,'more');b.id=id;b.addEventListener('click',action);controls.append(b);return b;};
 const control=(tag,id,label)=>{const l=el('label',label);l.htmlFor=id;const n=el(tag);n.id=id;controls.append(l,n);return n;};
 const update=patch=>{profile.reset(patch);load();};
 if(!stable){
 button('profile-close','← Direkte underposter',close);
 controls.append(el('h2','Find filer i gemt måling'));
 const scope=control('select','profile-scope','Område');scope.append(new Option('Aktuel mappe / projekt: '+selected.name_display,'current'),new Option('Hele målingen','all'));scope.value=profile.scope;scope.addEventListener('change',()=>update({scope:scope.value}));
 const age=control('input','profile-age','Ikke ændret i mindst N dage (tom = alle aldre)');age.type='text';age.inputMode='numeric';age.value=profile.age;age.addEventListener('change',()=>update({age:age.value}));
 const size=control('input','profile-size','Mindste størrelse (heltal; tom = alle størrelser)');size.type='text';size.inputMode='numeric';size.value=profile.size;size.addEventListener('change',()=>update({size:size.value}));
 const unit=control('select','profile-unit','Størrelsesenhed');for(const [name,key]of [['B','1'],['KiB (1.024 B)','1024'],['MiB (1.048.576 B)','1048576'],['GiB (1.073.741.824 B)','1073741824']])unit.append(new Option(name,key));unit.value=profile.unit;unit.addEventListener('change',()=>update({unit:unit.value}));
 const downloads=selected.kind==='directory'&&selected.name_display==='Downloads';
 button('profile-preset',downloads?'Downloads: mindst 100 MiB og 180 dage':'Store, gamle filer: mindst 100 MiB og 180 dage',()=>update({scope:'current',size:'100',unit:'1048576',age:'180',typeKey:null,typeLabel:null,types:false,sortCount:false}));
 if(!downloads)controls.append(el('p','For Downloads: vælg først den gemte Downloads-mappe i grafen eller underposter. Ingen aktuel mappe slås op.','insights-notice'));
 const view=control('select','profile-view','Vis');view.append(new Option('Filer','files'),new Option('Filtyper (gemte endelser)','types'));view.value=profile.types?'types':'files';view.addEventListener('change',()=>update({types:view.value==='types',sortCount:false,typeKey:null,typeLabel:null}));
 if(profile.types){const sort=control('select','profile-sort','Sorter filtyper efter');sort.append(new Option('Bytes','bytes'),new Option('Antal filstier','count'));sort.value=profile.sortCount?'count':'bytes';sort.addEventListener('change',()=>update({sortCount:sort.value==='count'}));}
 if(profile.typeKey!==null){controls.append(el('p','Valgt type: '+profile.typeLabel+' · '+profile.typeKey,'insights-scope'));button('profile-clear-type','← Alle filtyper',()=>update({typeKey:null,typeLabel:null,types:true,sortCount:false}));}
 }
 for(const [id,value]of [['profile-age',profile.age],['profile-size',profile.size],['profile-unit',profile.unit],['profile-scope',profile.scope]]){const input=controls.querySelector('#'+id);if(input!==document.activeElement||input.tagName!=='INPUT')input.value=value;}
 // Result buttons belong to results, while unchanged form controls retain DOM identity.
 controls=panel;
 const status=el('p',null,'insights-status');status.setAttribute('role','status');panel.append(status);
 if(profile.busy)status.textContent='Beregner fra gemte metadata…';
 else if(profile.error){status.textContent='Filanalysen kunne ikke beregnes: '+profile.error;
  panel.append(el('p','Ved manglende gemt referencetid: ryd aldersfilteret og prøv igen. Ved ressourcegrænsen: vælg en mindre gemt mappe i grafen og Aktuel mappe / projekt.','insights-notice'));
  button('profile-clear-age','Ryd aldersfilter og prøv igen',()=>update({age:''}));button('profile-retry','Prøv igen',()=>load());
 }else if(profile.data){const d=profile.data,t=d.totals,p=d.population;
  status.textContent='Målt '+(d.measured_at??d.started_at??'ukendt')+' · ikke live · '+(d.metric==='logical'?'Logisk':'Allokeret');
  panel.append(el('p','Område: '+d.scope.path_display,'insights-scope'),el('p','Gemt referencetid: '+referenceTime(d.reference_unix_ns,false)),el('p','Aldersgrænse: '+(d.age_threshold_unix_ns==null?'Alle aldre':referenceTime(d.age_threshold_unix_ns,false))));
  panel.append(el('p','Hele det filtrerede observerede udvalg (alle sider): '+profileBytes(t)+' · '+t.file_path_count+(d.coverage?.complete?' filstier':' observerede filstier (nedre grænse)'),'insights-total'));
  panel.append(el('p',d.coverage?.complete?'Komplet dækning af valgt område.':'Delvis dækning: ukendt total, observerede bytes og antal er nedre grænser. Komplette undermapper kan undersøges.','insights-notice'));
  const population=el('p','Før filtre: '+p.file_path_count+' observerede filstier · ukendt ændringstid '+p.unknown_mtime_count+' · fremtidig ændringstid '+p.future_mtime_count+' · tidsforhold ikke vurderet '+p.unassessed_mtime_count+' · ukendt størrelse '+p.unknown_size_count+' · ukendt type '+p.unknown_type_count);
  const hasUnknown=['unknown_mtime_count','future_mtime_count','unassessed_mtime_count','unknown_type_count','unknown_size_count'].some(k=>BigInt(p[k]??'0')>0n);
  if(hasUnknown)panel.append(population);else{const details=el('details');details.append(el('summary','Population før filtre'),population);panel.append(details);}
  const times=el('details');times.append(el('summary','Eksakte gemte tidsværdier'),el('p','Reference: '+(d.reference_unix_ns??'ukendt')+' ns · aldersgrænse: '+(d.age_threshold_unix_ns??'ingen')+' ns'));panel.append(times);
  if(d.filters)panel.append(el('p','Anvendt minimum: '+(d.filters.min_bytes==null?'alle størrelser':BigInt(d.filters.min_bytes).toLocaleString('da-DK')+' B')+' · alder: '+(d.filters.min_age_days==null?'alle aldre':d.filters.min_age_days+' dage')));
  panel.append(el('p','Alder er tid siden ændring, ikke bevis på manglende brug. Et aldersfilter udelader ukendte og fremtidige tider. Størrelsesfilter kræver kendte eksakte bytes. Endelser er gemte navne, ikke MIME. Allokering er ikke frigørbar APFS-plads.','insights-notice'));
  const list=el('div',null,'insight-rows');list.setAttribute('role','list');panel.append(list);
  for(const row of d.records){const item=el('article',null,'insight-row');item.setAttribute('role','listitem');const b=el('button',d.mode==='types'?(row.type_display??'Ukendt type')+' · '+(row.type_key??'ukendt'):row.path_display,'insight-open');b.title=d.mode==='types'?(row.type_key??'Ukendt type'):'Åbn filanalyse fra gemt måling';
   b.disabled=d.mode==='types'?row.type_key==null:row.drilldown_entry_id==null;
   b.addEventListener('click',()=>d.mode==='types'?update({typeKey:row.type_key,typeLabel:row.type_display??row.type_key,types:false,sortCount:false}):open(row));
   item.append(b,el('p',profileBytes(row),'insight-size'));
   item.append(el('p',d.mode==='types'?row.file_path_count+(d.coverage?.complete?' filstier':' observerede filstier (nedre grænse)'):'Ændret: '+savedTime(row.mtime_unix_seconds,row.future_mtime)));
   if(d.mode==='files')item.append(el('p','Type: '+(row.type_display??'Ukendt type')+' · '+(row.type_key??'ukendt'),'muted'));list.append(item);
  }
  if(!d.records.length)panel.append(el('p','Ingen observerede filer matcher. Delvis dækning beviser ikke, at området er tomt.','insights-notice'));
  if(profile.cursor)button('profile-next','Næste side',()=>load(true));
 }
 if(focus){const target=document.getElementById(focus)??panel;if(target!==document.activeElement)target.focus({preventScroll:true});if(typeof selection==='number'&&target.setSelectionRange)target.setSelectionRange(selection,selection);}
}
