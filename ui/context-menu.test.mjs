import test from 'node:test';
import assert from 'node:assert/strict';
import {createEntryMenu} from './context-menu.mjs';

test('native menu has three actions and reuses one menu for exact clicked targets',async()=>{
 let items,builds=0,position;
 const actions=[],errors=[];
 const Menu={new:async options=>{builds++;items=options.items;return {popup:async p=>{position=p;}};}};
 const show=createEntryMenu(Menu,(action,target)=>actions.push({action,...target}),e=>errors.push(e));
 await show({scanId:'first-scan',entry:'7'},{x:12,y:34});
 assert.deepEqual(position,{x:12,y:34});
 assert.deepEqual(items.map(i=>[i.id,i.text]),[
  ['reveal','Vis i Finder'],['copy','Kopiér filsti'],['copy_info','Kopiér information']
 ]);
 for(const item of items)await item.action();
 assert.deepEqual(actions.map(a=>[a.action,a.scanId,a.entry]),[
  ['reveal','first-scan',7],['copy','first-scan',7],['copy_info','first-scan',7]
 ]);
 await show({scanId:'second-scan',entry:'11'},{x:50,y:60});
 await items[1].action();
 assert.deepEqual(actions.at(-1),{action:'copy',scanId:'second-scan',entry:11});
 assert.equal(builds,1);
 assert.deepEqual(errors,[]);
});

test('an open menu cannot be retargeted by a second request',async()=>{
 let items,dismiss;
 const actions=[];
 const show=createEntryMenu({new:async options=>{
  items=options.items;
  return {popup:()=>new Promise(resolve=>{dismiss=resolve;})};
 }},(action,target)=>actions.push(target),assert.fail);
 const first=show({scanId:'a',entry:'3'});
 await new Promise(resolve=>setImmediate(resolve));
 await show({scanId:'b',entry:'99'});
 await items[2].action();
 assert.deepEqual(actions,[{scanId:'a',entry:3}]);
 dismiss();await first;
});

test('menu creation can recover and action failures are reported',async()=>{
 let attempts=0,items;
 const errors=[];
 const show=createEntryMenu({new:async options=>{
  if(++attempts===1)throw new Error('menu unavailable');
  items=options.items;return {popup:async()=>{}};
 }},async()=>{throw new Error('file moved');},e=>errors.push(e.message));
 await show({scanId:'a',entry:0});
 await show({scanId:'a',entry:1});
 await items[0].action();
 assert.deepEqual(errors,['menu unavailable','file moved']);
 assert.equal(attempts,2);
});
