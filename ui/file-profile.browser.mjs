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
    const selected={entry_id:String(args.entry),parent_id:args.entry?'0':null,name_display:args.entry===60?'Downloads':args.entry===61?'old.zip':args.entry===3?'project':args.entry===4?'removed':'synthetic',path_display:'/synthetic/'+(args.entry===3?'project':args.entry===4?'removed':''),kind:args.entry===4||args.entry===61?'file':'directory',bytes:'20',coverage_complete:true};
    return {selected,ancestors:[{entry_id:'0',name_display:'synthetic'}],nodes:args.entry?[selected]:[selected,{...selected,entry_id:'3',parent_id:'0',name_display:'project',path_display:'/synthetic/project',bytes:'17'}],children:{records:args.entry===4||args.entry===61?[]:[{entry_id:args.entry?'5':'3',parent_id:String(args.entry),name_display:args.entry?'ordinary-child':'project',kind:args.entry?'file':'directory',bytes:'7',coverage_complete:true},...(!args.entry?[{entry_id:'60',parent_id:'0',name_display:'Downloads',kind:'directory',bytes:'7',coverage_complete:true}]:[])],next_cursor:null},scope:{root_display:'/synthetic'},started_at:scans.find(s=>s.scan_id===args.scanId).started_at,status:'complete',entry_count:'5',child_count:args.entry===4?'0':'1'};
   }
   if(name==='list_children')return {records:[],next_cursor:null};
   if(name==='file_profile'){
    if(window.holdProfile)await new Promise(r=>window.finishProfile=r);
    if(window.mockCase==='profile-error')throw Error('saved reference unavailable');
    const partial=window.mockCase==='partial';
    const common={bytes:partial?null:'9007199254740993',observed_lower_bound_bytes:'9007199254740993',file_path_count:'21',unknown_size_count:'1',coverage_complete:!partial};
    const records=args.types?[{...common,type_key:'none',type_display:'Uden endelse'},{...common,type_key:'ext:FF',type_display:'�'},{...common,type_key:'ext:FE',type_display:'�'}]:[{...common,entry_id:'61',path_display:args.cursor?'/synthetic/page-two':'/synthetic/old.zip',drilldown_entry_id:'61',drilldown_scan_id:args.scanId,type_key:args.typeKey??'ext:7A6970',type_display:'zip',mtime_unix_seconds:'0',future_mtime:false}];
    return {coverage:{complete:!partial},filters:{min_bytes:args.minBytes,min_age_days:args.minAgeDays},mode:args.types?'types':'files',records,next_cursor:args.cursor?null:'page-2',totals:common,population:{file_path_count:'50',unknown_mtime_count:'2',future_mtime_count:'1',unassessed_mtime_count:'0',unknown_type_count:'3',unknown_size_count:'1'},scope:{path_display:args.entry===60?'/synthetic/Downloads':args.entry?'/synthetic/project':'/synthetic'},metric:args.metric,measured_at:'2026-10-22T10:00:00Z',reference_unix_ns:'1792663200000000000',age_threshold_unix_ns:args.minAgeDays?'1777111200000000000':null};
   }
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
 assert.equal(await page.locator('#rows .file-row').count(),2);
 await page.locator('#rows .file-row').first().click();await page.waitForFunction(()=>document.querySelector('#view-title').textContent==='project');
 await page.click('#find-files');await page.waitForSelector('#profile-panel .insight-row');
 assert.match(await page.locator('#profile-panel').innerText(),/9\.007\.199\.254\.740\.993 B/);
 assert.match(await page.locator('#profile-panel').innerText(),/21 filstier/);assert.match(await page.locator('#profile-panel').innerText(),/1970-01-01/);
 const last=()=>page.evaluate(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args);
 assert.equal((await last()).entry,3);
 await page.click('#profile-next');await page.waitForFunction(()=>document.querySelector('#profile-panel').textContent.includes('page-two'));
 assert.equal(await page.locator('#profile-panel .insight-row').count(),1);assert.doesNotMatch(await page.locator('#profile-panel .insight-row').innerText(),/old.zip/);
 await page.fill('#profile-age','90');await page.locator('#profile-age').press('Tab');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.minAgeDays==='90');
 assert.equal((await last()).cursor,null);assert.equal(await page.evaluate(()=>document.activeElement.id),'profile-size');
 await page.locator('#profile-size').fill('2');await page.locator('#profile-size').press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'profile-unit');await page.selectOption('#profile-unit','1024');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.minBytes==='2048');assert.equal((await last()).minAgeDays,'90');
 await page.evaluate(()=>window.holdProfile=true);await page.fill('#profile-age','91');await page.locator('#profile-age').press('Tab');await page.waitForFunction(()=>typeof window.finishProfile==='function');await page.fill('#profile-size','13');await page.evaluate(()=>{window.holdProfile=false;window.finishProfile();});await page.waitForFunction(()=>document.querySelector('#profile-panel').getAttribute('aria-busy')==='false');assert.equal(await page.locator('#profile-size').inputValue(),'13');assert.equal(await page.evaluate(()=>document.activeElement.id),'profile-size');await page.locator('#profile-size').press('Tab');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.minBytes==='13312');
 await page.click('#logical');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.metric==='logical');
 assert.equal((await last()).entry,3);assert.equal((await last()).minAgeDays,'91');assert.equal(await page.locator('#profile-panel').isVisible(),true);assert.equal(await page.locator('#error').isVisible(),false);
 await page.click('#chart-center');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.entry===0);
 assert.equal((await last()).minAgeDays,'91');assert.equal((await last()).cursor,null);
 await page.selectOption('#profile-view','types');await page.waitForSelector('#profile-sort');await page.selectOption('#profile-sort','count');await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.sortCount);
 assert.match(await page.locator('#profile-panel .insight-open').nth(1).innerText(),/ext:FF/);assert.match(await page.locator('#profile-panel .insight-open').nth(2).innerText(),/ext:FE/);
 await page.locator('#profile-panel .insight-open').nth(1).click();await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.typeKey==='ext:FF');
 assert.equal((await last()).types,false);await page.click('#profile-clear-type');await page.waitForSelector('#profile-sort');
 await page.locator('#profile-panel .insight-open').nth(2).click();await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.typeKey==='ext:FE');
 await page.click('#profile-clear-type');await page.waitForSelector('#profile-sort');await page.locator('#profile-panel .insight-open').first().click();await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.typeKey==='none');
 await page.click('#profile-close');assert.equal(await page.locator('#profile-panel').isVisible(),false);assert.equal(await page.locator('#rows .file-row').count(),2);
 await page.locator('#rows .file-row').nth(1).click();await page.waitForFunction(()=>document.querySelector('#view-title').textContent==='Downloads');
 await page.click('#find-files');await page.waitForSelector('#profile-preset');assert.match(await page.locator('#profile-preset').innerText(),/^Downloads:/);await page.click('#profile-preset');
 await page.waitForFunction(()=>window.calls.filter(c=>c.name==='file_profile').at(-1).args.minBytes==='104857600');assert.equal((await last()).minAgeDays,'180');assert.equal((await last()).entry,60);assert.equal((await last()).typeKey,null);
 await page.evaluate(()=>window.mockCase='partial');await page.selectOption('#profile-scope','all');await page.waitForFunction(()=>document.querySelector('#profile-panel').textContent.includes('nedre grænse'));assert.match(await page.locator('#profile-panel').innerText(),/Ukendt · mindst observeret/);
 await page.evaluate(()=>window.mockCase='profile-error');await page.selectOption('#profile-scope','current');await page.waitForSelector('#profile-clear-age');assert.match(await page.locator('#profile-panel').innerText(),/mindre gemt mappe/);
 await page.evaluate(()=>window.mockCase='normal');await page.click('#profile-clear-age');await page.waitForSelector('#profile-panel .insight-row');assert.equal((await last()).minAgeDays,null);
 await page.locator('#profile-panel').evaluate(p=>p.scrollTop=0);await page.screenshot({path:new URL('../evidence/issue-5B/profile-panel.png',import.meta.url).pathname,fullPage:true});
 await page.locator('#profile-panel .insight-open').click();await page.waitForFunction(()=>window.calls.some(c=>c.name==='view_scan'&&c.args.entry===61));assert.equal(await page.locator('#profile-panel').isVisible(),false);
 // A profile request resolved after returning to children cannot reopen its panel.
 await page.click('#find-files');await page.waitForSelector('#profile-panel .insight-row');await page.evaluate(()=>window.holdProfile=true);await page.selectOption('#profile-scope','all');await page.waitForFunction(()=>typeof window.finishProfile==='function');
 await page.click('#files-tab');await page.evaluate(()=>{window.holdProfile=false;window.finishProfile();});assert.equal(await page.locator('#profile-panel').isVisible(),false);
 // Tab choice during pending graph navigation wins when the navigation resolves.
 await page.click('#find-files');await page.waitForSelector('#profile-panel .insight-row');await page.evaluate(()=>{window.holdView=true;document.querySelector('#allocated').click();});await page.waitForFunction(()=>typeof window.finishView==='function');await page.locator('#changes-tab').focus();await page.locator('#changes-tab').press('Enter');await page.evaluate(()=>{window.holdView=false;window.finishView();});await page.waitForFunction(()=>!document.querySelector('.workspace').classList.contains('loading'));
 assert.equal(await page.locator('#changes-tab').getAttribute('aria-pressed'),'true');assert.equal(await page.locator('#profile-panel').isVisible(),false);
 assert.equal(await page.locator('#error').isVisible(),false);assert.deepEqual(errors,[]);
 console.log('PASS: filter Tab focus and size/unit editing, in-progress edit preserved across delayed answer; nonempty ordinary children, exact totals/time-zero, page replacement, age reset, metric and graph current scope preservation, opaque none/FF/FE type drilldown, count sort, Downloads preset, partial population, reference recovery, file dive, stale request and tab/navigation races; no page errors or error banner');
}finally{await browser?.close();await new Promise(r=>server.close(r));}
