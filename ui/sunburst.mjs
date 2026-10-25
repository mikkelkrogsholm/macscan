export const weight = n => BigInt(n?.bytes ?? n?.observed_bytes ?? n?.lower_bound_bytes ?? '0');
export function formatBytes(value) {
  if (value === null || value === undefined) return 'Ukendt';
  const n = Number(value), units = ['B','KB','MB','GB','TB','PB'];
  let i=0, v=n; while(Math.abs(v)>=1000 && i<units.length-1){v/=1000;i++;}
  return new Intl.NumberFormat('da-DK',{maximumFractionDigits:i?1:0}).format(v)+' '+units[i];
}
export function fraction(part,total) {
  if(total<=0n) return 0;
  return Number(part*1_000_000n/total)/1_000_000;
}
export function arcPath(inner,outer,start,end,cx=300,cy=300) {
  const point=(r,a)=>[cx+r*Math.sin(a),cy-r*Math.cos(a)];
  const [x0,y0]=point(outer,start),[x1,y1]=point(outer,end);
  const [x2,y2]=point(inner,end),[x3,y3]=point(inner,start);
  if(end-start>=Math.PI*2-1e-6) return `M ${cx} ${cy-outer} A ${outer} ${outer} 0 1 1 ${cx} ${cy+outer} A ${outer} ${outer} 0 1 1 ${cx} ${cy-outer} L ${cx} ${cy-inner} A ${inner} ${inner} 0 1 0 ${cx} ${cy+inner} A ${inner} ${inner} 0 1 0 ${cx} ${cy-inner} Z`;
  const big=end-start>Math.PI?1:0;
  return `M ${x0} ${y0} A ${outer} ${outer} 0 ${big} 1 ${x1} ${y1} L ${x2} ${y2} A ${inner} ${inner} 0 ${big} 0 ${x3} ${y3} Z`;
}
const hues=[262,204,171,44,18,326,235,143,285,77,4,187];
export function sectors(nodes,rootId,colors=new Map()) {
  const byParent=new Map(),root=nodes.find(n=>n.entry_id===rootId),result=[];
  for(const n of nodes) if(n.entry_id!==rootId){const p=n.parent_id;byParent.set(p,[...(byParent.get(p)??[]),n]);}
  for(const siblings of byParent.values()) siblings.sort((a,b)=>weight(a)>weight(b)?-1:weight(a)<weight(b)?1:Number(a.entry_id)-Number(b.entry_id));
  function visit(parent,start,end,depth,baseHue){
    if(depth>3) return;
    const children=byParent.get(parent.entry_id)??[],total=weight(parent);
    if(!children.length||total===0n) return;
    let angle=start, used=0n;
    children.forEach((child,index)=>{
      const bytes=weight(child);if(bytes<=0n)return;
      const hue=colors.get(child.entry_id)??(depth===1?hues[index%hues.length]:(baseHue+(index%5-2)*5+360)%360);
      colors.set(child.entry_id,hue);
      const stop=Math.min(end,angle+(end-start)*fraction(bytes,total));
      const gap=Math.min(0.006,(stop-angle)*0.1),fill=`hsl(${hue} ${depth===1?77:65}% ${depth===1?60:depth===2?67:74}%)`;
      if(stop-angle>0.0005) result.push({node:child,start:angle+gap,end:stop-gap,depth,fill,aggregate:false});
      visit(child,angle,stop,depth+1,hue);angle=stop;used+=bytes;
    });
    const rest=total-used;
    if(rest>0n&&end-angle>0.001) result.push({node:{entry_id:parent.entry_id,parent_id:parent.parent_id,name_display:'Øvrige poster',bytes:rest.toString(),kind:'aggregate',path_display:parent.path_display},start:angle+0.003,end:end-0.003,depth,fill:'#505969',aggregate:true});
  }
  if(root)visit(root,0,Math.PI*2,1,0);
  return result;
}
