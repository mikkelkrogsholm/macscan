// Saved synthetic data only. Exercise the same SVG listeners used by the app.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE??'playwright');
const server=createServer(async(req,res)=>{try{const name=new URL(req.url,'http://localhost').pathname.slice(1)||'index.html';if(!/^[\w.-]+$/.test(name))throw Error();res.setHeader('Content-Type',name.endsWith('.css')?'text/css':/\.(js|mjs)$/.test(name)?'text/javascript':'text/html');res.end(await readFile(new URL(name,import.meta.url)));}catch{res.writeHead(404);res.end();}});
await new Promise(r=>server.listen(0,'127.0.0.1',r));let browser;
try{
 browser=await chromium.launch({headless:true,channel:'chrome'});const page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  window.calls=[];window.delay=false;window.reject=false;
  const root={entry_id:'0',parent_id:null,name_display:'Data',path_display:'/synthetic/data',kind:'directory',bytes:null,observed_bytes:'100',logical_bytes:null,coverage_complete:false};
  const folder={...root,entry_id:'1',parent_id:'0',name_display:'Folder',path_display:'/synthetic/data/Folder',observed_bytes:'80'};
  const child={...folder,entry_id:'2',parent_id:'1',name_display:'Nested',path_display:'/synthetic/data/Folder/Nested',observed_bytes:'60'};
  const scans=[{kind:'machine',scan_id:'20-1',root:'Hele Mac’en',started_at:'2026-10-31T12:00:00Z',status:'partial'},{scan_id:'19-1',grouped_volume:true,root:'/synthetic/data',started_at:'2026-10-31T12:00:00Z',status:'partial'}];
  window.__TAURI__={menu:{Menu:{}},core:{invoke:async(name,args={})=>{
   window.calls.push({name,args});
   if(name==='bootstrap')return {scans,volumes:{records:[]},store:'/synthetic',explicit_store:true};
   if(name==='machine_session')return {session_id:'20-1',started_at:scans[0].started_at,status:'partial',entry_count:3,used_bytes:'100',free_bytes:'100',coverage_note:'Partial saved volume',volumes:[{name:'Data',root:'/synthetic/data',phase:'done',scan_id:'19-1',scan_status:'partial',os_used_bytes:'100',totals:{allocated_bytes:null}}]};
   if(name==='view_scan'){
    if(window.delay)await new Promise(r=>window.release=r);
    if(window.reject)throw Error('Saved query failed; retry');
    const selected=args.entry===0?root:args.entry===1?folder:child,children=args.entry===0?[folder]:args.entry===1?[child]:[];
    return {scan_id:args.scanId,selected,ancestors:args.entry===0?[root]:args.entry===1?[root,folder]:[root,folder,child],nodes:[selected,...children],children:{records:children,next_cursor:null},scope:{root_display:'/synthetic/data'},started_at:scans[0].started_at,status:'partial',entry_count:'3',child_count:String(children.length)};
   }
   if(name==='list_children')return {records:[],next_cursor:null};
   throw Error('Unexpected '+name);
  }}};
 });
 page.setDefaultTimeout(8000);
 const clickSector=async()=>{const point=await page.locator('#sunburst .sector:not(.aggregate)').first().evaluate(p=>{for(let a=0;a<360;a+=10){const angle=a*Math.PI/180,point=new DOMPoint(300+125*Math.cos(angle),300+125*Math.sin(angle));if(p.isPointInFill(point)){const screen=point.matrixTransform(p.getScreenCTM());return {x:screen.x,y:screen.y};}}throw Error('No clickable sector point');});await page.mouse.click(point.x,point.y);};
 const ready=()=>page.waitForFunction(()=>document.querySelector('.workspace').getAttribute('aria-busy')==='false');
 const whole=()=>page.getByRole('button',{name:'Hele Mac’en',exact:true}).last().click();
 await page.goto('http://127.0.0.1:'+server.address().port);await ready();
 const sector=page.locator('#sunburst .sector:not(.aggregate)').first();
 await page.evaluate(()=>{window.delay=true;window.reject=true;});await clickSector();
 await page.waitForFunction(()=>typeof window.release==='function');assert.equal(await page.locator('#navigation-status').isVisible(),true);assert.equal(await page.locator('.workspace').getAttribute('aria-busy'),'true');assert.equal(await page.locator('#view-title').innerText(),'Hele Mac’en');
 // Synthetic repeated events must not queue another expensive request.
 await sector.dispatchEvent('click');assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='view_scan').length),1);
 await page.evaluate(()=>{window.delay=false;window.release();delete window.release;});await ready();assert.match(await page.locator('#error').innerText(),/Saved query failed/);assert.equal(await page.locator('#view-title').innerText(),'Hele Mac’en');
 await page.evaluate(()=>window.reject=false);await clickSector();await ready();assert.equal(await page.locator('#view-title').innerText(),'Data');assert.match(await page.locator('#coverage').innerText(),/Delvis/);assert.equal(await page.locator('#error').isVisible(),false);
 await clickSector();await ready();assert.equal(await page.locator('#view-title').innerText(),'Folder');
 await page.locator('#rows .file-row').click();await ready();assert.equal(await page.locator('#view-title').innerText(),'Nested');await whole();assert.equal(await page.locator('#view-title').innerText(),'Hele Mac’en');
 // List entry follows the same transactional preload boundary.
 await page.locator('#rows .file-row').click();await ready();assert.equal(await page.locator('#view-title').innerText(),'Data');await whole();
 // Returning to the overview invalidates both successful and rejected old reads.
 for(const reject of [false,true]){
  await page.evaluate(r=>{window.delay=true;window.reject=r;},reject);await clickSector();await page.waitForFunction(()=>typeof window.release==='function');await page.locator('#history-list .history-item').click();await ready();
  await page.evaluate(()=>{window.delay=false;window.release();delete window.release;});await page.waitForTimeout(50);assert.equal(await page.locator('#view-title').innerText(),'Hele Mac’en');assert.equal(await page.locator('#error').isVisible(),false);assert.equal(await page.locator('#navigation-status').isVisible(),false);
 }
 assert.deepEqual(errors,[]);console.log('PASS actual SVG + list Data drilldown, partial coverage, delayed loading/repeated click guard, query error/retry, nested folder and whole-Mac breadcrumb, stale success/rejection, no unhandled promises');
}finally{await browser?.close();await new Promise(r=>server.close(r));}
