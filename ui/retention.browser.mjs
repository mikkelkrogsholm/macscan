// Saved synthetic whole-Mac sessions only, no native scanning or OS actions.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE??'playwright');
const root=new URL('./',import.meta.url),out=new URL('../evidence/issue-17/',import.meta.url);await mkdir(out,{recursive:true});
const server=createServer(async(req,res)=>{try{const name=new URL(req.url,'http://localhost').pathname.slice(1)||'index.html';if(!/^[\w.-]+$/.test(name))throw Error();res.setHeader('Content-Type',name.endsWith('.css')?'text/css':/\.(js|mjs)$/.test(name)?'text/javascript':'text/html');res.end(await readFile(new URL(name,root)));}catch{res.writeHead(404);res.end();}});
await new Promise(r=>server.listen(0,'127.0.0.1',r));let browser;
try{
 browser=await chromium.launch({headless:true,channel:'chrome'});const page=await browser.newPage({viewport:{width:1280,height:900}});page.setDefaultTimeout(12000);const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const context=n=>({os_identity:'synthetic',volume_before:{volume_uuid:'owned'},volume_after:{volume_uuid:'owned'},started_unix_ns:String(n)});
  const volume=(id,n)=>({scan_id:id,grouped_volume:true,root:'/synthetic/data',root_path_bytes_hex:'44415441',policy:'p',context:context(n),started_at:`2026-10-31T12:0${n}:00Z`,status:'complete'});
  const scans=[{kind:'machine',scan_id:'20-1',root:'Hele Mac’en',started_at:'2026-10-31T12:02:00Z',status:'complete'},volume('19-1',2),{kind:'machine',scan_id:'10-1',root:'Hele Mac’en',started_at:'2026-10-31T12:01:00Z',status:'complete'},volume('9-1',1)];window.calls=[];
  window.__TAURI__={menu:{Menu:{}},core:{invoke:async(name,args={})=>{window.calls.push({name,args});
   if(name==='bootstrap')return {scans,volumes:{records:[]},store:'/mock-history',explicit_store:true};
   if(name==='machine_session')return {session_id:args.sessionId,started_at:'2026-10-31T12:02:00Z',status:'complete',entry_count:2,used_bytes:'100',free_bytes:'100',coverage_note:'Saved synthetic group',volumes:[{name:'Data',root:'/synthetic/data',phase:'done',scan_id:args.sessionId==='20-1'?'19-1':'9-1',scan_status:'complete',os_used_bytes:'100',totals:{allocated_bytes:'100'}}]};
   if(name==='view_scan'){const scan=scans.find(s=>s.scan_id===args.scanId),selected={entry_id:'0',parent_id:null,name_display:'Data',path_display:'/synthetic/data',kind:'directory',bytes:'100',coverage_complete:true},child={...selected,entry_id:'1',parent_id:'0',name_display:'owned',kind:'file',bytes:'100'};return {scan_id:scan.scan_id,selected,ancestors:[selected],nodes:[selected,child],children:{records:[child],next_cursor:null},scope:{root_display:scan.root,root_path_bytes_hex:scan.root_path_bytes_hex,policy_fingerprint:'p'},measurement_context:scan.context,started_at:scan.started_at,status:'complete',entry_count:'2',child_count:'1'};}
   if(name==='growth')return {records:[],scope:{after:{path_display:'/synthetic/data'}},totals:{before:{},after:{}},contributions:{},interval:{},metric:'allocated',coverage:{complete:true}};
   throw Error('Unexpected '+name);
  }}};
 });
 await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>document.querySelector('#view-title').textContent==='Hele Mac’en');
 assert.equal(await page.locator('#history-list .history-item').count(),2);assert.match(await page.locator('#history-retention').innerText(),/Seneste 3 scanninger/);
 await page.locator('#rows .file-row').click();await page.waitForFunction(()=>document.querySelector('#before-scan option[value="9-1"]'));
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId),'19-1');assert.equal(await page.locator('#history-list .history-item').count(),2);
 await page.click('#changes-tab');await page.waitForFunction(()=>window.calls.some(c=>c.name==='growth'));assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='growth').at(-1).args.before),'9-1');
 await page.screenshot({path:new URL('grouped-volume-baseline.png',out).pathname});assert.deepEqual(errors,[]);
 console.log('PASS grouped child rows hidden; saved metadata remains available; retained whole-Mac same-volume baseline offered and used by growth; three-run note visible');
}finally{await browser?.close();await new Promise(r=>server.close(r));}
