import test from 'node:test';
import assert from 'node:assert/strict';
import {createProfileState,integerFilter,profileBytes,savedTime,referenceTime} from './file-profile.mjs';
test('exact integer size filters and totals retain values beyond Number precision',()=>{
 assert.equal(integerFilter('9007199254740993'),'9007199254740993');assert.equal(integerFilter('100',1048576n),'104857600');assert.equal(integerFilter(''),null);
 assert.equal(integerFilter('18446744073709551615'),'18446744073709551615');
 for(const invalid of ['-1','1.5',' 1','1e3','18446744073709551616'])assert.throws(()=>integerFilter(invalid));
 assert.match(profileBytes({bytes:'1844674407370955161500'}),/1\.844\.674\.407\.370\.955\.161\.500 B/);
 assert.match(profileBytes({bytes:null,observed_lower_bound_bytes:'9007199254740993'}),/Ukendt · mindst observeret 9\.007\.199\.254\.740\.993/);
});
test('saved time zero, negative, future, unknown and range fallback are distinct',()=>{
 assert.match(savedTime('0',false),/^1970-01-01/);assert.match(savedTime('-1',false),/^1969-12-31/);
 assert.match(savedTime('1',true),/fremtidig/);assert.match(savedTime(null,false),/Ukendt ændringstid/);
 assert.match(savedTime('1',null),/referencetid ukendt/);assert.match(savedTime('9223372036854775807',false),/9223372036854775807 Unix/);
 assert.match(referenceTime('9007199254740993123'),/9007199254740993123 ns/);
});
test('bounded page replacement, exact controls, opaque type key and reset invalidation',async()=>{
 const s=createProfileState();s.reset({active:true,age:'180',size:'100',unit:'1048576',typeKey:'ext:FF'});const calls=[];
 const fetch=async q=>{calls.push(q);return {records:[q.cursor?'second':'first'],next_cursor:q.cursor?null:'opaque'};};
 await s.load(fetch,{entry:60,metric:'logical'});assert.deepEqual(s.data.records,['first']);
 assert.equal(calls[0].minBytes,'104857600');assert.equal(calls[0].minAgeDays,'180');assert.equal(calls[0].typeKey,'ext:FF');
 await s.load(fetch,{entry:60,metric:'logical'},true);assert.deepEqual(s.data.records,['second']);assert.equal(calls[1].cursor,'opaque');
 s.reset({scope:'all',types:true,sortCount:true});assert.equal(s.cursor,null);assert.equal(s.data,null);assert.equal(s.error,null);
 await s.load(fetch,{entry:0,metric:'allocated'});assert.equal(calls[2].cursor,null);assert.equal(calls[2].sortCount,true);
});
test('stale success and failure cannot restore state after filter/navigation/tab reset',async()=>{
 for(const fail of [false,true]){const s=createProfileState();let finish;const pending=s.load(()=>new Promise((resolve,reject)=>finish=()=>fail?reject(Error('old failure')):resolve({records:['old'],next_cursor:'old'})),{});
  s.reset({age:'90'});finish();assert.equal(await pending,false);assert.equal(s.data,null);assert.equal(s.cursor,null);assert.equal(s.error,null);assert.equal(s.busy,false);
 }
});
test('validation errors recover without querying and loading clears previous page/error',async()=>{
 const s=createProfileState();s.age='invalid';let count=0;await s.load(async()=>count++,{});assert.equal(count,0);assert.match(s.error,/heltal/);
 s.reset({age:''});await s.load(async()=>({records:[],next_cursor:null}),{});assert.deepEqual(s.data.records,[]);assert.equal(s.error,null);
});
