import test from 'node:test';
import assert from 'node:assert/strict';
import {createCoverageState,count,reasons,volumeStatus,sampleTarget,reportStatus} from './coverage.mjs';
test('coverage values stay exact and unknown counters stay unknown',()=>{
 assert.equal(count('18446744073709551615'),'18446744073709551615');assert.equal(count(null),'Ukendt');assert.equal(Object.keys(reasons).length,9);
 assert.match(reportStatus('cancelled'),/stoppet/);assert.match(reportStatus('unexpected'),/ukendt/);
 assert.match(volumeStatus({phase:'done',scan_status:'complete'}),/afsluttet/);assert.match(volumeStatus({phase:'done',scan_status:'partial'}),/delvis/);assert.match(volumeStatus({phase:'done'}),/ukendt/);
 for(const phase of ['pending','not_scanned'])assert.equal(volumeStatus({phase}),'Ikke gennemgået');assert.match(volumeStatus({phase:'weird'}),/Ukendt/);
 assert.deepEqual(sampleTarget({scan_id:'660-2'},{path_available:true,path_truncated:true,path_context:'saved_parent_and_raw_child',saved_entry_id:'1'}),{scanId:'660-2',entry:'1'});
 assert.equal(sampleTarget({scan_id:'660-2'},{path_available:false,saved_entry_id:'1'}),null);
});
test('bounded pages replace examples and preserve full counters and quality',async()=>{
 const s=createCoverageState();s.reset({scanId:'a'});const counters={access_denied:'150'};let args;
 await s.load(async a=>(args=a,{reason_counts:counters,quality:'event_counts',records:[1],next_cursor:'next',truncated:true,samples_truncated:true}));
 assert.equal(args.cursor,null);assert.equal(s.data.samples_truncated,true);assert.equal(s.data.truncated,true);
 await s.load(async a=>(args=a,{reason_counts:counters,quality:'event_counts',records:[2],samples_truncated:true,truncated:false}),true);
 assert.equal(args.cursor,'next');assert.deepEqual(s.data.records,[2]);assert.equal(s.data.reason_counts,counters);assert.equal(s.data.samples_truncated,true);assert.equal(s.cursor,null);
 s.reset({scanId:'legacy'});await s.load(async()=>({quality:'legacy_samples_only',reason_counts:null,error_count:'151',records:[]}));assert.equal(s.data.reason_counts,null);assert.equal(s.data.error_count,'151');
});
test('close/scan/reason reset discards stale success and error',async()=>{
 const s=createCoverageState();s.reset({scanId:'a'});let resolve,reject;
 const first=s.load(()=>new Promise(r=>resolve=r));s.reset({scanId:'b',reason:'io_error'});resolve({records:['stale']});assert.equal(await first,false);assert.equal(s.data,null);
 const second=s.load(()=>new Promise((_,r)=>reject=r));s.reset();reject(Error('stale error'));assert.equal(await second,false);assert.equal(s.error,null);assert.equal(s.busy,false);
 await s.load(async()=>{throw Error('current');});assert.equal(s.error,'Error: current');await s.load(async()=>({records:[]}));assert.equal(s.error,null);
});
