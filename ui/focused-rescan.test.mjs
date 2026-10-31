import assert from 'node:assert/strict';
import test from 'node:test';
import {rescanLabel,sameRootBaseline} from './focused-rescan.mjs';
test('header identifies selected root, nested map and file',()=>{
 assert.equal(rescanLabel({entry_id:'0',kind:'directory'}),'Genmål hele målingen');
 assert.equal(rescanLabel({entry_id:'5',kind:'directory'}),'Genmål denne mappe');
 assert.equal(rescanLabel({entry_id:'0',kind:'file'}),'Genmål denne fil');
});
test('only exact raw-root policy OS volume and chronological baselines are offered; origin is irrelevant',()=>{
 const context={os_identity:'os',volume_before:{volume_uuid:'v'},volume_after:{volume_uuid:'v'},started_unix_ns:'2'};
 const view={scope:{root_path_bytes_hex:'FF',policy_fingerprint:'p'},measurement_context:context};
 const candidate={scan_id:'older',root_path_bytes_hex:'FF',policy:'p',context:{...context,started_unix_ns:'1'},origin:{source_scan_id:'broader'}};
 assert.equal(sameRootBaseline(candidate,view,'new'),true);
 for(const changed of [{root_path_bytes_hex:'FE'},{policy:'other'},{context:{...context,os_identity:'other'}},{context:{...context,volume_after:{volume_uuid:'other'}}},{context:null},{context:{...candidate.context,started_unix_ns:undefined}},{context:{...candidate.context,started_unix_ns:'invalid'}},{kind:'machine'},{scan_id:'new'}])assert.equal(sameRootBaseline({...candidate,...changed},view,'new'),false);
});
