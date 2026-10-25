use macscan_analysis::Store;
use macscan_engine::{
    scan_with_progress, volumes, CompareQuery, Metric, Query, QueryKind, ScanOptions,
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

const HISTORY_BUDGET: u64 = 512 * 1024 * 1024;
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
                scans.push(json!({"scan_id":id,"root":m["scope"]["root_display"],"started_at":m["started_at"],"status":m["status"],"entries":m["entry_count"]}));
            }
        }
    }
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
        result["volumes"] = volumes(20, None).map_err(err)?;
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
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err("Vælg en absolut lokal sti".into());
    }
    let store = store_path(&state)?;
    let mut slot = state.job.lock().map_err(err)?;
    if let Some(job) = slot.as_ref() {
        let s = job.status.lock().map_err(err)?;
        if s["phase"] == "scanning" || s["phase"] == "saving" {
            return Err("Der kører allerede en scanning".into());
        }
    }
    let job = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(json!({"phase":"scanning","entries_seen":0,"elapsed_ms":0,"root":root})),
    });
    *slot = Some(job.clone());
    std::thread::Builder::new().name("macscan-reader".into()).spawn(move||{
        let result=(||->Result<String,String>{
            let mut options=ScanOptions::default();options.exclusions.push(store.clone());
            let scan=scan_with_progress(&root,&options,&job.cancel,|p|{
                *job.status.lock().unwrap()=json!({"phase":"scanning","entries_seen":p.entries_seen,"elapsed_ms":p.elapsed_ms,"root":root});
            }).map_err(err)?;
            if job.cancel.load(Ordering::Relaxed) {return Err("cancelled".into());}
            job.status.lock().unwrap()["phase"]=json!("saving");
            Store::open(&store,true).map_err(err)?.save_with_cancel(&scan,HISTORY_BUDGET,&job.cancel).map_err(err)?;
            Ok(scan.id)
        })();
        let mut status=job.status.lock().unwrap();
        match result {Ok(id)=>{status["phase"]=json!("done");status["scan_id"]=json!(id);},Err(e)=>{
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
    if !["copy", "reveal"].contains(&action.as_str()) {
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
        if action == "copy" {
            let mut child = Command::new("/usr/bin/pbcopy")
                .stdin(Stdio::piped())
                .spawn()
                .map_err(err)?;
            child
                .stdin
                .take()
                .ok_or("clipboard stdin")?
                .write_all(&raw)
                .map_err(err)?;
            if !child.wait().map_err(err)?.success() {
                return Err("Kunne ikke kopiere stien".into());
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
            choose_history,
            restore_history,
            view_scan,
            list_children,
            compare_scans,
            start_scan,
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
