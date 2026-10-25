// One native menu, shared by chart sectors and keyboard-accessible list rows.
export function createEntryMenu(Menu, runAction, reportError) {
 let menuPromise, target, showing=false;
 return async function show(entry, position) {
  if(showing)return;
  showing=true;
  target={scanId:entry.scanId,entry:Number(entry.entry)};
  try {
   menuPromise??=Menu.new({items:[
    ['reveal','Vis i Finder'],['copy','Kopiér filsti'],['copy_info','Kopiér information']
   ].map(([id,text])=>({id,text,action:()=>{
    const selected=target;
    return Promise.resolve().then(()=>runAction(id,selected)).catch(reportError);
   }}))}).catch(e=>{menuPromise=undefined;throw e;});
   const menu=await menuPromise;
   await menu.popup(position);
  }catch(e){reportError(e);}
  finally{showing=false;}
 };
}
