//! Owned fixtures only. The ignored test explicitly exercises native clonefile.
use macscan_analysis::{Store, FULL_HISTORY_BUDGET};
use macscan_engine::{CompareQuery, Metric, ScanOptions};
use serde_json::{json, Value};
use std::{
    ffi::CString,
    fs,
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::FileExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let mut template = std::env::temp_dir()
            .join("macscan-capture-XXXXXX")
            .as_os_str()
            .as_bytes()
            .to_vec();
        template.push(0);
        assert!(!unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null());
        template.pop();
        Self(
            PathBuf::from(std::ffi::OsStr::from_bytes(&template))
                .canonicalize()
                .unwrap(),
        )
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
        eprintln!("owned capture fixture cleaned");
    }
}
fn rows(store: &Store, id: &str, fields: &str) -> Value {
    store
        .query_sql(
            id,
            None,
            &format!("SELECT {fields} FROM entries ORDER BY path_bytes"),
            100,
            0,
            &STOP,
        )
        .unwrap()["records"]
        .clone()
}
fn capture(store: &Store, root: &Path, on: bool) -> String {
    let options = ScanOptions {
        capture_clones: on,
        ..ScanOptions::full()
    };
    store
        .scan_to_store(root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {})
        .unwrap()
}
fn primary(store: &Store, id: &str) -> Value {
    rows(store,id,"path_display,kind,logical_bytes,allocated_bytes,accounted_allocated_bytes,own_allocated_bytes,device_id,inode,link_count,observed_link_count,charged_to,coverage_complete,enumeration_complete,category")
}
#[test]
fn capture_is_opt_in_and_primary_policy_and_publication_are_unchanged() {
    let tree = Tree::new();
    let history = Tree::new();
    fs::write(tree.0.join("a"), b"own data").unwrap();
    fs::hard_link(tree.0.join("a"), tree.0.join("b")).unwrap();
    let store = Store::open(&history.0.join("history"), true).unwrap();
    let off = capture(&store, &tree.0, false);
    let on = capture(&store, &tree.0, true);
    assert_eq!(primary(&store, &off), primary(&store, &on));
    assert_eq!(
        store.manifest(&off).unwrap()["scope"],
        store.manifest(&on).unwrap()["scope"]
    );
    let context = store.manifest(&on).unwrap()["context"]["clone_capture"].clone();
    assert_eq!(context["version"], 1);
    assert_eq!(context["requested"], true);
    assert_eq!(context["capability_status"], "not_queried");
    let off_context=store.manifest(&off).unwrap()["context"]["clone_capture"].clone();
    assert_eq!(off_context["counts_collected"],false);
    assert_eq!(off_context["regular_file_status_counts"],json!({}));
    assert_eq!(context["counts_collected"],true);
    for row in rows(
        &store,
        &off,
        "clone_status,clone_id,clone_extended_flags,clone_data_allocated_bytes,clone_device_id",
    )
    .as_array()
    .unwrap()
    {
        assert_eq!(row["clone_status"], "not_requested");
        assert!(row["clone_id"].is_null());
    }
    let compared = store
        .compare(
            &off,
            &on,
            &CompareQuery {
                metric: Metric::Allocated,
                limit: 100,
                cursor: None,
            },
            &STOP,
        )
        .unwrap();
    assert!(compared["records"].as_array().unwrap().is_empty());
    assert!(macscan_engine::scan(
        &tree.0,
        &ScanOptions {
            capture_clones: true,
            ..ScanOptions::default()
        },
        &STOP
    )
    .is_err());
    let before = fs::read_dir(store.path()).unwrap().count();
    let during = AtomicBool::new(false);
    assert!(store
        .scan_to_store(
            &tree.0,
            &ScanOptions {
                capture_clones: true,
                ..ScanOptions::full()
            },
            FULL_HISTORY_BUDGET,
            &during,
            |_| {
                during.store(true, Ordering::Relaxed);
            }
        )
        .is_err());
    assert!(store
        .scan_to_store(
            &tree.0,
            &ScanOptions {
                capture_clones: true,
                ..ScanOptions::full()
            },
            FULL_HISTORY_BUDGET,
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err());
    assert!(store
        .scan_to_store(
            &tree.0,
            &ScanOptions {
                capture_clones: true,
                ..ScanOptions::full()
            },
            1,
            &STOP,
            |_| {}
        )
        .is_err());
    // Enough budget to create a stage, too little to publish: stage must unwind.
    let used: u64 = fs::read_dir(store.path())
        .unwrap()
        .map(|d| {
            fs::read_dir(d.unwrap().path())
                .unwrap()
                .map(|p| {
                    let p = p.unwrap().path();
                    if p.is_dir() {
                        fs::read_dir(p)
                            .unwrap()
                            .map(|f| f.unwrap().metadata().unwrap().len())
                            .sum()
                    } else {
                        p.metadata().unwrap().len()
                    }
                })
                .sum::<u64>()
        })
        .sum();
    assert!(store
        .scan_to_store(
            &tree.0,
            &ScanOptions {
                capture_clones: true,
                ..ScanOptions::full()
            },
            used + 1,
            &STOP,
            |_| {}
        )
        .is_err());
    assert_eq!(fs::read_dir(store.path()).unwrap().count(), before);
    assert!(!fs::read_dir(store.path()).unwrap().any(|p| p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
}
#[test]
fn unsupported_initial_bulk_falls_back_without_dropping_or_duplicating_primary_rows() {
    let tree = Tree::new();
    let history = Tree::new();
    for name in ["a", "b", "c"] {
        fs::write(tree.0.join(name), b"own").unwrap();
    }
    let store = Store::open(&history.0.join("history"), true).unwrap();
    let off = capture(&store, &tree.0, false);
    let (on, p) = store
        .scan_to_store_profiled(
            &tree.0,
            &ScanOptions {
                capture_clones: true,
                ..ScanOptions::full()
            },
            FULL_HISTORY_BUDGET,
            &STOP,
            |_| {},
            true,
        )
        .unwrap();
    assert_eq!(primary(&store, &off), primary(&store, &on));
    assert!(p.walk.fstatat.calls >= 3);
    for r in rows(
        &store,
        &on,
        "kind,clone_status,clone_id,clone_data_allocated_bytes",
    )
    .as_array()
    .unwrap()
    .iter()
    .filter(|r| r["kind"] == "file")
    {
        assert_eq!(r["clone_status"], "unsupported");
        assert!(r["clone_id"].is_null());
    }
    assert_eq!(
        store.manifest(&on).unwrap()["context"]["clone_capture"]["regular_file_status_counts"]
            ["unsupported"],
        3
    );
}
#[test]
#[ignore = "Explicit native clonefile + Parquet production capture on owned fixture"]
fn native_owned_apfs_clone_capture_roundtrip() {
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let tree = Tree::new();
    let history = Tree::new();
    let root = &tree.0;
    eprintln!("native owned capture fixture ready");
    let mut source = fs::File::create(root.join("source")).unwrap();
    source.write_all(&vec![0x5a; 1024 * 1024]).unwrap();
    source.sync_all().unwrap();
    for name in ["full", "forked", "partial"] {
        let a = CString::new(root.join("source").as_os_str().as_bytes()).unwrap();
        let b = CString::new(root.join(name).as_os_str().as_bytes()).unwrap();
        if unsafe { libc::clonefile(a.as_ptr(), b.as_ptr(), 0) } != 0 {
            let e = std::io::Error::last_os_error();
            if [Some(libc::ENOTSUP), Some(libc::ENOSYS)].contains(&e.raw_os_error()) {
                eprintln!("native fixture clonefile observed unsupported: {e}");
                return;
            }
            panic!("native clonefile: {e}");
        }
    }
    fs::hard_link(root.join("source"), root.join("hardlink")).unwrap();
    let partial = fs::OpenOptions::new()
        .write(true)
        .open(root.join("partial"))
        .unwrap();
    partial.write_all_at(&[0x31; 4096], 0).unwrap();
    partial.sync_all().unwrap();
    let mut fork = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(root.join("forked/..namedfork/rsrc"))
        .unwrap();
    fork.write_all(&vec![0x73; 65536]).unwrap();
    fork.sync_all().unwrap();
    let sparse = fs::File::create(root.join("sparse")).unwrap();
    sparse.set_len(16 * 1024 * 1024).unwrap();
    sparse
        .write_all_at(&[0x22; 4096], 16 * 1024 * 1024 - 4096)
        .unwrap();
    sparse.sync_all().unwrap();
    let hole = libc::fpunchhole_t {
        fp_flags: 0,
        reserved: 0,
        fp_offset: 0,
        fp_length: 16 * 1024 * 1024 - 4096,
    };
    assert_eq!(
        unsafe { libc::fcntl(sparse.as_raw_fd(), libc::F_PUNCHHOLE, &hole) },
        0
    );
    sparse.sync_all().unwrap();
    let store = Store::open(&history.0.join("history"), true).unwrap();
    let off = capture(&store, root, false);
    let on = capture(&store, root, true);
    assert_eq!(primary(&store, &off), primary(&store, &on));
    let saved=rows(&store,&on,"name_display,kind,logical_bytes,own_allocated_bytes,clone_status,clone_id,clone_extended_flags,clone_data_allocated_bytes,clone_device_id,device_id,inode,charged_to");
    let files: Vec<_> = saved
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "file")
        .collect();
    assert_eq!(files.len(), 6);
    if files.iter().all(|r| r["clone_status"] == "unsupported") {
        eprintln!("production extended metadata observed unsupported");
        return;
    }
    for r in &files {
        assert_eq!(r["clone_status"], "observed");
        assert_eq!(r["clone_device_id"], r["device_id"]);
    }
    let get = |name: &str| *files.iter().find(|r| r["name_display"] == name).unwrap();
    let src = get("source");
    for name in ["full", "forked", "hardlink"] {
        assert_eq!(get(name)["clone_id"], src["clone_id"]);
        assert_eq!(
            get(name)["clone_extended_flags"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                & 64,
            64
        );
    }
    assert_ne!(get("partial")["clone_id"], src["clone_id"]);
    assert_eq!(
        get("partial")["clone_extended_flags"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            & 64,
        0
    );
    assert_eq!(get("hardlink")["inode"], src["inode"]);
    assert_ne!(get("full")["inode"], src["inode"]);
    assert_eq!(
        get("forked")["clone_data_allocated_bytes"],
        src["clone_data_allocated_bytes"]
    );
    let number = |r: &Value, k: &str| r[k].as_str().unwrap().parse::<u64>().unwrap();
    assert_eq!(
        number(get("forked"), "own_allocated_bytes")
            - number(get("forked"), "clone_data_allocated_bytes"),
        65536
    );
    assert_eq!(number(get("sparse"), "logical_bytes"), 16 * 1024 * 1024);
    assert_eq!(number(get("sparse"), "own_allocated_bytes"), 4096);
    let compared = store
        .compare(
            &off,
            &on,
            &CompareQuery {
                metric: Metric::Allocated,
                limit: 100,
                cursor: None,
            },
            &STOP,
        )
        .unwrap();
    assert!(compared["records"].as_array().unwrap().is_empty());
    // Sanitized matrix uses symbolic ID/device relations, no fixture path or persistent identifiers.
    let matrix:Vec<_>=files.iter().map(|r|json!({"fixture":r["name_display"],"status":r["clone_status"],"clone_id_relation":if r["clone_id"]==src["clone_id"]{"source"}else{"different"},"extended_flags":r["clone_extended_flags"],"total":r["own_allocated_bytes"],"data":r["clone_data_allocated_bytes"]})).collect();
    println!(
        "{}",
        json!({"native_matrix":matrix,"primary_parity":true,"capture_context":store.manifest(&on).unwrap()["context"]["clone_capture"]})
    );
}
#[test]
fn supplemental_capture_respects_root_and_child_scope_exclusions() {
    let tree = Tree::new();
    let history = Tree::new();
    let provider = tree.0.join("Library/CloudStorage/owned");
    fs::create_dir_all(&provider).unwrap();
    fs::write(provider.join("hidden"), b"owned").unwrap();
    let excluded = tree.0.join("excluded");
    fs::create_dir(&excluded).unwrap();
    fs::write(excluded.join("hidden"), b"owned").unwrap();
    fs::write(tree.0.join("visible"), b"owned").unwrap();
    let store = Store::open(&history.0.join("history"), true).unwrap();
    let options = ScanOptions {
        capture_clones: true,
        exclusions: vec![excluded.clone()],
        ..ScanOptions::full()
    };
    let id = store
        .scan_to_store(&tree.0, &options, FULL_HISTORY_BUDGET, &STOP, |_| {})
        .unwrap();
    let all = rows(
        &store,
        &id,
        "name_display,kind,clone_status,clone_id,clone_data_allocated_bytes",
    );
    assert!(!all
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name_display"] == "hidden"));
    for name in ["CloudStorage", "excluded"] {
        let r = all
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name_display"] == name)
            .unwrap();
        assert!(r["clone_id"].is_null());
        assert!(r["clone_data_allocated_bytes"].is_null());
    }
    let root_id = capture(&store, &provider, true);
    assert_eq!(store.manifest(&root_id).unwrap()["entry_count"], 1);
    assert_eq!(
        store.manifest(&root_id).unwrap()["context"]["clone_capture"]["regular_file_status_counts"],
        json!({})
    );
    let file_id = capture(&store, &tree.0.join("visible"), true);
    let raw = rows(
        &store,
        &file_id,
        "clone_status,clone_id,clone_data_allocated_bytes,logical_bytes,coverage_complete",
    );
    assert_eq!(raw[0]["clone_status"], "missing");
    assert!(raw[0]["clone_id"].is_null());
    assert_eq!(raw[0]["logical_bytes"], "5");
    assert_eq!(raw[0]["coverage_complete"], true);
}
