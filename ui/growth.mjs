// Exact saved values only: never round deltas through Number or infer unknown sides.
export const exactBytes=value=>value==null?'Ukendt':BigInt(value).toLocaleString('da-DK')+' B';
export const signedDelta=value=>value==null?'Ukendt ændring':(BigInt(value)>0n?'+':'')+exactBytes(value);
export const knownBytes=(value,observed)=>value==null?'Ukendt'+(observed==null?'':' · mindst observeret '+exactBytes(observed)):exactBytes(value);
export const growthTarget=row=>row.drilldown_scan_id!=null&&row.drilldown_entry_id!=null?{scanId:row.drilldown_scan_id,entry:row.drilldown_entry_id}:null;
export const growthCounts=row=>`Nye ${row.new_count} · fjernede ${row.removed_count} · ændrede ${row.changed_count} · ukendte ${row.unknown_count}`;
export function elapsedLabel(ns){
 if(ns==null)return 'Ukendt interval mellem målingernes afslutning';
 let n=BigInt(ns);const days=n/86400000000000n;n%=86400000000000n;const hours=n/3600000000000n;n%=3600000000000n;const minutes=n/60000000000n;n%=60000000000n;
 const parts=[];if(days)parts.push(days+' døgn');if(hours)parts.push(hours+' timer');if(minutes)parts.push(minutes+' min.');if(n||!parts.length)parts.push((n/1000000000n)+((n%1000000000n)?','+(n%1000000000n).toString().padStart(9,'0').replace(/0+$/,''):'')+' sek.');
 return parts.join(' ')+' ('+BigInt(ns).toLocaleString('da-DK')+' ns)';
}
export function createGrowthState(){
 let revision=0,binding=null;
 return {
  scope:'all',grouping:'area',files:false,group:null,change:'all',data:null,cursor:null,busy:false,error:null,
  reset(patch={}){revision++;binding=null;Object.assign(this,{data:null,cursor:null,busy:false,error:null},patch);},
  async load(fetch,query,next=false){
   const args={...query,grouping:this.grouping,files:this.files,group:this.group,change:this.change};
   const key=JSON.stringify(args),cursor=next&&key===binding?this.cursor:null,token=++revision;
   binding=key;this.busy=true;this.error=null;this.data=null;this.cursor=null;
   try{const data=await fetch({...args,cursor});if(token!==revision)return false;this.data=data;this.cursor=data.next_cursor??null;}
   catch(e){if(token!==revision)return false;this.error=String(e);}
   finally{if(token===revision)this.busy=false;}
   return token===revision;
  }
 };
}
