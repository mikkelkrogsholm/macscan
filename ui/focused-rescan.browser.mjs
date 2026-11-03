// Synthetic native-command boundary only; no real scans or OS actions.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE??'playwright');
const root=new URL('./',import.meta.url),out=new URL('../evidence/issue-13/',import.meta.url);await mkdir(out,{recursive:true});
const server=createServer(async(req,res)=>{try{const name=new URL(req.url,'http://localhost').pathname.slice(1)||'index.html';if(!/^[\w.-]+$/.test(name))throw Error();res.setHeader('Content-Type',name.endsWith('.css')?'text/css':/\.(js|mjs)$/.test(name)?'text/javascript':'text/html');res.end(await readFile(new URL(name,root)));}catch{res.writeHead(404);res.end();}});
await new Promise(r=>server.listen(0,'127.0.0.1',r));let browser;
try{
 browser=await chromium.launch({headless:true,channel:'chrome'});const page=await browser.newPage({viewport:{width:1280,height:900}});page.setDefaultTimeout(12000);const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const context=n=>({os_identity:'synthetic',volume_before:{volume_uuid:'v'},volume_after:{volume_uuid:'v'},started_unix_ns:String(n)});
  const scans=[{scan_id:'130-1',root:'/synthetic',policy:'p',root_path_bytes_hex:'2F73796E746865746963',context:context(1),started_at:'2026-10-31T12:00:00Z',status:'complete'},{scan_id:'130-2',root:'/synthetic',policy:'p',root_path_bytes_hex:'2F73796E746865746963',context:context(2),started_at:'2026-10-31T12:00:00Z',status:'complete'}];
  if(localStorage.getItem('mock-focused-scans'))scans.splice(0,scans.length,...JSON.parse(localStorage.getItem('mock-focused-scans')));
  window.calls=[];window.pending=false;window.holdPrepare=false;window.holdView=false;
  window.__TAURI__={menu:{Menu:{}},core:{invoke:async(name,args={})=>{
   window.calls.push({name,args});
   if(name==='bootstrap')return {scans,volumes:{records:[]},store:'/mock-history',explicit_store:true};
   if(name==='view_scan'){
    if(window.holdView)await new Promise(r=>window.finishView=r);
    const scan=scans.find(s=>s.scan_id===args.scanId);if(!scan||window.missingSourceId===args.scanId)throw Error('Oprindelig måling mangler');
    const base=scan.root;const selected={entry_id:String(args.entry),parent_id:args.entry?'0':null,name_display:args.entry===9?'ordinary.txt':args.entry?'child':'',path_display:base+(args.entry?'/child':''),kind:args.entry===9||scan.file?'file':'directory',bytes:'20',coverage_complete:true};
    const child={entry_id:'3',parent_id:String(args.entry),name_display:'ordinary-child',path_display:base+'/child',kind:'directory',bytes:'7',coverage_complete:true};
    return {scan_id:args.scanId,selected,ancestors:[{entry_id:'0',name_display:'synthetic'},{entry_id:'3',name_display:'child'}].slice(0,args.entry?2:1),nodes:[selected,child],children:{records:[child],next_cursor:null},scope:{root_display:base,root_path_bytes_hex:scan.root_path_bytes_hex,policy_fingerprint:'p'},measurement_context:scan.context,origin:scan.origin,started_at:scan.started_at,status:'complete',entry_count:'3',child_count:'1'};
   }
   if(name==='prepare_rescan'){
    const scan=scans.find(s=>s.scan_id===args.scanId);if(!scan||window.missingSourceId===args.scanId)throw Error('Oprindelig måling mangler');
    if(window.holdPrepare)await new Promise(r=>window.finishPrepare=r);
    return {source_scan_id:args.scanId,source_entry_id:String(args.entry),source_started_at:'2026-10-31T12:00:00Z',root_display:scan.root+(args.entry===9?'/ordinary.txt':args.entry?'/child':''),kind:args.entry===9?'file':'directory',root_path_bytes_hex:'raw-only'};
   }
   if(name==='start_rescan'){
    window.pending=true;const id='130-'+(scans.length+1);window.newId=id;const source=scans.find(s=>s.scan_id===args.scanId);
    const file=args.entry===9||source.file,root=args.entry===9?'/synthetic/ordinary.txt':args.entry?source.root+'/child':source.root;
    scans.unshift({scan_id:id,root,root_path_bytes_hex:args.entry===0?source.root_path_bytes_hex:file?'46494C45':'4348494C44',file,policy:'p',context:context(scans.length+1),started_at:'2026-10-31T12:00:00Z',status:'complete',origin:{source_scan_id:args.scanId,source_entry_id:String(args.entry),source_started_at:'2026-10-31T12:00:00Z'}});localStorage.setItem('mock-focused-scans',JSON.stringify(scans));return {started:true};
   }
   if(name==='job_status')return window.pending?{phase:'scanning',entries_seen:2,root:'/synthetic/child',branches:[]}:{phase:'done',scan_id:window.newId,whole_mac:false,retention_warning:window.retentionWarning};
   if(name==='stop_scan'){window.pending=false;return null;}
   if(name==='list_children')return {records:[],next_cursor:null};
   if(name==='insights')return {scan_id:args.scanId,mode:args.category?'candidates':'categories',metric:'allocated',started_at:'2026-10-31T12:00:00Z',scope:{path_display:'/synthetic/child'},coverage:{complete:true},totals:{bytes:'7',file_path_count:'1'},records:[{category:'build_output',entry_id:'1',drilldown_entry_id:'3',path_display:'/synthetic/evidence-above-scope',bytes:'7',file_path_count:'1',explanation:'Synthetic candidate',rule_id:'rust_target',rule_version:'insights-1',context_coverage:'complete'}]};
   if(name==='growth')return {records:[],scope:{after:{path_display:'/synthetic'}},totals:{before:{},after:{}},contributions:{},interval:{},metric:'allocated',coverage:{complete:true}};
   throw Error('Unexpected '+name);
  }}};
 });
 const dialog=page.locator('#rescan-dialog');const calls=()=>page.evaluate(()=>window.calls.filter(c=>c.name==='start_rescan'));
 const cancel=async()=>{await dialog.getByRole('button',{name:'Annuller',exact:true}).click();};
 const open=async()=>{await page.click('#rescan');await dialog.getByRole('button',{name:'Start genmåling',exact:true}).waitFor();await page.waitForFunction(()=>!document.querySelector('#rescan-dialog .primary').disabled);};
 await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>document.querySelector('#rescan').textContent==='Genmål hele målingen');
 assert.match(await page.locator('#history-retention').innerText(),/Seneste 3 scanninger.*tæller én gang/);
 // Same display-second older measurement remains an eligible baseline via exact ns.
 assert.equal(await page.locator('#before-scan option[value="130-1"]').count(),0); // currently oldest
 await page.locator('#history-list .history-item').nth(1).click();await page.waitForFunction(()=>document.querySelector('#before-scan option[value="130-1"]'));
 await open();assert.match(await dialog.innerText(),/\/synthetic/);
 // Native-dialog contrast regression: every text role uses a readable dark palette.
 const colors=await dialog.evaluate(d=>{
  const rgb=value=>value.match(/[\d.]+/g).slice(0,3).map(Number);
  const luminance=value=>rgb(value).map(n=>{n/=255;return n<=.04045?n/12.92:((n+.055)/1.055)**2.4}).reduce((sum,n,i)=>sum+n*[.2126,.7152,.0722][i],0);
  const contrast=(a,b)=>{const x=luminance(a),y=luminance(b);return (Math.max(x,y)+.05)/(Math.min(x,y)+.05)};
  const background=getComputedStyle(d).backgroundColor;
  return {background,ratios:[d.querySelector('h2'),d.querySelector('p'),...d.querySelectorAll('button')].map(n=>{
   const s=getComputedStyle(n),own=s.backgroundColor;return contrast(s.color,own==='rgba(0, 0, 0, 0)'?background:own);
  })};
 });
 assert.equal(colors.background,'rgb(32, 38, 52)');for(const ratio of colors.ratios)assert.ok(ratio>=4.5,`dialog text contrast ${ratio}`);
 await dialog.getByRole('button',{name:'Annuller',exact:true}).focus();await page.keyboard.press('Tab');
 const focus=await dialog.locator(':focus-visible').evaluate(n=>{const s=getComputedStyle(n);return {width:s.outlineWidth,color:s.outlineColor};});assert.equal(focus.width,'2px');assert.equal(focus.color,'rgb(199, 189, 255)');
 await page.screenshot({path:new URL('focused-dialog-dark.png',out).pathname});
 await page.keyboard.press('Escape');assert.equal((await calls()).length,0);
 await page.locator('#rows .file-row').click();await page.waitForFunction(()=>document.querySelector('#rescan').textContent==='Genmål denne mappe');await open();assert.match(await dialog.innerText(),/\/synthetic\/child/);await cancel();assert.equal((await calls()).length,0);
 // Delayed accepted-view navigation keeps header source pair on the old display.
 await page.evaluate(()=>{window.holdView=true;document.querySelectorAll('#history-list .history-item')[0].click();});await page.waitForFunction(()=>typeof window.finishView==='function');await open();assert.deepEqual(await page.evaluate(()=>window.calls.filter(c=>c.name==='prepare_rescan').at(-1).args),{scanId:'130-2',entry:3});await cancel();await page.evaluate(()=>{window.holdView=false;window.finishView();delete window.finishView;});await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 // Closed preparation cannot re-enable a stale target or start a scan.
 await page.evaluate(()=>window.holdPrepare=true);await page.click('#rescan');await page.waitForFunction(()=>typeof window.finishPrepare==='function');await page.keyboard.press('Escape');await page.evaluate(()=>{window.holdPrepare=false;window.finishPrepare();delete window.finishPrepare;});assert.equal(await dialog.isVisible(),false);assert.equal((await calls()).length,0);
 await page.click('#insights-tab');await page.getByRole('button',{name:'Rust-buildoutput',exact:true}).click();await page.getByRole('button',{name:'Genmål dette område…',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('#rescan-dialog .primary').disabled);assert.deepEqual(await page.evaluate(()=>window.calls.filter(c=>c.name==='prepare_rescan').at(-1).args),{scanId:'130-1',entry:3});await cancel();
 // An old insight result stays paired with its source during pending navigation.
 await page.evaluate(()=>{window.holdView=true;document.querySelectorAll('#history-list .history-item')[1].click();});await page.waitForFunction(()=>typeof window.finishView==='function');await page.getByRole('button',{name:'Genmål dette område…',exact:true}).evaluate(b=>b.click());await page.waitForFunction(()=>!document.querySelector('#rescan-dialog .primary').disabled);assert.deepEqual(await page.evaluate(()=>window.calls.filter(c=>c.name==='prepare_rescan').at(-1).args),{scanId:'130-1',entry:3});await cancel();await page.evaluate(()=>{window.holdView=false;window.finishView();delete window.finishView;});await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 await page.click('#insights-tab');await page.getByRole('button',{name:'Rust-buildoutput',exact:true}).click();await page.getByRole('button',{name:'Genmål dette område…',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('#rescan-dialog .primary').disabled);await dialog.getByRole('button',{name:'Start genmåling',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#rescan').disabled);assert.equal((await calls()).length,1);assert.equal((await calls())[0].args.captureClones,false);assert.equal(await page.getByRole('button',{name:'Genmål dette område…',exact:true}).isDisabled(),true);assert.equal(await page.locator('#stop').isEnabled(),true);
 await page.evaluate(()=>{window.retentionWarning='Målingen er gemt, men historikken kunne ikke ryddes: synthetic';window.pending=false;});await page.waitForFunction(()=>document.querySelector('#source-origin'));await page.waitForFunction(()=>document.querySelector('#toast').textContent.includes('historikken kunne ikke ryddes'));assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId),await page.evaluate(()=>window.newId));assert.match(await page.locator('#source-origin').innerText(),/Selvstændig genmåling/);assert.match(await page.locator('#scan-stamp').innerText(),/ikke live/);await page.click('#changes-tab');assert.match(await page.locator('#growth-panel').innerText(),/bredere rod/);await page.screenshot({path:new URL('focused-standalone.png',out).pathname});
 await page.getByRole('button',{name:'Åbn oprindelig måling/post',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#rescan').textContent==='Genmål denne mappe');assert.equal(await page.locator('#source-origin').count(),0);assert.equal(await page.locator('#files-tab').getAttribute('aria-pressed'),'true');
 // Missing source leaves the accepted new result coherent for metric and rescan actions.
 await page.reload();await page.waitForSelector('#source-origin');const focusedId=await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId);
 await page.evaluate(()=>window.missingSourceId='130-2');await page.getByRole('button',{name:'Åbn oprindelig måling/post',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#source-origin').textContent.includes('ikke tilgængelig'));
 await page.click('#logical');await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId),focusedId);await open();assert.deepEqual(await page.evaluate(()=>window.calls.filter(c=>c.name==='prepare_rescan').at(-1).args),{scanId:focusedId,entry:0});await cancel();await page.evaluate(()=>window.missingSourceId=null);
 // Source link survives app reload from bootstrap data.
 await page.reload();await page.waitForSelector('#source-origin');await page.getByRole('button',{name:'Åbn oprindelig måling/post',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 // Simulate file root from saved metadata, then ensure explicit confirmation and new/old separation.
 await page.evaluate(async()=>{await window.__TAURI__.core.invoke('start_rescan',{scanId:'130-1',entry:9});window.pending=false;});await page.reload();await page.waitForFunction(()=>document.querySelector('#rescan').textContent==='Genmål denne fil');await open();assert.match(await dialog.innerText(),/\/synthetic/);await cancel();assert.equal(await page.locator('#files-tab').getAttribute('aria-pressed'),'true');
 const fileSourceId=await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId);await open();await dialog.getByRole('button',{name:'Start genmåling',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#rescan').disabled);await page.evaluate(()=>window.pending=false);await page.waitForFunction(()=>!document.querySelector('#rescan').disabled&&document.querySelector('#source-origin'));
 assert.equal(await page.locator('#rescan').textContent(),'Genmål denne fil');assert.notEqual(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId),fileSourceId);await page.getByRole('button',{name:'Åbn oprindelig måling/post',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').at(-1).args.scanId),fileSourceId);
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error').innerText());
 console.log('PASS focused header root/map/file, explicit confirm/cancel/Escape, stale preparation, exact same-second baseline, accepted-view header/insight pairing, clipped candidate ID, pending one-job controls, standalone origin/reload/source navigation, broader-root explanation, file mode separation, ordinary nonempty children');
}catch(e){const page=browser?.contexts()[0]?.pages()[0];if(page)await page.screenshot({path:new URL('focused-browser-failure.png',out).pathname});throw e;}finally{await browser?.close();await new Promise(r=>server.close(r));}
