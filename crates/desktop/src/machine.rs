//! Whole-startup-disk sessions reference ordinary immutable volume snapshots.
//! Capacity is OS APFS accounting, never a sum of file scans or disk images.
use macscan_analysis::{Store,FULL_HISTORY_BUDGET};
use macscan_engine::{volumes,ScanOptions};
use serde_json::{json,Value};
use std::{fs,io::{self,Read,Write},path::{Path,PathBuf},process::{Command,Stdio},sync::atomic::{AtomicBool,Ordering},time::{Instant,SystemTime,UNIX_EPOCH}};

fn plist(args:&[&str])->io::Result<Value>{
    let mut child=Command::new("/usr/sbin/diskutil").args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut bytes=Vec::new();child.stdout.take().unwrap().take(2*1024*1024+1).read_to_end(&mut bytes)?;
    if bytes.len()>2*1024*1024 {let _=child.kill();let _=child.wait();return Err(io::Error::other("oversize disk inventory"));}
    if !child.wait()?.success(){return Err(io::Error::other("macOS kunne ikke læse APFS-volumener. Prøv igen fra den lokale Macscan-app."));}
    let mut convert=Command::new("/usr/bin/plutil").args(["-convert","json","-o","-","-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    convert.stdin.take().unwrap().write_all(&bytes)?;
    let output=convert.wait_with_output()?;
    if !output.status.success(){return Err(io::Error::other("invalid APFS inventory"));}
    serde_json::from_slice(&output.stdout).map_err(io::Error::other)
}
pub fn mounts()->io::Result<Vec<Value>> {
    let mut all=Vec::new();let mut cursor=None;
    loop {let page=volumes(500,cursor.as_deref())?;all.extend(page["records"].as_array().cloned().unwrap_or_default());
        match page["next_cursor"].as_str(){Some(c) if cursor.as_deref()!=Some(c)=>cursor=Some(c.to_owned()),_=>break}
        if all.len()>4096{return Err(io::Error::other("oversize mount inventory"));}
    }
    Ok(all)
}
pub fn plan()->io::Result<Value>{
    let info=plist(&["info","-plist","/System/Volumes/Data"])?;
    build_plan(&plist(&["apfs","list","-plist"] )?,&info,&mounts()?)
}
fn number(v:&Value)->u64{v.as_u64().or_else(||v.as_str()?.parse().ok()).unwrap_or(0)}
fn physical_disk(s:&str)->&str{s.rsplit_once('s').filter(|(p,s)|p.starts_with("disk")&&s.bytes().all(|b|b.is_ascii_digit())).map(|(p,_)|p).unwrap_or(s)}
fn build_plan(apfs:&Value,info:&Value,mounts:&[Value])->io::Result<Value>{
    let containers=apfs["Containers"].as_array().ok_or_else(||io::Error::other("APFS containers unavailable"))?;
    let startup=containers.iter().find(|c|c["ContainerReference"]==info["APFSContainerReference"]&&info["APFSContainerReference"].is_string()).ok_or_else(||io::Error::other("startup container unavailable"))?;
    let stores=startup["PhysicalStores"].as_array().ok_or_else(||io::Error::other("physical stores unavailable"))?;
    let disks:Vec<_>=stores.iter().filter_map(|p|p["DeviceIdentifier"].as_str()).map(physical_disk).collect();
    if disks.is_empty(){return Err(io::Error::other("startup physical disk unavailable"));}
    let mut capacity=0u64;let mut free=0u64;let mut used=0u64;let mut rows=Vec::new();
    let mut seen=std::collections::HashSet::new();
    for c in containers {
        let matches=c["PhysicalStores"].as_array().is_some_and(|a|a.iter().any(|p|p["DeviceIdentifier"].as_str().is_some_and(|d|disks.contains(&physical_disk(d)))));
        if !matches{continue;}
        capacity=capacity.saturating_add(number(&c["CapacityCeiling"]));free=free.saturating_add(number(&c["CapacityFree"]));
        used=used.saturating_add(number(&c["CapacityCeiling"]).saturating_sub(number(&c["CapacityFree"])));
        for v in c["Volumes"].as_array().into_iter().flatten(){
            let uuid=v["APFSVolumeUUID"].as_str().ok_or_else(||io::Error::other("volume UUID unavailable"))?;
            if !seen.insert(uuid){continue;}
            let device=v["DeviceIdentifier"].as_str().unwrap_or("");
            let mount=mounts.iter().find(|m|{
                let source=m["device_display"].as_str().unwrap_or("");
                source==format!("/dev/{device}") || (v["Roles"].as_array().is_some_and(|a|a.iter().any(|r|r=="System"))&&source.strip_prefix(&format!("/dev/{device}s")).is_some_and(|s|!s.is_empty()&&s.bytes().all(|b|b.is_ascii_digit())))
            });
            let root=mount.and_then(|m|m["mount_display"].as_str());
            rows.push(json!({"uuid":uuid,"device":device,"name":v["Name"],"roles":v["Roles"],"container":c["ContainerReference"],"root":root,
                "os_used_bytes":number(&v["CapacityInUse"]).to_string(),"phase":if root.is_some(){"pending"}else{"unmounted"},"scan_id":null,
                "scan_status":null,"entries":0,"branches":[],"error":null}));
        }
    }
    // Small/system volumes complete first; the large data volume still gets live branch progress.
    rows.sort_by_key(|r|(r["roles"].as_array().is_some_and(|a|a.iter().any(|v|v=="Data")),number(&r["os_used_bytes"])));
    if !rows.iter().any(|r|r["uuid"]==info["VolumeUUID"]&&r["root"].is_string()) {return Err(io::Error::other("mounted data volume missing"));}
    Ok(json!({"kind":"whole_mac_v1","title":"Hele Mac’en","scope":"startup_physical_APFS_containers","capacity_bytes":capacity.to_string(),"free_bytes":free.to_string(),"used_bytes":used.to_string(),"volumes":rows,
        "accounting":"APFS container/volume capacity from macOS; file scans are a separate breakdown, not additive. Disk images and external disks excluded.",
        "coverage_note":"Kun monterede volumener gennemgås. Umonteret/afvist adgang, cloudområder, snapshots og APFS-deling forklares særskilt; filsummer er ikke sikkert frigørbare bytes."}))
}
fn id()->String{format!("{}-{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(),std::process::id())}
fn path(store:&Path,id:&str)->io::Result<PathBuf>{
    if id.is_empty()||id.len()>100||!id.bytes().all(|b|b.is_ascii_digit()||b==b'-'){return Err(io::Error::other("invalid machine session ID"));}
    Ok(store.join(format!("{id}.machine.json")))
}
pub fn load(store:&Path,id:&str)->io::Result<Value>{
    let p=path(store,id)?;if fs::metadata(&p)?.len()>256*1024{return Err(io::Error::other("oversize machine session"));}
    let value:Value=serde_json::from_slice(&fs::read(p)?).map_err(io::Error::other)?;
    if value["kind"]!="whole_mac_v1"||value["session_id"]!=id{return Err(io::Error::other("invalid machine session"));}
    Ok(value)
}
pub fn history(store:&Path)->Vec<Value>{
    let Ok(entries)=fs::read_dir(store)else{return Vec::new();};
    let mut result=Vec::new();
    for e in entries.flatten(){let name=e.file_name();let name=name.to_string_lossy();let Some(id)=name.strip_suffix(".machine.json")else{continue;};
        if let Ok(s)=load(store,id){result.push(json!({"kind":"machine","scan_id":id,"root":"Hele Mac’en","started_at":s["started_at"],"sort_at":s["ended_at"],"status":s["status"]}));}
    }
    result
}
fn publish(store:&Path,s:&Value)->io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let p=path(store,s["session_id"].as_str().unwrap())?;let temp=p.with_extension("tmp");
    let mut f=fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
    f.write_all(&serde_json::to_vec(s)?)?;f.sync_all()?;
    if p.exists(){return Err(io::Error::other("machine session already exists"));}
    fs::rename(temp,p)
}
fn options_for(root:&str)->io::Result<ScanOptions>{
    let mut options=ScanOptions::full();
    if root=="/" {
        // stat/device boundaries alone do not reliably identify macOS firmlinks.
        // Data is scanned at its canonical volume root, exactly once.
        let links=fs::read_to_string("/usr/share/firmlinks")?;
        if links.len()>64*1024{return Err(io::Error::other("oversize firmlink inventory"));}
        options.exclusions=system_exclusions(&links);
    }
    Ok(options)
}
fn system_exclusions(links:&str)->Vec<PathBuf>{
    let mut roots=vec![PathBuf::from("/System/Volumes"),PathBuf::from("/Volumes"),PathBuf::from("/dev")];
    for line in links.lines(){if let Some(source)=line.split('\t').next().filter(|s|s.starts_with('/')&&*s!="/") {let path=PathBuf::from(source);if !roots.contains(&path){roots.push(path);}}}
    roots.sort();roots
}
pub fn run(store:&Path,mut session:Value,cancel:&AtomicBool,mut progress:impl FnMut(Value))->io::Result<String>{
    let store=Store::open(store,true)?;let start=Instant::now();let session_id=id();
    session["session_id"]=json!(session_id);session["started_at"]=json!(utc());session["status"]=json!("running");
    let count=session["volumes"].as_array().unwrap().len();let mut completed=0usize;
    for i in 0..count {
        if cancel.load(Ordering::Relaxed){break;}
        let Some(root)=session["volumes"][i]["root"].as_str().map(str::to_owned) else{continue;};
        session["volumes"][i]["phase"]=json!("scanning");
        let result=options_for(&root).and_then(|options|store.scan_to_store(Path::new(&root),&options,FULL_HISTORY_BUDGET,cancel,|p|{
            session["volumes"][i]["phase"]=json!(if p.finished{"saving"}else{"scanning"});
            session["volumes"][i]["entries"]=json!(p.entries_seen);session["volumes"][i]["branches"]=json!(p.branches);
            progress(json!({"phase":if p.finished{"saving"}else{"scanning"},"whole_mac":true,"root":root,"entries_seen":completed+p.entries_seen,"elapsed_ms":start.elapsed().as_millis(),"machine":session}));
        }));
        // Live batches are ephemeral; only final corrected snapshots are retained.
        session["volumes"][i]["branches"]=json!([]);
        match result {
            Ok(id)=>{let m=store.manifest(&id)?;completed+=m["entry_count"].as_u64().unwrap_or(0) as usize;
                session["volumes"][i]["scan_id"]=json!(id);session["volumes"][i]["phase"]=json!("done");session["volumes"][i]["scan_status"]=m["status"].clone();
                session["volumes"][i]["totals"]=m["totals"].clone();},
            Err(e)=>{session["volumes"][i]["phase"]=json!(if cancel.load(Ordering::Relaxed){"cancelled"}else{"error"});session["volumes"][i]["error"]=json!(e.to_string());
                if e.kind()==io::ErrorKind::StorageFull {session["error"]=json!(e.to_string());break;}
            }
        }
        progress(json!({"phase":"scanning","whole_mac":true,"root":root,"entries_seen":completed,"elapsed_ms":start.elapsed().as_millis(),"machine":session}));
    }
    let cancelled=cancel.load(Ordering::Relaxed);
    for row in session["volumes"].as_array_mut().unwrap(){if row["phase"]=="pending" {row["phase"]=json!(if cancelled{"cancelled"}else{"not_scanned"});}}
    session["status"]=json!(if cancelled{"cancelled"}else if session["volumes"].as_array().unwrap().iter().any(|v|v["phase"]!="done"||v["scan_status"]!="complete"){"partial"}else{"complete"});
    session["entry_count"]=json!(completed);session["ended_at"]=json!(utc());
    publish(store.path(),&session)?;
    Ok(session_id)
}
fn utc()->String { // Existing engine timestamps have the same public UTC format.
    let t=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as libc::time_t;
    let mut out=std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe{libc::gmtime_r(&t,out.as_mut_ptr())}.is_null(){return String::new();}
    let x=unsafe{out.assume_init()};format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",x.tm_year+1900,x.tm_mon+1,x.tm_mday,x.tm_hour,x.tm_min,x.tm_sec)
}

#[cfg(test)]mod tests {
    use super::*;
    #[test]fn system_aliases_and_mounted_images_are_explicitly_pruned(){
        let exclusions=system_exclusions("/Applications\tApplications\n/Library\tLibrary\n/usr/local\tusr/local\n");
        for p in ["/Applications","/Library","/usr/local","/System/Volumes","/dev","/Volumes"]{assert!(exclusions.contains(&PathBuf::from(p)));}
        assert!(!exclusions.contains(&PathBuf::from("/")));assert!(!exclusions.contains(&PathBuf::from("/System/Library")));
        assert!(options_for("/System/Volumes/Data").unwrap().exclusions.is_empty());
    }
    #[test]fn grouping_excludes_images_external_and_duplicate_firmlinks(){
        let a=json!({"Containers":[{"ContainerReference":"disk3","CapacityCeiling":1000,"CapacityFree":200,"PhysicalStores":[{"DeviceIdentifier":"disk0s2"}],"Volumes":[
            {"APFSVolumeUUID":"data","DeviceIdentifier":"disk3s5","Roles":["Data"],"Name":"Data","CapacityInUse":600},
            {"APFSVolumeUUID":"sys","DeviceIdentifier":"disk3s1","Roles":["System"],"Name":"System","CapacityInUse":100},
            {"APFSVolumeUUID":"rec","DeviceIdentifier":"disk3s3","Roles":["Recovery"],"CapacityInUse":50}]},
            {"ContainerReference":"disk8","CapacityCeiling":9999,"PhysicalStores":[{"DeviceIdentifier":"disk7s1"}],"Volumes":[{"APFSVolumeUUID":"image","DeviceIdentifier":"disk8s1","CapacityInUse":9000}]}]});
        let info=json!({"APFSContainerReference":"disk3","VolumeUUID":"data"});
        let mounts=vec![json!({"device_display":"/dev/disk3s1s1","mount_display":"/"}),json!({"device_display":"/dev/disk3s5","mount_display":"/System/Volumes/Data"}),json!({"device_display":"/dev/disk8s1","mount_display":"/Volumes/Simulator"})];
        let p=build_plan(&a,&info,&mounts).unwrap();assert_eq!(p["volumes"].as_array().unwrap().len(),3);assert_eq!(p["used_bytes"],"800");
        assert!(p["volumes"].as_array().unwrap().iter().any(|v|v["uuid"]=="sys"&&v["root"]=="/"));
        assert!(p["volumes"].as_array().unwrap().iter().any(|v|v["uuid"]=="rec"&&v["phase"]=="unmounted"));
        assert!(path(Path::new("/tmp"),"../oops").is_err());
    }
    #[test]fn cancellation_keeps_completed_volume_results_and_marks_unvisited_unknown(){
        let base=std::env::temp_dir().join(format!("macscan-machine-cancel-{}",id()));fs::create_dir(&base).unwrap();
        let a=base.join("a");let b=base.join("b");fs::create_dir(&a).unwrap();fs::create_dir(&b).unwrap();fs::write(a.join("tiny"),b"abc").unwrap();fs::write(b.join("keep"),b"xyz").unwrap();
        let s=json!({"kind":"whole_mac_v1","volumes":[{"root":a,"phase":"pending"},{"root":b,"phase":"pending"}]});
        let cancel=AtomicBool::new(false);let history=base.join("history");
        let session=run(&history,s,&cancel,|p|{if p["machine"]["volumes"][0]["phase"]=="done"{cancel.store(true,Ordering::Relaxed);}}).unwrap();
        let saved=load(&history,&session).unwrap();assert_eq!(saved["status"],"cancelled");assert!(saved["volumes"][0]["scan_id"].is_string());assert_eq!(saved["volumes"][1]["phase"],"cancelled");assert_eq!(saved["entry_count"],2);
        assert_eq!(fs::read(b.join("keep")).unwrap(),b"xyz");let store=Store::open(&history,false).unwrap();assert_eq!(store.ids().unwrap().len(),1);
        assert!(!fs::read_dir(&history).unwrap().flatten().any(|e|e.file_name().to_string_lossy().ends_with(".partial")));
        fs::remove_dir_all(base).unwrap();
    }
    #[test]fn failed_volume_does_not_erase_other_results(){
        let base=std::env::temp_dir().join(format!("macscan-machine-error-{}",id()));fs::create_dir(&base).unwrap();let a=base.join("ok");fs::create_dir(&a).unwrap();fs::write(a.join("file"),b"a").unwrap();
        let s=json!({"kind":"whole_mac_v1","volumes":[{"root":base.join("missing"),"phase":"pending"},{"root":a,"phase":"pending"},{"root":null,"phase":"unmounted"}]});
        let history=base.join("history");let session=run(&history,s,&AtomicBool::new(false),|_|{}).unwrap();let saved=load(&history,&session).unwrap();
        assert_eq!(saved["status"],"partial");assert_eq!(saved["volumes"][0]["phase"],"error");assert!(saved["volumes"][1]["scan_id"].is_string());assert_eq!(saved["volumes"][2]["phase"],"unmounted");fs::remove_dir_all(base).unwrap();
    }
}
