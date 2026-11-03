import {APFS,apfsComparable} from './metrics.mjs';
// A preparation response is metadata only. Closing invalidates pending responses.
export function createFocusedRescan(dialog,{invoke,onStarted,onError,isScanning}){
 let generation=0,target=null;
 const title=document.createElement('h2');title.textContent='Genmål valgt område';
 const body=document.createElement('div'),cancel=document.createElement('button'),start=document.createElement('button');
 cancel.textContent='Annuller';start.textContent='Start genmåling';start.className='primary';
 dialog.append(title,body,cancel,start);
 const close=()=>{++generation;target=null;dialog.close();};
 dialog.addEventListener('cancel',e=>{e.preventDefault();close();});cancel.addEventListener('click',close);
 start.addEventListener('click',async()=>{
  if(!target||isScanning())return;
  const chosen=target;start.disabled=true;
  try{await invoke('start_rescan',{...chosen,captureClones:body.querySelector('#rescan-capture').checked});close();onStarted();}catch(e){start.disabled=false;onError(e);}
 });
 return {close,async open(scanId,entry){
  if(isScanning())return;
  const token=++generation;target=null;body.textContent='Henter den valgte gemte sti…';start.disabled=true;dialog.showModal();
  try{const prepared=await invoke('prepare_rescan',{scanId,entry:Number(entry)});if(token!==generation)return;
   body.replaceChildren();
   for(const text of [prepared.root_display,'Oprindeligt målt '+prepared.source_started_at,
    'Ny selvstændig måling af denne sti. Historikken beholder de seneste tre scanninger. Dækning og tidspunkt gælder kun den nye måling.']){
    const p=document.createElement('p');p.textContent=text;body.append(p);
   }
   const label=document.createElement('label'),capture=document.createElement('input');capture.type='checkbox';capture.id='rescan-capture';capture.checked=false;label.append(capture,' APFS-klonmetadata til denne nye måling (fra som standard)');body.append(label);
   target={scanId,entry:Number(entry)};
   start.disabled=false;
  }catch(e){if(token===generation){body.textContent=String(e);start.disabled=true;}}
 }};
}
export function rescanLabel(selected){return selected.kind==='file'?'Genmål denne fil':String(selected.entry_id)==='0'?'Genmål hele målingen':'Genmål denne mappe';}
export function sameRootBaseline(scan,view,current,metric='allocated'){
 if(metric===APFS&&(!apfsComparable(scan)||!apfsComparable(view)))return false;
 if(scan.kind==='machine'||scan.scan_id===current)return false;
 const before=scan.context?.started_unix_ns,after=view.measurement_context?.started_unix_ns;
 if(typeof before!=='string'||typeof after!=='string'||!/^\d+$/.test(before)||!/^\d+$/.test(after))return false;
 try{if(BigInt(before)>=BigInt(after))return false;}catch{return false;}
 if(!view.scope.root_path_bytes_hex||scan.root_path_bytes_hex!==view.scope.root_path_bytes_hex||scan.policy!==view.scope.policy_fingerprint)return false;
 const a=scan.context,b=view.measurement_context;
 return Boolean(a?.os_identity&&a.os_identity===b?.os_identity&&a.volume_before?.volume_uuid&&
  a.volume_before.volume_uuid===a.volume_after?.volume_uuid&&a.volume_before.volume_uuid===b.volume_before?.volume_uuid&&a.volume_before.volume_uuid===b.volume_after?.volume_uuid);
}
