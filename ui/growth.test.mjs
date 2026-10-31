import test from 'node:test';
import assert from 'node:assert/strict';
import {createGrowthState,exactBytes,signedDelta,knownBytes,growthTarget,elapsedLabel,growthCounts} from './growth.mjs';
test('signed exact deltas retain integers beyond Number, null is never a decrease',()=>{
 assert.equal(signedDelta('18446744073709551616'),'+18.446.744.073.709.551.616 B');
 assert.equal(signedDelta('-3'),'-3 B');assert.equal(signedDelta('0'),'0 B');assert.equal(signedDelta(null),'Ukendt ændring');
 assert.equal(knownBytes(null,'9007199254740993'),'Ukendt · mindst observeret 9.007.199.254.740.993 B');
 assert.equal(knownBytes('0','99'),'0 B');assert.equal(knownBytes(null,null),'Ukendt');assert.equal(exactBytes(null),'Ukendt');
 assert.match(growthCounts({new_count:'2',removed_count:'3',changed_count:'4',unknown_count:'5'}),/Nye 2 · fjernede 3 · ændrede 4 · ukendte 5/);
});
test('interval uses saved exact nanoseconds, including subsecond remainder',()=>{
 assert.equal(elapsedLabel('86400000000000'),'1 døgn (86.400.000.000.000 ns)');
 assert.match(elapsedLabel('1000000001'),/^1,000000001 sek/);assert.match(elapsedLabel(null),/^Ukendt/);
});
test('removed files open the returned BEFORE scan and id, never after id',()=>{
 assert.deepEqual(growthTarget({before_entry_id:'4',after_entry_id:null,drilldown_scan_id:'before',drilldown_entry_id:'4'}),{scanId:'before',entry:'4'});
 assert.deepEqual(growthTarget({before_entry_id:'4',after_entry_id:'9',drilldown_scan_id:'after',drilldown_entry_id:'9'}),{scanId:'after',entry:'9'});
 assert.equal(growthTarget({before_entry_id:'4'}),null);
});
test('stale success and stale failure cannot replace a new scope or pair',async()=>{
 const state=createGrowthState();let resolve,reject;
 const old=state.load(()=>new Promise(r=>resolve=r),{before:'a',after:'b',entry:0,metric:'allocated'});
 assert.equal(state.busy,true);state.reset({scope:'current'});
 const failed=state.load(()=>new Promise((_,r)=>reject=r),{before:'a',after:'b',entry:3,metric:'logical'});
 state.reset();await state.load(async()=>({records:['current'],next_cursor:'now'}),{before:'b',after:'c',entry:3,metric:'logical'});
 resolve({records:['stale']});reject(Error('old error'));
 assert.equal(await old,false);assert.equal(await failed,false);assert.deepEqual(state.data.records,['current']);assert.equal(state.cursor,'now');assert.equal(state.error,null);
});
test('paging replaces rows and request changes invalidate cursors even without reset',async()=>{
 for(const patch of [{metric:'logical'},{entry:3},{before:'older'},{after:'newer'}]){
  const state=createGrowthState(),queries=[];const base={before:'a',after:'b',entry:0,metric:'allocated'};
  const fetch=async q=>{queries.push(q);return {records:[queries.length],next_cursor:'opaque'};};
  await state.load(fetch,base);await state.load(fetch,base,true);assert.equal(queries[1].cursor,'opaque');assert.deepEqual(state.data.records,[2]);
  await state.load(fetch,{...base,...patch},true);assert.equal(queries[2].cursor,null);
 }
});
test('scope/filter/grouping/drilldown resets discard pages and preserve selected scope',async()=>{
 const state=createGrowthState(),queries=[];const fetch=async q=>{queries.push(q);return {records:[],next_cursor:'page'};};
 for(const patch of [{scope:'current'},{change:'removed'},{grouping:'category'},{files:true,group:'dependencies'},{files:false,group:null}]){
  state.reset(patch);assert.equal(state.cursor,null);await state.load(fetch,{entry:3,metric:'logical'},true);assert.equal(queries.at(-1).cursor,null);
 }
 assert.equal(state.scope,'current');assert.equal(queries.at(-1).change,'removed');
});
test('error clears obsolete page and remains retryable',async()=>{
 const state=createGrowthState();await state.load(async()=>({records:[1],next_cursor:'page'}),{});
 await state.load(async()=>{throw Error('resource budget');},{},true);assert.match(state.error,/resource/);assert.equal(state.cursor,null);assert.equal(state.data,null);assert.equal(state.busy,false);
 await state.load(async()=>({records:[2]}),{});assert.equal(state.error,null);assert.deepEqual(state.data.records,[2]);
});
