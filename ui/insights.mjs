import {formatBytes} from './sunburst.mjs';
const labels={build_output:'Rust-buildoutput',dependencies:'Projektafhængigheder',build_hint:'Muligt buildoutput',dependency_hint:'Mulige afhængigheder',cache_hint:'Mulig cache',xcode_derived_data:'Xcode-builddata',xcode_archives:'Xcode-arkiver',simulator_devices:'Simulatordata',docker_disk:'Docker · virtuel disk',orbstack_disk:'OrbStack · virtuelle diske',ollama_models:'Ollama-modeller',huggingface_models:'Hugging Face-modeller',lmstudio_models:'LM Studio-modeller',package_cache:'Pakkecache',unclassified:'Ukendt formål'};
export const insightLabel=category=>labels[category]??'Ukendt formål';
export const insightBytes=row=>row.bytes==null?(row.observed_lower_bound_bytes==null?'Ukendt':'mindst '+formatBytes(row.observed_lower_bound_bytes)):formatBytes(row.bytes);
export const contextLabel=value=>({complete:'Gemt kontekst dækket',partial:'Delvis kontekst',context_unavailable:'Kontekst mangler i denne ældre måling'})[value]??'Kontekst mangler';
export const insightTarget=(mode,row)=>mode==='categories'?{category:row.category}:{entry:row.drilldown_entry_id};
// Each page replaces the previous one. The opaque cursor advances by the actual
// encoded page, which may be shorter than the requested row budget.
export function createInsightsState(){
 let revision=0;
 return {
  scope:'all',category:null,data:null,cursor:null,busy:false,error:null,
  reset(patch={}){revision++;Object.assign(this,{data:null,cursor:null,busy:false,error:null},patch);},
  async load(fetch,query,next=false){
   const token=++revision,cursor=next?this.cursor:null;
   this.busy=true;this.error=null;this.data=null;
   try{const data=await fetch({...query,category:this.category,cursor});if(token!==revision)return false;
    this.data=data;this.cursor=data.next_cursor??null;
   }catch(e){if(token!==revision)return false;this.error=String(e);this.cursor=null;}
   finally{if(token===revision)this.busy=false;}
   return token===revision;
  }
 };
}
