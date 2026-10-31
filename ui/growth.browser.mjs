// Optional real-browser interaction check. Uses an existing Playwright installation;
// no dependency or browser download is performed.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE??'playwright');
const root=new URL('./',import.meta.url);
const server=createServer(async(req,res)=>{try{
 const name=decodeURIComponent(new URL(req.url,'http://localhost').pathname).slice(1)||'index.html';
 if(!/^[\w.-]+$/.test(name))throw Error('path');
 const data=await readFile(new URL(name,root));res.setHeader('Content-Type',name.endsWith('.css')?'text/css':/\.(js|mjs)$/.test(name)?'text/javascript':'text/html');res.end(data);
}catch{res.writeHead(404);res.end();}});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
let browser;
try{
 browser=await chromium.launch({headless:true,channel:process.env.MACSCAN_BROWSER_CHANNEL??'chrome'});const page=await browser.newPage({viewport:{width:1280,height:850}});const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  const scans=[{scan_id:'400-2',root:'/synthetic',policy:'p',started_at:'2026-10-22T10:00:00Z',status:'complete'},{scan_id:'400-1',root:'/synthetic',policy:'p',started_at:'2026-10-21T10:00:00Z',status:'complete'}];
  window.calls=[];window.mockCase='normal';
  const row={group_key:'project',path_display:'/synthetic/project',delta_bytes:'-3',changed_file_before_bytes:'10',changed_file_after_bytes:'7',observed_changed_file_before_bytes:'10',observed_changed_file_after_bytes:'7',file_path_count:'1',new_count:'0',removed_count:'0',changed_count:'1',unknown_count:'0',context_coverage:'complete',category:'dependencies'};
  window.__TAURI__={menu:{Menu:{}},core:{invoke:async(name,args={})=>{
   window.calls.push({name,args});
   if(name==='bootstrap')return {scans,volumes:{records:[]},store:'/mock-history',explicit_store:true};
   if(name==='view_scan'){
    if(window.holdView)await new Promise(r=>window.finishView=r);
    const selected={entry_id:String(args.entry),parent_id:args.entry?'0':null,name_display:args.entry===3?'project':args.entry===4?'removed':'synthetic',path_display:'/synthetic/'+(args.entry===3?'project':args.entry===4?'removed':''),kind:args.entry===4?'file':'directory',bytes:'20',coverage_complete:true};
    return {selected,ancestors:[{entry_id:'0',name_display:'synthetic'}],nodes:args.entry?[selected]:[selected,{...selected,entry_id:'3',parent_id:'0',name_display:'project',path_display:'/synthetic/project',bytes:'17'}],children:{records:args.entry===4?[]:[{entry_id:args.entry?'5':'3',parent_id:String(args.entry),name_display:args.entry?'ordinary-child':'project',kind:args.entry?'file':'directory',bytes:'7',coverage_complete:true}],next_cursor:null},scope:{root_display:'/synthetic'},started_at:scans.find(s=>s.scan_id===args.scanId).started_at,status:'complete',entry_count:'5',child_count:args.entry===4?'0':'1'};
   }
   if(name==='list_children')return {records:[],next_cursor:null};
   if(name==='growth'){
    if(window.mockCase==='error')throw Error('resource budget');
    if(window.mockCase==='stale')await new Promise(r=>window.finishGrowth=r);
    const partial=window.mockCase==='partial',file={group_key:'project',path_display:'/synthetic/removed',before_entry_id:'4',after_entry_id:null,before_bytes:'3',after_bytes:'0',delta_bytes:'-3',change:'removed',context_coverage:'complete',drilldown_scan_id:'400-1',drilldown_entry_id:'4'};
    const records=args.files?[file]:[{...row,path_display:args.cursor?'/synthetic/page-two':'/synthetic/project',...(partial?{delta_bytes:null,changed_file_after_bytes:null,observed_changed_file_after_bytes:'7'}:{})}];
    return {before_scan_id:'400-1',scan_id:'400-2',records,next_cursor:args.cursor?null:'page-2',grouping:args.grouping,mode:args.files?'files':'groups',metric:args.metric,scope:{after:{path_display:args.entry?'/synthetic/project':'/synthetic'}},interval:{before_measured_at:'2026-10-21T10:00:00Z',after_measured_at:'2026-10-22T10:00:00Z',elapsed_ns:'86400000000000'},coverage:{complete:!partial},totals:{before:{bytes:'100',file_path_count:'5'},after:{bytes:partial?null:'97',observed_lower_bound_bytes:'90',file_path_count:'5'}},contributions:{...row,...(partial?{delta_bytes:null,changed_file_after_bytes:null}:{})},growth_version:'growth-1',rule_version:'insights-1',accounting_note:'Kun ændrede almindelige filstier',attribution_note:'Gemt kontekst'};
   }
   throw Error('Unexpected '+name);
  }}};
 });
 await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>document.querySelector('#view-title').textContent==='synthetic');
 await page.locator('#rows .file-row').click();await page.waitForFunction(()=>document.querySelector('#view-title').textContent==='project');
 await page.click('#changes-tab');await page.waitForSelector('#growth-scope');await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('Ændringsbidrag: -3 B'));
 assert.match(await page.locator('#growth-panel').innerText(),/86\.400\.000\.000\.000 ns/);
 assert.match(await page.locator('#growth-panel').innerText(),/Alle almindelige filer/);
 await page.getByRole('button',{name:'Næste side',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('page-two'));
 assert.equal(await page.locator('#growth-panel .insight-row').count(),1);assert.equal(await page.locator('#growth-panel .insight-row').innerText().then(s=>s.includes('/synthetic/project')),false);
 await page.selectOption('#growth-scope','current');
 await page.click('#logical');await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error').innerText());
 await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('· Logisk')&&document.querySelector('#growth-panel').textContent.includes('Ændringsbidrag: -3 B'));
 assert.deepEqual(await page.evaluate(()=>{const q=window.calls.filter(c=>c.name==='growth').at(-1).args;return {entry:q.entry,metric:q.metric,before:q.before,after:q.after};}),{entry:3,metric:'logical',before:'400-1',after:'400-2'});
 assert.equal(await page.locator('#rows').isVisible(),false);
 assert.equal(await page.locator('#changes-tab').getAttribute('aria-pressed'),'true');assert.equal(await page.locator('#growth-scope').inputValue(),'current');
 // Navigate in the graph while changes stay active; current scope must use AFTER id.
 await page.click('#chart-center');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='growth').at(-1).args.entry===0);
 assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error').innerText());
 const point=await page.locator('#sunburst').evaluate(svg=>{const p=svg.createSVGPoint();p.x=425;p.y=300;const result=p.matrixTransform(svg.getScreenCTM());return {x:result.x,y:result.y};});await page.mouse.click(point.x,point.y);await page.waitForFunction(()=>window.calls.filter(c=>c.name==='growth').at(-1).args.entry===3);
 assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error').innerText());assert.match(await page.locator('#growth-panel').innerText(),/Ændringsbidrag: -3 B/);
 assert.equal(await page.locator('#before-scan').inputValue(),'400-1');assert.equal(await page.locator('#growth-scope').inputValue(),'current');
 await page.evaluate(()=>window.mockCase='partial');await page.selectOption('#growth-change','unknown');await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('Ukendt ændring'));
 assert.match(await page.locator('#growth-panel').innerText(),/mindst observeret 90 B/);
 await page.evaluate(()=>window.mockCase='error');await page.selectOption('#growth-change','all');await page.waitForSelector('#growth-panel button.more');await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('resource budget'));
 assert.match(await page.locator('#growth-panel').innerText(),/mindre mappe i grafen/);
 await page.evaluate(()=>window.mockCase='normal');await page.getByRole('button',{name:'Prøv igen',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#growth-panel .insight-open'));
 await page.selectOption('#growth-grouping','category');await page.waitForFunction(()=>document.querySelector('#growth-panel .insight-open').textContent==='Projektafhængigheder');
 await page.locator('#growth-panel .insight-open').click();await page.waitForFunction(()=>document.querySelector('#growth-panel').textContent.includes('Fjernet filsti'));
 await page.locator('#growth-panel .insight-open').click();await page.waitForFunction(()=>window.calls.some(c=>c.name==='view_scan'&&c.args.scanId==='400-1'&&c.args.entry===4));
 assert.equal(await page.locator('#files-tab').getAttribute('aria-pressed'),'true');
 assert.equal(await page.evaluate(()=>window.calls.some(c=>c.name==='view_scan'&&c.args.scanId==='400-2'&&c.args.entry===4)),false);
 // Changing tab during a graph request must not reopen Changes on resolution.
 await page.click('#changes-tab');await page.evaluate(()=>{window.holdView=true;document.querySelector('#logical').click();});await page.waitForFunction(()=>typeof window.finishView==='function');await page.locator('#files-tab').focus();await page.locator('#files-tab').press('Enter');await page.evaluate(()=>{window.holdView=false;window.finishView();});await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 assert.equal(await page.locator('#files-tab').getAttribute('aria-pressed'),'true');
 assert.deepEqual(errors,[]);console.log('PASS: nonempty ordinary children through project metric/graph refresh without error banner, logical project -3 B; panel totals, exact interval/delta, bounded page replacement, scope/metric preservation, partial/resource recovery, category drilldown, BEFORE file open, tab race; no page errors');
}finally{await browser?.close();await new Promise(r=>server.close(r));}
