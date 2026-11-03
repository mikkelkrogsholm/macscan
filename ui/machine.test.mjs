import {test} from 'node:test';import assert from 'node:assert/strict';
import {machineView,phaseLabel} from './machine.mjs';
test('whole disk overview uses OS capacity, never sums file-scan totals',()=>{
 const s={used_bytes:'1000',volumes:[{name:'Data',os_used_bytes:'600',scan_id:'saved',totals:{accounted_allocated_bytes:'999999'},phase:'done',scan_status:'partial'},{name:'Recovery',os_used_bytes:'200',phase:'unmounted'}]};
 const v=machineView(s);assert.equal(v.selected.bytes,'1000');assert.deepEqual(v.nodes.slice(1).map(n=>n.bytes),['600','200','200']);assert.equal(v.children.records[0].scan_id,'saved');assert.equal(v.children.records[1].scan_id,undefined);assert.equal(phaseLabel(s.volumes[1]),'Ikke monteret');assert.equal(s.volumes.length,2);
});
test('overlapping OS allocation is explicit and exact u64 capacities survive',()=>{
 const v=machineView({used_bytes:'1',volumes:[{os_used_bytes:'18446744073709551615'}]});
 assert.equal(v.accountingOverlap,true);assert.equal(v.selected.bytes,'18446744073709551615');assert.equal(v.nodes.length,2);
});

test('unknown OS capacities remain unknown without residual or overlap claims',()=>{
 for(const used_bytes of [null,undefined]){
  const v=machineView({used_bytes,volumes:[{name:'Known',os_used_bytes:'500'},{name:'Unknown',os_used_bytes:null},{name:'Missing'}]});
  assert.equal(v.selected.bytes,null);assert.equal(v.selected.observed_bytes,null);assert.equal(v.accountingOverlap,null);
  assert.deepEqual(v.children.records.map(r=>r.bytes),['500',null,null]);assert.equal(v.nodes.length,4);
 }
 const known=machineView({used_bytes:'1000',volumes:[{os_used_bytes:'500'},{os_used_bytes:null}]});
 assert.equal(known.selected.bytes,'1000');assert.equal(known.selected.observed_bytes,'1000');assert.equal(known.accountingOverlap,null);assert.equal(known.nodes.length,3);
});
