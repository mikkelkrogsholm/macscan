import test from 'node:test';
import assert from 'node:assert/strict';
import {sectors,weight,formatBytes,arcPath,fraction} from './sunburst.mjs';
test('ring angles preserve hierarchy and account for omitted children',()=>{
 const nodes=[{entry_id:'0',bytes:'100'},{entry_id:'1',parent_id:'0',bytes:'60'},{entry_id:'2',parent_id:'0',bytes:'20'},{entry_id:'3',parent_id:'1',bytes:'30'}];
 const a=sectors(nodes,'0');assert.equal(a.filter(x=>x.depth===1).length,3);
 assert.equal(a.find(x=>x.aggregate&&x.depth===1).node.bytes,'20');
 const parent=a.find(x=>x.node.entry_id==='1'&&!x.aggregate),child=a.find(x=>x.node.entry_id==='3');
 assert.ok(child.start>=parent.start-0.01&&child.end<=parent.end+0.01);assert.ok(a.every(x=>Number.isFinite(x.start)&&x.end>=x.start));
});
test('unknown values use observed minimum, never invent complete totals',()=>{
 assert.equal(weight({bytes:null,observed_bytes:'40'}),40n);assert.equal(formatBytes(null),'Ukendt');
 assert.equal(sectors([{entry_id:'0',bytes:null,observed_bytes:'0'}],'0').length,0);
});
test('u64 geometry is bounded and full-circle paths use two arcs',()=>{
 assert.equal(fraction(18446744073709551615n,18446744073709551615n),1);
 const p=arcPath(100,150,0,2*Math.PI);assert.equal((p.match(/ A /g)||[]).length,4);assert.ok(!p.includes('NaN'));
});
test('byte formatting has Danish decimal units and signed growth',()=>{
 assert.equal(formatBytes('1500000000'),'1,5 GB');assert.equal(formatBytes('-2000'),'-2 KB');
});
test('colors remain stable when moving into an existing branch',()=>{
 const colors=new Map(),nodes=[{entry_id:'0',bytes:'100'},{entry_id:'1',parent_id:'0',bytes:'100'},{entry_id:'2',parent_id:'1',bytes:'100'}];
 sectors(nodes,'0',colors);const hue=colors.get('2');sectors(nodes.slice(1),'1',colors);assert.equal(colors.get('2'),hue);
});
