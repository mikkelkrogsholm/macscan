import assert from 'node:assert/strict';
import test from 'node:test';
import {APFS,apfsSupported,apfsComparable,apfsUnavailable,metricLabel,apfsContribution,apfsZeroNonowner} from './metrics.mjs';
import {sameRootBaseline} from './focused-rescan.mjs';
const metadata={algorithm_version:'apfs-data-clone-1',capture_version:1,status:'limited',comparison_ready:true};
const context={clone_capture:{requested:true,version:1},os_identity:'os',volume_before:{volume_uuid:'v'},volume_after:{volume_uuid:'v'},started_unix_ns:'2'};
const view={status:'complete',apfs_data_clone:metadata,measurement_context:context,scope:{root_path_bytes_hex:'FF',policy_fingerprint:'p'}};
const scan={status:'complete',scan_id:'older',apfs_data_clone:metadata,context:{...context,started_unix_ns:'1'},root_path_bytes_hex:'FF',policy:'p'};
test('APFS availability requires both supported versions, explicit capture and available saved columns',()=>{
 assert.equal(apfsSupported(view),true);assert.match(metricLabel(APFS),/Begrænset DATA/);
 for(const patch of [{apfs_data_clone:null},{apfs_data_clone:{...metadata,algorithm_version:'future'}},{apfs_data_clone:{...metadata,capture_version:2}},{apfs_data_clone:{...metadata,columns_available:false}},{measurement_context:{clone_capture:{requested:false,version:1}}},{measurement_context:{clone_capture:{requested:true,version:2}}}])assert.equal(apfsSupported({...view,...patch}),false);
 assert.equal(apfsSupported({...view,apfs_data_clone:{...metadata,status:'fallback',comparison_ready:false}}),true);
 assert.equal(apfsUnavailable('apfs-data-clone unavailable: rescan'),true);assert.equal(apfsUnavailable('missing source'),false);
});
test('APFS baseline gates both measurements; primary baseline behavior remains unchanged',()=>{
 assert.equal(sameRootBaseline(scan,view,'current',APFS),true);
 for(const bad of [{status:'partial'},{apfs_data_clone:{...metadata,comparison_ready:false}},{apfs_data_clone:{...metadata,algorithm_version:'future'}},{apfs_data_clone:{...metadata,capture_version:2}},{apfs_data_clone:null}]){
  assert.equal(sameRootBaseline({...scan,...bad},view,'current',APFS),false);
  assert.equal(sameRootBaseline(scan,{...view,...bad},'current',APFS),false);
  assert.equal(sameRootBaseline({...scan,...bad},view,'current','allocated'),true);
 }
 assert.equal(apfsComparable(view),true);
 for(const bad of [{policy:'other'},{root_path_bytes_hex:'FE'},{context:{...scan.context,os_identity:'other'}},{context:{...scan.context,volume_after:{volume_uuid:'other'}}}])assert.equal(sameRootBaseline({...scan,...bad},view,'current',APFS),false);
});

test('zero attribution explanation applies only to file nonowners, never zero-own directories',()=>{
 const file={kind:'file',own_apfs_data_clone_bytes:'0',apfs_data_clone_reason:'full_data_nonowner'};
 assert.equal(apfsZeroNonowner(file),true);assert.match(apfsContribution(file),/global ejerpost/);
 assert.equal(apfsZeroNonowner({...file,kind:'directory',bytes:'2170880',apfs_data_clone_reason:'nonfile_retained'}),false);
 assert.equal(apfsZeroNonowner({...file,apfs_data_clone_reason:'limited_retained'}),false);
 assert.equal(apfsZeroNonowner({...file,apfs_data_clone_reason:'hardlink_nonowner'}),true);
});
