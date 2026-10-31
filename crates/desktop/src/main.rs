use macscan_analysis::{FileProfileQuery, GrowthGrouping, GrowthQuery, InsightsQuery, Store, FULL_HISTORY_BUDGET};
use macscan_engine::{
    CompareQuery, Metric, Query, QueryKind, ScanOptions,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Manager, State};
mod machine;

const HISTORY_BUDGET: u64 = FULL_HISTORY_BUDGET;
type Reply = Result<Value, String>;
struct Job {
    cancel: AtomicBool,
    status: Mutex<Value>,
}
struct Desktop {
    store: Mutex<PathBuf>,
    explicit_store: bool,
    job: Mutex<Option<Arc<Job>>>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn store_path(state: &Desktop) -> Result<PathBuf, String> {
    Ok(state.store.lock().map_err(err)?.clone())
}
fn history(path: &std::path::Path) -> Reply {
    let mut scans = Vec::new();
    if path.exists() {
        let store = Store::open(path, false).map_err(err)?;
        for id in store.ids().map_err(err)?.into_iter().rev().take(100) {
            if let Ok(m) = store.manifest(&id) {
                scans.push(json!({"scan_id":id,"root":m["scope"]["root_display"],"apfs_data_clone":m["apfs_data_clone"],"policy":m["scope"]["policy_fingerprint"],"root_path_bytes_hex":m["scope"]["root_path_bytes_hex"],"context":m["context"],"origin":m["origin"],"started_at":m["started_at"],"status":m["status"],"entries":m["entry_count"]}));
            }
        }
    }
    scans.extend(machine::history(path));
    scans.sort_by(|a,b|b["sort_at"].as_str().or(b["started_at"].as_str()).cmp(&a["sort_at"].as_str().or(a["started_at"].as_str())));
    scans.truncate(100);
    Ok(json!({"store":path,"scans":scans,"metadata_budget_bytes":HISTORY_BUDGET}))
}
async fn blocking(f: impl FnOnce() -> Reply + Send + 'static) -> Reply {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}
#[tauri::command]
async fn bootstrap(state: State<'_, Desktop>) -> Reply {
    let path = store_path(&state)?;
    let explicit = state.explicit_store;
    blocking(move || {
        let mut result = history(&path)?;
        result["explicit_store"] = json!(explicit);
        result["volumes"] = json!({"records":machine::mounts().map_err(err)?});
        Ok(result)
    })
    .await
}
#[tauri::command]
async fn choose_root() -> Result<Option<String>, String> {
    Ok(rfd::AsyncFileDialog::new()
        .set_title("Vælg mappe til read-only scanning")
        .pick_folder()
        .await
        .map(|p| p.path().to_string_lossy().into_owned()))
}
#[tauri::command]
fn open_privacy()->Result<(),String>{
    if !Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles").status().map_err(err)?.success(){return Err("Åbn selv Systemindstillinger → Anonymitet & sikkerhed → Fuld diskadgang".into());}
    Ok(())
}
#[tauri::command]
async fn choose_history(state: State<'_, Desktop>) -> Reply {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Vælg mappe med Macscan-målinger")
        .pick_folder()
        .await;
    match picked {
        None => Ok(Value::Null),
        Some(p) => {
            let path = p.path().to_path_buf();
            Store::open(&path, false).map_err(err)?;
            *state.store.lock().map_err(err)? = path.clone();
            blocking(move || history(&path)).await
        }
    }
}
#[tauri::command]
async fn restore_history(path: String, state: State<'_, Desktop>) -> Reply {
    let path = PathBuf::from(path);
    Store::open(&path, false).map_err(err)?;
    *state.store.lock().map_err(err)? = path.clone();
    blocking(move || history(&path)).await
}
#[tauri::command]
async fn view_scan(
    scan_id: String,
    entry: u32,
    metric: String,
    state: State<'_, Desktop>,
) -> Reply {
    let path = store_path(&state)?;
    blocking(move || {
        let store = Store::open(&path, false).map_err(err)?;
        let metric = Metric::parse(&metric).map_err(err)?;
        let cancel = AtomicBool::new(false);
        let mut view = store
            .sunburst(&scan_id, entry, metric, &cancel)
            .map_err(err)?;
        view["children"] = store
            .report(
                &scan_id,
                &Query {
                    entry,
                    metric,
                    limit: 32,
                    ..Query::default()
                },
                &cancel,
            )
            .map_err(err)?;
        Ok(view)
    })
    .await
}
#[tauri::command]
async fn insights(
    scan_id: String,
    entry: u32,
    metric: String,
    category: Option<String>,
    cursor: Option<String>,
    state: State<'_, Desktop>,
) -> Reply {
    let path = store_path(&state)?;
    blocking(move || {
        Store::open(&path, false).map_err(err)?.insights(
            &scan_id,
            &InsightsQuery {
                entry,
                metric: Metric::parse(&metric).map_err(err)?,
                category,
                cursor,
                ..InsightsQuery::default()
            },
            &AtomicBool::new(false),
        ).map_err(err)
    }).await
}
// The native dialog shares the immutable, bounded Store report with the CLI.
fn coverage_at(path: &std::path::Path, scan_id: &str, reason: Option<&str>, cursor: Option<&str>) -> Reply {
    Store::open(path, false).map_err(err)?.coverage(scan_id, reason, cursor, 10, &AtomicBool::new(false)).map_err(err)
}
#[tauri::command]
async fn coverage(scan_id: String, reason: Option<String>, cursor: Option<String>, state: State<'_, Desktop>) -> Reply {
    let path = store_path(&state)?;
    blocking(move || coverage_at(&path, &scan_id, reason.as_deref(), cursor.as_deref())).await
}
// Keep native growth requests on the same saved-data contract as the CLI.
fn growth_at(path: &std::path::Path, before: &str, after: &str, query: &GrowthQuery) -> Reply {
    Store::open(path, false).map_err(err)?.growth(before, after, query, &AtomicBool::new(false)).map_err(err)
}
#[tauri::command]
async fn growth(
    before: String, after: String, entry: u32, metric: String,
    grouping: String, files: bool, group: Option<String>, change: String,
    cursor: Option<String>, state: State<'_, Desktop>,
) -> Reply {
    let path = store_path(&state)?;
    blocking(move || {
        let grouping = match grouping.as_str() {
            "area" => GrowthGrouping::Area,
            "category" => GrowthGrouping::Category,
            _ => return Err("Ukendt gruppering".into()),
        };
        growth_at(&path, &before, &after, &GrowthQuery {
            entry, metric: Metric::parse(&metric).map_err(err)?, grouping,
            files, group, change, cursor, limit: 10,
        })
    }).await
}
// Decimal strings keep UI thresholds exact across the JavaScript boundary.
fn profile_integer(value: Option<String>) -> Result<Option<u64>, String> {
    value.map(|s| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Filter skal være et ikke-negativt heltal i decimalformat".into());
        }
        s.parse::<u64>().map_err(|_| "Filter er større end u64".into())
    }).transpose()
}
fn file_profile_at(path: &std::path::Path, scan_id: &str, query: &FileProfileQuery) -> Reply {
    Store::open(path, false).map_err(err)?.file_profile(scan_id, query, &AtomicBool::new(false)).map_err(err)
}
#[tauri::command]
async fn file_profile(
    scan_id: String, entry: u32, metric: String, min_age_days: Option<String>,
    min_bytes: Option<String>, type_key: Option<String>, types: bool,
    sort_count: bool, cursor: Option<String>, state: State<'_, Desktop>,
) -> Reply {
    let path = store_path(&state)?;
    blocking(move || file_profile_at(&path, &scan_id, &FileProfileQuery {
        entry, metric: Metric::parse(&metric).map_err(err)?,
        min_age_days: profile_integer(min_age_days)?, min_bytes: profile_integer(min_bytes)?,
        type_key, types, sort_count, cursor, limit: 10,
    })).await
}
#[tauri::command]
async fn list_children(
    scan_id: String,
    entry: u32,
    metric: String,
    cursor: Option<String>,
    search: String,
    state: State<'_, Desktop>,
) -> Reply {
    if search.len() > 1024 {
        return Err("Søgningen er for lang".into());
    }
    let path = store_path(&state)?;
    blocking(move||{
        let store=Store::open(&path,false).map_err(err)?;
        let parsed=Metric::parse(&metric).map_err(err)?;
        if search.is_empty() {
            return store.report(&scan_id,&Query{entry,metric:parsed,cursor,limit:32,..Query::default()},&AtomicBool::new(false)).map_err(err);
        }
        let key=search.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>();
        let binding=format!("ui-search:{scan_id}:{entry}:{metric}:{key}:");
        let offset=match cursor {None=>0,Some(c)=>c.strip_prefix(&binding).ok_or("Ugyldig søgeside")?.parse::<usize>().map_err(err)?};
        let (col,observed)=match parsed {Metric::Logical=>("logical_bytes","observed_logical_bytes"),Metric::ApfsDataClone=>("apfs_data_clone_bytes","observed_apfs_data_clone_bytes"),Metric::Allocated=>("accounted_allocated_bytes","observed_accounted_allocated_bytes"),Metric::Private=>return Err("private metric unavailable".into())};
        if parsed==Metric::ApfsDataClone { store.report(&scan_id,&Query{metric:parsed,kind:macscan_engine::QueryKind::Summary,..Query::default()},&AtomicBool::new(false)).map_err(err)?; }
        let value=search.replace('\'',"''");
        let sql=format!("SELECT entry_id,name_display,kind,{col} AS bytes,CASE WHEN NOT coverage_complete THEN {observed} END AS lower_bound_bytes,coverage_complete,category,category_rule_id,category_rule_version,category_reason,category_coverage,category_evidence_source FROM entries_classified((SELECT list(entry_id) FROM (SELECT entry_id FROM entries_saved WHERE parent_id={entry} AND contains(lower(name_display),lower('{value}')) ORDER BY COALESCE({col},{observed}) DESC,entry_id LIMIT 33 OFFSET {offset}))) ORDER BY COALESCE({col},{observed}) DESC,entry_id");
        let mut answer=store.query_sql(&scan_id,None,&sql,32,0,&AtomicBool::new(false)).map_err(err)?;
        answer["offset"]=json!(offset);
        if let Some(next)=answer["next_offset"].as_u64(){answer["next_offset"]=json!(offset.saturating_add(next as usize));}
        answer["next_cursor"]=answer["next_offset"].as_u64().map(|n|json!(format!("{binding}{n}"))).unwrap_or(Value::Null);
        Ok(answer)
    }).await
}
#[tauri::command]
async fn compare_scans(
    before: String,
    after: String,
    metric: String,
    cursor: Option<String>,
    state: State<'_, Desktop>,
) -> Reply {
    let path = store_path(&state)?;
    blocking(move || {
        Store::open(&path, false)
            .map_err(err)?
            .compare(
                &before,
                &after,
                &CompareQuery {
                    metric: Metric::parse(&metric).map_err(err)?,
                    limit: 20,
                    cursor,
                },
                &AtomicBool::new(false),
            )
            .map_err(err)
    })
    .await
}
#[tauri::command]
fn start_scan(root: String, state: State<'_, Desktop>) -> Reply {
    launch_scan(Some(root),None,&state)
}
#[tauri::command]
fn start_machine(state: State<'_,Desktop>)->Reply {launch_scan(None,None,&state)}
#[tauri::command]
async fn machine_session(session_id:String,state:State<'_,Desktop>)->Reply {
    let path=store_path(&state)?;
    blocking(move||machine::load(&path,&session_id).map_err(err)).await
}
#[tauri::command]
async fn prepare_rescan(scan_id:String,entry:u32,state:State<'_,Desktop>)->Reply {
    let path=store_path(&state)?;
    blocking(move||Store::open(&path,false).map_err(err)?.prepare_rescan(&scan_id,entry).map_err(err)).await
}
#[tauri::command]
async fn start_rescan(scan_id:String,entry:u32,state:State<'_,Desktop>)->Reply {
    let path=store_path(&state)?;
    let source_id=scan_id.clone();
    let prepared=blocking(move||Store::open(&path,false).map_err(err)?.prepare_rescan(&source_id,entry).map_err(err)).await?;
    launch_scan(None,Some((scan_id,entry,prepared)),&state)
}
fn launch_scan(root:Option<String>,source:Option<(String,u32,Value)>,state:&Desktop)->Reply {
    let whole=root.is_none()&&source.is_none();
    let root=root.unwrap_or_else(||"/".into());
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err("Vælg en absolut lokal sti".into());
    }
    let store = store_path(&state)?;
    let prepared=source.as_ref().map(|(_,_,prepared)|prepared);
    let root_display=prepared.map(|p|p["root_display"].clone()).unwrap_or_else(||json!(root.to_string_lossy()));
    let root_hex=prepared.map(|p|p["root_path_bytes_hex"].clone()).unwrap_or(Value::Null);
    let mut slot = state.job.lock().map_err(err)?;
    if let Some(job) = slot.as_ref() {
        let s = job.status.lock().map_err(err)?;
        if ["planning","scanning","saving"].iter().any(|p|s["phase"]==*p) {
            return Err("Der kører allerede en scanning".into());
        }
    }
    let job = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(json!({"phase":if whole{"planning"}else{"scanning"},"whole_mac":whole,"entries_seen":0,"elapsed_ms":0,"root":root_display,"root_path_bytes_hex":root_hex})),
    });
    *slot = Some(job.clone());
    std::thread::Builder::new().name("macscan-reader".into()).spawn(move||{
        let result=(||->Result<String,String>{
            if whole {
                let plan=machine::plan().map_err(err)?;
                return machine::run(&store,plan,&job.cancel,|value|{*job.status.lock().unwrap()=value;}).map_err(err);
            }
            let store=Store::open(&store,true).map_err(err)?;
            let progress=|p:&macscan_engine::Progress|{
                *job.status.lock().unwrap()=json!({"phase":if p.finished {"saving"}else{"scanning"},"entries_seen":p.entries_seen,"elapsed_ms":p.elapsed_ms,"root":root_display,"root_path_bytes_hex":root_hex,"branches":p.branches});
            };
            if let Some((id,entry,_))=source {
                store.rescan_to_store(&id,entry,&ScanOptions::full(),HISTORY_BUDGET,&job.cancel,progress).map_err(err)
            } else {
                store.scan_to_store(&root,&ScanOptions::full(),HISTORY_BUDGET,&job.cancel,progress).map_err(err)
            }
        })();
        let mut status=job.status.lock().unwrap();
        match result {Ok(id)=>{status["phase"]=json!("done");status["scan_id"]=json!(id);status["whole_mac"]=json!(whole);},Err(e)=>{
            status["phase"]=json!(if job.cancel.load(Ordering::Relaxed){"cancelled"}else{"error"});status["error"]=json!(e);
        }}
    }).map_err(err)?;
    Ok(json!({"started":true}))
}
#[tauri::command]
fn job_status(state: State<'_, Desktop>) -> Reply {
    Ok(match state.job.lock().map_err(err)?.as_ref() {
        Some(job) => job.status.lock().map_err(err)?.clone(),
        None => json!({"phase":"idle"}),
    })
}
#[tauri::command]
fn stop_scan(state: State<'_, Desktop>) -> Result<(), String> {
    if let Some(job) = state.job.lock().map_err(err)?.as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}
