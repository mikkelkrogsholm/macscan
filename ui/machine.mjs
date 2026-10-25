// OS capacity and file measurements are deliberately separate accounting layers.
export function machineView(session){
 const volumes=session.volumes??[];
 const sum=volumes.reduce((n,v)=>n+BigInt(v.os_used_bytes??0),0n),used=BigInt(session.used_bytes??0);
 const rows=volumes.map((v,i)=>({entry_id:String(i+1),parent_id:'0',name_display:v.name??v.device,path_display:v.root??'Ikke monteret',kind:'volume',bytes:v.os_used_bytes,observed_bytes:v.os_used_bytes,coverage_complete:v.scan_status==='complete',scan_id:v.scan_id,scan_status:v.scan_status,phase:v.phase,roles:v.roles,logical_bytes:null,accounted_allocated_bytes:null,error:v.error}));
 if(used>sum)rows.push({entry_id:String(rows.length+1),parent_id:'0',name_display:'Øvrig APFS-allokering',kind:'capacity',bytes:String(used-sum),observed_bytes:String(used-sum),coverage_complete:false,path_display:'Snapshots, metadata og anden APFS-allokering; ikke klassificeret som junk.'});
 rows.sort((a,b)=>BigInt(a.bytes)>BigInt(b.bytes)?-1:BigInt(a.bytes)<BigInt(b.bytes)?1:0);
 const root={entry_id:'0',parent_id:null,name_display:'Hele Mac’en',kind:'machine',bytes:String(used>sum?used:sum),observed_bytes:String(used>sum?used:sum),coverage_complete:false,path_display:'Opstartsdisken · APFS-volumener',logical_bytes:null,accounted_allocated_bytes:null};
 return {selected:root,nodes:[root,...rows],children:{records:rows,next_cursor:null},ancestors:[],child_count:rows.length,scope:{root_display:'Hele Mac’en'},entry_count:session.entry_count??0,started_at:session.started_at,status:session.status,issues:[],error_count:0,capacity:session,accountingOverlap:sum>used};
}
export function phaseLabel(row){
 return ({pending:'Afventer',planning:'Læser diske',scanning:'Scanner',saving:'Beregner',traversed:'Gennemgået · foreløbig',done:row.scan_status==='complete'?'Målt':'Målt · delvis',unmounted:'Ikke monteret',cancelled:'Stoppet',not_scanned:'Ikke scannet',error:'Kunne ikke måles'})[row.phase]??row.phase;
}
