import test from 'node:test';
import assert from 'node:assert/strict';
import {createInsightsState,insightBytes,insightTarget,insightLabel,contextLabel} from './insights.mjs';
test('unknown sizes retain observed minima without inventing zero',()=>{
 assert.equal(insightBytes({bytes:null,observed_lower_bound_bytes:'2000'}),'mindst 2 KB');
 assert.equal(insightBytes({bytes:null}),'Ukendt');assert.equal(insightBytes({bytes:'0'}),'0 B');
 assert.match(contextLabel('context_unavailable'),/ældre måling/);
 assert.notEqual(insightLabel('xcode_archives'),insightLabel('cache_hint'));
 assert.notEqual(insightLabel('huggingface_models'),insightLabel('cache_hint'));
});
test('categories select contributions; candidates use clipped drilldown anchor',()=>{
 assert.deepEqual(insightTarget('categories',{category:'dependencies',entry_id:'1'}),{category:'dependencies'});
 assert.deepEqual(insightTarget('candidates',{entry_id:'1',candidate_entry_id:'1',drilldown_entry_id:'42'}),{entry:'42'});
});
test('loading and stale responses cannot overwrite newer selection',async()=>{
 const state=createInsightsState();let resolve;
 const old=state.load(()=>new Promise(r=>resolve=r),{entry:0,metric:'allocated'});
 assert.equal(state.busy,true);assert.equal(state.data,null);
 state.reset({scope:'current',category:'dependencies'});
 await state.load(async q=>({records:[q.entry],next_cursor:'new-page'}),{entry:42,metric:'logical'});
 resolve({records:['stale'],next_cursor:'old-page'});assert.equal(await old,false);
 assert.deepEqual(state.data.records,[42]);assert.equal(state.cursor,'new-page');assert.equal(state.busy,false);
});
test('scope/category/metric resets discard cursors and pagination replaces bounded page',async()=>{
 const state=createInsightsState();const queries=[];
 const fetch=async q=>{queries.push(q);return {records:queries.length===1?[1,2]:[3],next_cursor:queries.length===1?'opaque-short-page':null};};
 await state.load(fetch,{entry:0,metric:'allocated'});
 await state.load(fetch,{entry:0,metric:'allocated'},true);
 assert.equal(queries[1].cursor,'opaque-short-page');assert.deepEqual(state.data.records,[3]);assert.equal(state.cursor,null);
 state.reset({category:'build_output',scope:'current'});await state.load(fetch,{entry:1,metric:'logical'});
 assert.equal(queries[2].cursor,null);assert.equal(queries[2].category,'build_output');
});
test('errors leave retryable state and invalidated failures stay stale',async()=>{
 const state=createInsightsState();await state.load(async()=>{throw Error('query failed');},{});
 assert.match(state.error,/query failed/);assert.equal(state.busy,false);assert.equal(state.cursor,null);
 let reject;const pending=state.load(()=>new Promise((_,r)=>reject=r),{});state.reset();reject(Error('stale'));
 assert.equal(await pending,false);assert.equal(state.error,null);
});