#[tauri::command]
async fn file_action(
    scan_id: String,
    entry: u32,
    action: String,
    state: State<'_, Desktop>,
) -> Reply {
    if !["copy", "copy_info", "reveal"].contains(&action.as_str()) {
        return Err("Ukendt handling".into());
    }
    let path = store_path(&state)?;
    blocking(move || {
        let result = Store::open(&path, false)
            .map_err(err)?
            .report(
                &scan_id,
                &Query {
                    entry,
                    kind: QueryKind::Inspect,
                    ..Query::default()
                },
                &AtomicBool::new(false),
            )
            .map_err(err)?;
        let hex = result["records"][0]["path_bytes_hex"]
            .as_str()
            .ok_or("Mangler sti")?;
        let raw = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        if action == "copy" || action == "copy_info" {
            let copied = if action == "copy_info" {
                information_to_copy(&result)?
            } else {
                raw
            };
            let mut child = Command::new("/usr/bin/pbcopy")
                .stdin(Stdio::piped())
                .spawn()
                .map_err(err)?;
            child
                .stdin
                .take()
                .ok_or("clipboard stdin")?
                .write_all(&copied)
                .map_err(err)?;
            if !child.wait().map_err(err)?.success() {
                return Err("Kunne ikke kopiere til udklipsholderen".into());
            }
        } else {
            use std::os::unix::ffi::OsStringExt;
            let actual = PathBuf::from(std::ffi::OsString::from_vec(raw));
            actual
                .symlink_metadata()
                .map_err(|_| "Filen er flyttet eller findes ikke længere".to_owned())?;
            if !Command::new("/usr/bin/open")
                .arg("-R")
                .arg(actual)
                .status()
                .map_err(err)?
                .success()
            {
                return Err("Finder kunne ikke åbnes".into());
            }
        }
        Ok(json!({"ok":true}))
    })
    .await
}
fn information_to_copy(result: &Value) -> Result<Vec<u8>, String> {
    let entry = result["records"]
        .get(0)
        .filter(|v| v.is_object())
        .ok_or("Mangler information om posten")?;
    serde_json::to_vec_pretty(&json!({
        "app":"Macscan",
        "source":"saved_scan_not_live",
        "scan_id":result["scan_id"],
        "measured_at":result["started_at"],
        "entry":entry,
        "note":"Størrelser er bytes. Null betyder ukendt. Allokeret plads er ikke nødvendigvis frigørbar APFS-plads."
    })).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_preparation_never_starts_a_job_and_active_guard_is_authoritative() {
        use macscan_engine::scan;
        let base=std::env::temp_dir().canonicalize().unwrap().join(format!("macscan-native-focused-{}",std::process::id()));
        std::fs::create_dir(&base).unwrap();let source=base.join("source");std::fs::create_dir(&source).unwrap();std::fs::write(source.join("ordinary"),b"synthetic").unwrap();
        let path=base.join("history");let store=Store::open(&path,true).unwrap();
        let mut saved=scan(&source,&ScanOptions::default(),&AtomicBool::new(false)).unwrap();
        use std::os::unix::ffi::OsStringExt;
        saved.root=base.join(std::ffi::OsString::from_vec(b"missing-\xff".to_vec()));
        store.save_with_cancel(&saved,HISTORY_BUDGET,&AtomicBool::new(false)).unwrap();
        let desktop=Desktop{store:Mutex::new(path),explicit_store:true,job:Mutex::new(None)};
        let prepared=store.prepare_rescan(&saved.id,0).unwrap();assert!(prepared["root_path_bytes_hex"].as_str().unwrap().ends_with("FF"));
        assert!(desktop.job.lock().unwrap().is_none()); // prepare then cancel is no job.
        *desktop.job.lock().unwrap()=Some(Arc::new(Job{cancel:AtomicBool::new(false),status:Mutex::new(json!({"phase":"saving"}))}));
        assert!(launch_scan(None,Some((saved.id.clone(),0,prepared.clone())),&desktop).unwrap_err().contains("allerede"));
        *desktop.job.lock().unwrap()=None;
        launch_scan(None,Some((saved.id.clone(),0,prepared.clone())),&desktop).unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        loop {let status=desktop.job.lock().unwrap().as_ref().unwrap().status.lock().unwrap().clone();
            assert!(status["root"].is_string());assert_eq!(status["root_path_bytes_hex"],prepared["root_path_bytes_hex"]);
            if status["phase"]=="error"{break;}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(store.ids().unwrap(),vec![saved.id]);std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn native_growth_matches_shared_store_for_synthetic_saved_history() {
        use macscan_engine::{Entry, EntryKind, Scan, ScanStatus, STORE_BUDGET};
        let path = std::env::temp_dir().join(format!("macscan-desktop-growth-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let store = Store::open(&path, true).unwrap();
        let make_entry = |parent, name: &[u8], kind, size| Entry {
            clone_metadata: macscan_engine::CloneMetadata::default(),
            parent, name: name.to_vec(), kind, logical:size, allocated:size, accounted:size,
            own_allocated:size, device:1, inode:1, links:1, observed_links:1, charged_to:None,
            mtime:0, complete:true, enumeration_complete:true,
        };
        for n in 1..=2 {
            let scan = Scan {
                id:format!("600-{n}"), root:PathBuf::from("/synthetic-not-accessed"),
                entries:vec![make_entry(None,b"",EntryKind::Directory,0),
                    make_entry(Some(0),b"project",EntryKind::Directory,0),
                    make_entry(Some(1),b"file",EntryKind::File,if n==1 {10} else {7})],
                issues:vec![], issue_count:0, excluded_count:0, status:ScanStatus::Complete,
                started_at:format!("2026-10-2{n}T10:00:00Z"), ended_at:format!("2026-10-2{n}T10:00:00Z"),
                duration_ms:0, policy:"synthetic".into(),
                context:json!({"os_identity":"synthetic","volume_before":{"volume_uuid":"synthetic"},"volume_after":{"volume_uuid":"synthetic"},"started_unix_ns":(1_792_490_400_000_000_000u128+86_400_000_000_000u128*n).to_string(),"ended_unix_ns":(1_792_490_400_000_000_000u128+86_400_000_000_000u128*n).to_string()}),
            };
            store.save_with_cancel(&scan, STORE_BUDGET, &AtomicBool::new(false)).unwrap();
        }
        let native_coverage = coverage_at(&path, "600-2", None, None).unwrap();
        assert_eq!(native_coverage, store.coverage("600-2", None, None, 10, &AtomicBool::new(false)).unwrap());
        assert_eq!(native_coverage["quality"], "legacy_samples_only");
        assert!(native_coverage["reason_counts"].is_null());
        assert!(coverage_at(&path, "600-2", Some("invented"), None).is_err());
        for types in [false, true] {
            for sort_count in [false, true] {
                if sort_count && !types { continue; }
                let q = FileProfileQuery {entry:1,metric:Metric::Logical,types,sort_count,
                    min_bytes:profile_integer(Some("7".into())).unwrap(),
                    min_age_days:profile_integer(Some("180".into())).unwrap(),..Default::default()};
                let native = file_profile_at(&path,"600-2",&q).unwrap();
                assert_eq!(native,store.file_profile("600-2",&q,&AtomicBool::new(false)).unwrap());
                assert_eq!(native["totals"]["bytes"],"7");
                assert_eq!(native["totals"]["file_path_count"],"1");
                assert_eq!(native["records"][0]["type_key"],"none");
            }
        }
        assert_eq!(profile_integer(Some(u64::MAX.to_string())).unwrap(),Some(u64::MAX));
        for invalid in ["18446744073709551616", "-1", "+1", "1.5", "", " 1"] {
            assert!(profile_integer(Some(invalid.into())).is_err());
        }
        for grouping in [GrowthGrouping::Area, GrowthGrouping::Category] {
            for files in [false,true] {
                let q = GrowthQuery {entry:1,metric:Metric::Logical,grouping,files,limit:10,..Default::default()};
                let native = growth_at(&path,"600-1","600-2",&q).unwrap();
                assert_eq!(native,store.growth("600-1","600-2",&q,&AtomicBool::new(false)).unwrap());
                assert_eq!(native["contributions"]["delta_bytes"],"-3");
                assert_eq!(native["totals"]["before"]["bytes"],"10");
                assert_eq!(native["totals"]["after"]["bytes"],"7");
            }
        }
        let q = GrowthQuery::default();
        assert!(growth_at(&path,"600-2","600-1",&q).is_err());
        let invalid = GrowthQuery {change:"invented".into(),..Default::default()};
        assert!(growth_at(&path,"600-1","600-2",&invalid).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn copied_information_preserves_exact_sizes_paths_and_measurement_context() {
        let result = json!({"scan_id":"scan-a","started_at":"2026-10-25T12:00:00Z","records":[{
            "path_display":"/example/a\n\"quoted\".mov","kind":"file",
            "logical_bytes":"18446744073709551615","accounted_allocated_bytes":null,
            "coverage_complete":false
        }]});
        let copy: Value = serde_json::from_slice(&information_to_copy(&result).unwrap()).unwrap();
        assert_eq!(copy["entry"], result["records"][0]);
        assert_eq!(copy["scan_id"], "scan-a");
        assert_eq!(copy["measured_at"], result["started_at"]);
        assert_eq!(copy["source"], "saved_scan_not_live");
    }

    #[test]
    fn missing_copy_information_is_an_error_not_an_empty_success() {
        assert!(information_to_copy(&json!({"records":[]})).is_err());
    }
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let explicit = args
        .windows(2)
        .find(|a| a[0] == "--store")
        .map(|a| PathBuf::from(&a[1]));
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let state = Desktop {
        explicit_store: explicit.is_some(),
        store: Mutex::new(
            explicit.unwrap_or_else(|| home.join("Library/Application Support/Macscan")),
        ),
        job: Mutex::new(None),
    };
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            choose_root,
            open_privacy,
            choose_history,
            restore_history,
            view_scan,
            list_children,
            insights,
            growth,
            coverage,
            file_profile,
            compare_scans,
            prepare_rescan,
            start_rescan,
            start_scan,
            start_machine,
            machine_session,
            job_status,
            stop_scan,
            file_action
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                if let Some(job) = window.state::<Desktop>().job.lock().unwrap().as_ref() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Macscan desktop");
}
