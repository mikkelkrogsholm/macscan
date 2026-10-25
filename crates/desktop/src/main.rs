use macscan_analysis::{Store, FULL_HISTORY_BUDGET};
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
                scans.push(json!({"scan_id":id,"root":m["scope"]["root_display"],"policy":m["scope"]["policy_fingerprint"],"started_at":m["started_at"],"status":m["status"],"entries":m["entry_count"]}));
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
        let (col,observed)=if parsed==Metric::Logical {("logical_bytes","observed_logical_bytes")}else{("accounted_allocated_bytes","observed_accounted_allocated_bytes")};
        let value=search.replace('\'',"''");
        let sql=format!("SELECT entry_id,name_display,kind,{col} AS bytes,CASE WHEN NOT coverage_complete THEN {observed} END AS lower_bound_bytes,coverage_complete,category FROM entries WHERE parent_id={entry} AND contains(lower(name_display),lower('{value}')) ORDER BY COALESCE({col},{observed}) DESC,entry_id");
        let mut answer=store.query_sql(&scan_id,None,&sql,32,offset,&AtomicBool::new(false)).map_err(err)?;
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
    launch_scan(Some(root),&state)
}
#[tauri::command]
fn start_machine(state: State<'_,Desktop>)->Reply {launch_scan(None,&state)}
#[tauri::command]
async fn machine_session(session_id:String,state:State<'_,Desktop>)->Reply {
    let path=store_path(&state)?;
    blocking(move||machine::load(&path,&session_id).map_err(err)).await
}
fn launch_scan(root:Option<String>,state:&Desktop)->Reply {
    let whole=root.is_none();
    let root=root.unwrap_or_else(||"/".into());
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err("Vælg en absolut lokal sti".into());
    }
    let store = store_path(&state)?;
    let mut slot = state.job.lock().map_err(err)?;
    if let Some(job) = slot.as_ref() {
        let s = job.status.lock().map_err(err)?;
        if ["planning","scanning","saving"].iter().any(|p|s["phase"]==*p) {
            return Err("Der kører allerede en scanning".into());
        }
    }
    let job = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(json!({"phase":if whole{"planning"}else{"scanning"},"whole_mac":whole,"entries_seen":0,"elapsed_ms":0,"root":root})),
    });
    *slot = Some(job.clone());
    std::thread::Builder::new().name("macscan-reader".into()).spawn(move||{
        let result=(||->Result<String,String>{
            if whole {
                let plan=machine::plan().map_err(err)?;
                return machine::run(&store,plan,&job.cancel,|value|{*job.status.lock().unwrap()=value;}).map_err(err);
            }
            Store::open(&store,true).map_err(err)?.scan_to_store(&root,&ScanOptions::full(),HISTORY_BUDGET,&job.cancel,|p|{
                *job.status.lock().unwrap()=json!({"phase":if p.finished {"saving"}else{"scanning"},"entries_seen":p.entries_seen,"elapsed_ms":p.elapsed_ms,"root":root,"branches":p.branches});
            }).map_err(err)
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
            compare_scans,
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
