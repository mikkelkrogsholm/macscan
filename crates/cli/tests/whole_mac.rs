//! Noninstalled synthetic subprocess host invoking the exact installed CLI entrypoint.
//! The only planner supplied here constructs owned fixture roots; never host inventory.
use macscan_analysis::{Store,machine};
use serde_json::{json,Value};
use std::{fs,io::{BufRead,BufReader},path::{Path,PathBuf},process::{Command,Output,Stdio},sync::atomic::{AtomicBool,Ordering},time::{Duration,Instant}};
fn plan(base:&Path,case:&str)->Value{
    let mut rows=vec![json!({"uuid":"a","root":base.join("a"),"phase":"pending","scan_id":null,"roles":[],"os_used_bytes":"100"}),
        json!({"uuid":"b","root":base.join("b"),"phase":"pending","scan_id":null,"roles":[],"os_used_bytes":"200"})];
    if case=="io"{rows[1]["root"]=json!(base.join("missing"));}
    if case=="unmounted"{rows.push(json!({"uuid":"unmounted","root":null,"phase":"unmounted","scan_id":null,"roles":[]}));}
    if case=="many"{for n in 0..500{rows.push(json!({"uuid":format!("unmounted-{n}"),"root":null,"phase":"unmounted","scan_id":null,"roles":[]}));}}
    json!({"kind":"whole_mac_v1","scope":"synthetic-owned","capacity_bytes":"1000","used_bytes":"300","free_bytes":"700","volumes":rows})
}
struct Fixture(PathBuf);
impl Fixture{
    fn new()->Self{
        let p=std::env::temp_dir().canonicalize().unwrap().join(format!("macscan-whole-cli-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&p).unwrap();for n in ["a","b"]{fs::create_dir(p.join(n)).unwrap();for f in 0..8{fs::write(p.join(n).join(format!("file{f}")),b"owned").unwrap();}}Self(p)
    }
    fn store(&self)->PathBuf{self.0.join("store")}
    fn cmd(&self,case:&str,args:&[&str])->Command{
        let mut c=Command::new(std::env::current_exe().unwrap());
        c.arg("--host").arg(case).arg(&self.0).args(args).arg("--store").arg(self.store());c
    }
    fn run(&self,case:&str,args:&[&str],exit:i32)->Value{let out=self.cmd(case,args).output().unwrap();receipt(&out,exit)}
    fn saved(&self,run:&Value)->Value{machine::load(&self.store(),run["run_id"].as_str().unwrap()).unwrap()}
}
impl Drop for Fixture{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn receipt(out:&Output,exit:i32)->Value{
    assert_eq!(out.status.code(),Some(exit),"stdout={} stderr={}",String::from_utf8_lossy(&out.stdout),String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.len()<=8192);let mut diagnostic=None;
    for line in out.stderr.split(|b|*b==b'\n').filter(|b|!b.is_empty()){
        let v:Value=serde_json::from_slice(line).unwrap();if v["type"]!="progress"{diagnostic=Some(v);}
    }
    let v:Value=if matches!(exit,0|10){assert!(diagnostic.is_none());serde_json::from_slice(&out.stdout).unwrap()}
        else{assert!(out.stdout.is_empty());diagnostic.unwrap()};
    assert!(serde_json::to_vec(&v).unwrap().len()+1<=8192);assert_eq!(v["schema_version"],"3.0");v
}
fn main()->std::process::ExitCode{
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a|a=="--host"){
        let case=args[1].to_str().unwrap().to_owned();let base=PathBuf::from(&args[2]);let tail=args[3..].to_vec();
        return macscan::entry(tail,||{
            fs::write(base.join("planner-called"),b"synthetic planner only")?;
            if case=="must-not-plan"{panic!("invalid arguments reached planner");}
            if case=="pre-cancel"{unsafe{libc::raise(libc::SIGINT);}}
            Ok(plan(&base,&case))
        });
    }
    full_partial_capture_temporary_and_budgets();
    failures_busy_invalid_and_retention();
    publication_failure_and_stdout_failure();
    sigint_after_published_child();
    println!("PASS whole-Mac exact CLI entrypoint subprocesses: full, per-volume partial, unmounted, I/O, storage, busy, SIGINT, capture, shared budget, temporary, invalid-before-planner, compact JSON/NDJSON, retention");
    std::process::ExitCode::SUCCESS
}
fn full_partial_capture_temporary_and_budgets(){
    let f=Fixture::new();let args=["scan","--whole-mac","--save","--progress","ndjson"];
    let out=f.cmd("full",&args).output().unwrap();let v=receipt(&out,0);
    let events:Vec<Value>=out.stderr.split(|b|*b==b'\n').filter(|b|!b.is_empty()).map(|l|{assert!(l.len()<768);serde_json::from_slice(l).unwrap()}).collect();
    assert_eq!(events[0]["phase"],"plan");let terminals:Vec<_>=events.iter().filter(|p|p["phase"]=="volume_complete").collect();assert_eq!(terminals.len(),2);
    for (index,p) in terminals.iter().enumerate(){assert_eq!(p["volume_index"],index);assert_eq!(p["saved_members"],index+1);assert_eq!(p["volume_phase"],"done");assert_eq!(p["scan_status"],"complete");}
    let periodic=events.iter().filter(|p|p["phase"]!="plan"&&p["phase"]!="volume_complete").count();
    let max_ms=events.iter().filter_map(|p|p["elapsed_ms"].as_u64()).max().unwrap();assert!(periodic as u64<=1+max_ms/200);
    assert!(events.iter().all(|p|p.get("machine").is_none()&&p.get("root").is_none()&&p["run_id"]==v["run_id"]));
    assert_eq!(v["kind"],"whole_mac_v1");assert_eq!(v["retained"],true);assert_eq!(v["coverage"]["saved_members"],2);assert_eq!(v["os_capacity"]["used_bytes"],"300");
    let saved=f.saved(&v);let store=Store::open(&f.store(),false).unwrap();
    for row in saved["volumes"].as_array().unwrap(){let m=store.manifest(row["scan_id"].as_str().unwrap()).unwrap();assert_eq!(m["apfs_data_clone"]["capture_version"],Value::Null);}
    let members=f.run("must-not-plan",&["scans","--run",v["run_id"].as_str().unwrap()],0);assert_eq!(members["records"].as_array().unwrap().len(),2);
    let partial=f.run("full",&["scan","--whole-mac","--save","--max-entries","2"],10);assert_eq!(partial["outcome"],"partial");
    for row in f.saved(&partial)["volumes"].as_array().unwrap(){let m=store.manifest(row["scan_id"].as_str().unwrap()).unwrap();assert!(m["entry_count"].as_u64().unwrap()<=2);assert_eq!(m["status"],"partial");}
    let unmounted=f.run("unmounted",&args,10);assert_eq!(unmounted["coverage"]["unmounted_members"],1);
    let many=f.run("many",&args,10);assert_eq!(many["coverage"]["members"],502);assert!(serde_json::to_vec(&many).unwrap().len()<2048);
    let capture=f.run("full",&["scan","--whole-mac","--save","--capture-apfs-clones","--memory-budget","262144","--max-duration","20"],0);
    for row in f.saved(&capture)["volumes"].as_array().unwrap(){let id=row["scan_id"].as_str().unwrap();let m=store.manifest(id).unwrap();assert_eq!(m["context"]["clone_capture"]["requested"],true);assert_eq!(m["apfs_data_clone"]["capture_version"],1);assert!(fs::read_dir(f.store().join(id)).unwrap().any(|p|p.unwrap().file_name()=="manifest.json"));}
    let before=store.ids().unwrap();let child=f.cmd("full",&["scan","--whole-mac"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();let pid=child.id();let temporary=receipt(&child.wait_with_output().unwrap(),0);
    assert!(!fs::read_dir(std::env::temp_dir()).unwrap().any(|p|p.unwrap().file_name().to_string_lossy().starts_with(&format!("macscan-preview-{pid}-"))));assert_eq!(temporary["retained"],false);assert_eq!(temporary["temporary"],true);assert_eq!(temporary["members_query"],Value::Null);assert!(temporary["note"].as_str().unwrap().contains("removed"));assert_eq!(before,store.ids().unwrap());
    // A cleanup warning is surfaced without turning a published group into failure.
    fs::write(f.store().join("999-1.machine.json"),b"{}").unwrap();
    let warning=f.run("full",&["scan","--whole-mac","--save"],0);assert!(warning["retention_warning"].is_string());f.saved(&warning);
    fs::remove_file(f.store().join("999-1.machine.json")).unwrap();
    let before=store.ids().unwrap();
    // Explicit combined total does not gain a fresh allowance atop old metadata.
    let denied=f.run("full",&["scan","--whole-mac","--save","--metadata-budget","1"],5);assert_eq!(denied["outcome"],"failed");assert_eq!(denied["retained"],true);assert_eq!(denied["error"]["code"],"METADATA_LIMIT");assert_eq!(store.ids().unwrap(),before);
    let denied=f.run("full",&["scan","--whole-mac","--save","--metadata-budget","1","--format","ndjson"],5);assert_eq!(denied["type"],"error");assert_eq!(denied["retained"],true);
    let child=f.cmd("full",&["scan","--whole-mac","--metadata-budget","1"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();let pid=child.id();let failed_temporary=receipt(&child.wait_with_output().unwrap(),5);assert_eq!(failed_temporary["retained"],false);
    assert!(!fs::read_dir(std::env::temp_dir()).unwrap().any(|p|p.unwrap().file_name().to_string_lossy().starts_with(&format!("macscan-preview-{pid}-"))));
    // NDJSON uses the same compact receipt in its start/end framing.
    let out=f.cmd("full",&["scan","--whole-mac","--format","ndjson"]).output().unwrap();assert!(out.status.success());assert!(out.stdout.len()<=8192);for l in out.stdout.split(|b|*b==b'\n').filter(|b|!b.is_empty()){let v:Value=serde_json::from_slice(l).unwrap();assert_eq!(v["schema_version"],"3.0");}
}
fn failures_busy_invalid_and_retention(){
    let f=Fixture::new();let args=["scan","--whole-mac","--save"];
    let mut previous=Vec::new();for _ in 0..3{previous.push(f.run("full",&args,0));}
    let zero=f.run("pre-cancel",&["scan","--whole-mac","--save","--format","ndjson"],130);assert_eq!(zero["type"],"error");assert_eq!(zero["coverage"]["complete"],false);assert_eq!(zero["coverage"]["saved_members"],0);assert_eq!(zero["measurement_status"],"cancelled");
    let failed=f.run("io",&args,3);assert_eq!(failed["outcome"],"failed");assert_eq!(failed["coverage"]["saved_members"],1);assert_eq!(failed["error"]["code"],"IO_ERROR");
    for p in &previous{f.saved(p);}
    let s=Store::open(&f.store(),false).unwrap();let locked=s.begin_run().unwrap();let out=f.cmd("full",&args).output().unwrap();assert_eq!(out.status.code(),Some(7));let error:Value=serde_json::from_slice(&out.stderr).unwrap();assert_eq!(error["error"]["retryable"],true);drop(locked);
    let restored=f.run("full",&args,0);assert!(f.saved(&restored)["volumes"].is_array());assert_eq!(s.history_catalog().unwrap().machines.len(),3);
    for flags in [vec!["scan","--whole-mac","ROOT"],vec!["scan","ROOT","--whole-mac"],vec!["scan","--whole-mac","--metric","allocated"],vec!["scan","--whole-mac","--limit","10"],vec!["scan","--whole-mac","--max-entries","0"],vec!["scan","--whole-mac","--unknown"]]{
        let out=f.cmd("must-not-plan",&flags).output().unwrap();assert_eq!(out.status.code(),Some(2));assert!(out.stdout.is_empty());let _:Value=serde_json::from_slice(&out.stderr).unwrap();
    }
    // Measure one real child, then admit only 1.5 children: budget cannot renew.
    fn bytes(path:&Path)->u64{fs::read_dir(path).unwrap().map(|e|{let e=e.unwrap();if e.file_type().unwrap().is_dir(){bytes(&e.path())}else{e.metadata().unwrap().len()}}).sum()}
    let probe=Fixture::new();let single=Store::open(&probe.store(),true).unwrap().begin_run().unwrap();
    single.scan_to_store(&probe.0.join("a"),&macscan_engine::ScanOptions::full(),u64::MAX,&AtomicBool::new(false),|_|{}).unwrap();let footprint=bytes(&probe.store());
    let small=Fixture::new();let p=plan(&small.0,"full");let cancel=AtomicBool::new(false);
    let report=machine::run_configured(&small.store(),p,&macscan_engine::ScanOptions::full(),Some(footprint+footprint/2),&cancel,|_|{}).unwrap();
    assert_eq!(report.failure,Some(machine::FailureKind::Storage));assert!(report.session["volumes"][0]["scan_id"].is_string());assert!(report.session["volumes"][1]["scan_id"].is_null());
    // Natural zero-child cooperative cancellation still publishes a terminal identity.
    cancel.store(true,Ordering::Relaxed);let report=machine::run_configured(&small.store(),plan(&small.0,"full"),&macscan_engine::ScanOptions::full(),None,&cancel,|_|{}).unwrap();assert_eq!(report.session["status"],"cancelled");assert!(report.session["volumes"].as_array().unwrap().iter().all(|r|r["scan_id"].is_null()));
}
fn sigint_after_published_child(){
    let f=Fixture::new();let old=f.run("full",&["scan","--whole-mac","--save"],0);
    // Real work in an owned second volume leaves time to observe the first publication.
    for n in 0..15000{fs::write(f.0.join("b").join(format!("cancel-{n}")),b"owned").unwrap();}
    let mut child=f.cmd("full",&["scan","--whole-mac","--save","--progress","ndjson"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let stderr=child.stderr.take().unwrap();let mut lines=BufReader::new(stderr).lines();let started=Instant::now();let mut saw=false;
    for line in lines.by_ref(){let value:Value=serde_json::from_str(&line.unwrap()).unwrap();if value["saved_members"].as_u64().unwrap_or(0)>=1{assert_eq!(unsafe{libc::kill(child.id() as i32,libc::SIGINT)},0);saw=true;break;}assert!(started.elapsed()<Duration::from_secs(30));}
    assert!(saw);let tail=std::thread::spawn(move||{let mut all=Vec::new();for line in lines{let line=line.unwrap();let _:Value=serde_json::from_str(&line).unwrap();all.extend_from_slice(line.as_bytes());all.push(b'\n');}all});
    let mut out=child.wait_with_output().unwrap();out.stderr=tail.join().unwrap();let result=receipt(&out,130);assert_eq!(result["outcome"],"cancelled");assert_eq!(result["retained"],true);assert!(result["coverage"]["saved_members"].as_u64().unwrap()>=1);assert_eq!(f.saved(&result)["status"],"cancelled");f.saved(&old);
    assert!(!fs::read_dir(f.store()).unwrap().any(|p|p.unwrap().file_name().to_string_lossy().ends_with(".partial")));
}

fn publication_failure_and_stdout_failure(){
    let f=Fixture::new();let mut child=f.cmd("full",&["scan","--whole-mac","--save","--progress","ndjson"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut lines=BufReader::new(child.stderr.take().unwrap()).lines();let first:Value=serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();assert_eq!(first["phase"],"plan");
    let collision=f.store().join(format!("{}.machine.json",first["run_id"].as_str().unwrap()));fs::create_dir(&collision).unwrap();
    let remaining:Vec<_>=lines.map(|l|l.unwrap()).collect();let out=child.wait_with_output().unwrap();assert_eq!(out.status.code(),Some(3));assert!(out.stdout.is_empty());
    let error:Value=serde_json::from_str(remaining.last().unwrap()).unwrap();assert_eq!(error["retained"],true);assert_eq!(error["session_published"],false);assert_eq!(error["coverage"]["saved_members"],2);assert!(error["recovery"]["first_scan_id"].is_string());assert_eq!(error["error"]["operation"],"machine_publication");
    fs::remove_dir(collision).unwrap();assert_eq!(Store::open(&f.store(),false).unwrap().ids().unwrap().len(),2);assert!(!fs::read_dir(f.store()).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    // Broken stdout must retain the just-published group context on stderr.
    let mut child=f.cmd("full",&["scan","--whole-mac","--save"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();drop(child.stdout.take());let out=child.wait_with_output().unwrap();assert_eq!(out.status.code(),Some(3));let error:Value=serde_json::from_slice(&out.stderr).unwrap();assert_eq!(error["error"]["code"],"BROKEN_PIPE");assert_eq!(error["retained"],true);assert_eq!(error["session_published"],true);f.saved(&error);
}
