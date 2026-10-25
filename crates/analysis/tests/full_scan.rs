use macscan_analysis::{Store, FULL_HISTORY_BUDGET};
use macscan_engine::{scan, CompareQuery, Metric, Query, QueryKind, ScanOptions};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = PathBuf::from(format!(
            "/private/tmp/macscan-full-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn save(store: &Store, root: &PathBuf, options: &ScanOptions) -> String {
    store
        .scan_to_store(
            root,
            options,
            FULL_HISTORY_BUDGET,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
}

#[test]
fn streaming_totals_hardlinks_raw_names_and_categories_match_reference() {
    use std::os::unix::fs::symlink;
    let root = Fixture::new();
    let workspace = Fixture::new();
    fs::create_dir(root.0.join("node_modules")).unwrap();
    fs::write(root.0.join("node_modules/z"), b"12345").unwrap();
    fs::hard_link(root.0.join("node_modules/z"), root.0.join("a")).unwrap();
    fs::write(root.0.join("quoted\"\nfile"), b"xy").unwrap();
    fs::File::create(root.0.join("sparse"))
        .unwrap()
        .set_len(1_000_000_000)
        .unwrap();
    symlink(&root.0, root.0.join("loop")).unwrap();
    let reference = scan(&root.0, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let id = save(&store, &root.0, &ScanOptions::full());
    let m = store.manifest(&id).unwrap();
    assert_eq!(m["entry_count"], reference.entries.len());
    assert_eq!(
        m["totals"]["logical_bytes"],
        reference.report(0)["totals"]["logical_bytes"]
    );
    assert_eq!(
        m["totals"]["accounted_allocated_bytes"],
        reference.report(0)["totals"]["accounted_allocated_bytes"]
    );
    let rows=store.query_sql(&id,None,"SELECT path_display,logical_bytes,accounted_allocated_bytes,charged_to,category FROM entries ORDER BY path_bytes",100,0,&AtomicBool::new(false)).unwrap();
    for row in rows["records"].as_array().unwrap() {
        let expected = reference
            .entries
            .iter()
            .enumerate()
            .find(|(i, _)| {
                reference.path(*i as u32).to_string_lossy() == row["path_display"].as_str().unwrap()
            })
            .unwrap();
        assert_eq!(row["logical_bytes"], expected.1.logical.to_string());
        assert_eq!(
            row["accounted_allocated_bytes"],
            expected.1.accounted.to_string()
        );
    }
    let nested = rows["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["path_display"]
                .as_str()
                .unwrap()
                .ends_with("node_modules/z")
        })
        .unwrap();
    assert_eq!(nested["category"], "dependencies");
    assert!(nested["charged_to"].is_string());
    assert_eq!(fs::read_dir(store.path().join(&id)).unwrap().count(), 3);
}

#[test]
fn full_scan_supports_more_than_64_levels_and_small_batches() {
    let root = Fixture::new();
    let workspace = Fixture::new();
    let mut path = root.0.clone();
    for _ in 0..90 {
        path = path.join("d");
        fs::create_dir(&path).unwrap();
    }
    fs::write(path.join("end"), b"abc").unwrap();
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let mut options = ScanOptions::full();
    options.max_memory_bytes = 256 * 1024;
    let id = save(&store, &root.0, &options);
    let m = store.manifest(&id).unwrap();
    assert_eq!(m["status"], "complete");
    assert_eq!(m["entry_count"], 92);
    assert_eq!(m["totals"]["logical_bytes"], "3");
    let file = store
        .query_sql(
            &id,
            None,
            "SELECT entry_id FROM entries WHERE kind='file'",
            1,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    let entry = file["records"][0]["entry_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let view = store
        .sunburst(&id, entry, Metric::Logical, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(view["ancestors"].as_array().unwrap().len(), 92);
}

#[test]
fn partial_coverage_is_not_deletion_and_finished_branches_remain_complete() {
    use std::os::unix::fs::PermissionsExt;
    let root = Fixture::new();
    let workspace = Fixture::new();
    fs::create_dir(root.0.join("hidden")).unwrap();
    fs::write(root.0.join("hidden/a"), b"abc").unwrap();
    fs::create_dir(root.0.join("visible")).unwrap();
    fs::write(root.0.join("visible/b"), b"xy").unwrap();
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let before = save(&store, &root.0, &ScanOptions::full());
    fs::set_permissions(root.0.join("hidden"), fs::Permissions::from_mode(0o000)).unwrap();
    let after = store.scan_to_store(
        &root.0,
        &ScanOptions::full(),
        FULL_HISTORY_BUDGET,
        &AtomicBool::new(false),
        |_| {},
    );
    fs::set_permissions(root.0.join("hidden"), fs::Permissions::from_mode(0o700)).unwrap();
    let after = after.unwrap();
    assert_eq!(
        store.manifest(&after).unwrap()["totals"]["logical_bytes"],
        serde_json::Value::Null
    );
    let rows=store.query_sql(&after,None,"SELECT name_display,coverage_complete,enumeration_complete,logical_bytes FROM entries ORDER BY entry_id",20,0,&AtomicBool::new(false)).unwrap();
    let visible = rows["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name_display"] == "visible")
        .unwrap();
    assert_eq!(visible["coverage_complete"], true);
    assert_eq!(visible["logical_bytes"], "2");
    // Coverage query itself is independent of compare policy compatibility.
    let coverage = store
        .query_sql(
            &after,
            None,
            "SELECT count(*) AS n FROM entries WHERE name_display='a'",
            1,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(coverage["records"][0]["n"], "0");
    let changes = store
        .compare(
            &before,
            &after,
            &CompareQuery::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    let missing = changes["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path_display"].as_str().unwrap().ends_with("hidden/a"))
        .unwrap();
    assert!(missing["delta_bytes"].is_null());
}

#[test]
fn cancellation_and_budget_failure_leave_no_new_measurement_or_spool() {
    let root = Fixture::new();
    let workspace = Fixture::new();
    for i in 0..100 {
        fs::write(root.0.join(format!("f{i}")), b"123").unwrap();
    }
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let retained = save(&store, &root.0, &ScanOptions::full());
    let original = fs::read(store.path().join(&retained).join("manifest.json")).unwrap();
    let cancelled = AtomicBool::new(false);
    let result = store.scan_to_store(
        &root.0,
        &ScanOptions::full(),
        FULL_HISTORY_BUDGET,
        &cancelled,
        |p| {
            if p.entries_seen > 0 {
                cancelled.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    assert!(store
        .scan_to_store(
            &root.0,
            &ScanOptions::full(),
            1,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
    assert_eq!(fs::read_dir(store.path()).unwrap().count(), 1);
    assert_eq!(
        fs::read(store.path().join(retained).join("manifest.json")).unwrap(),
        original
    );
    let empty = Store::open(&workspace.0.join("empty"), true).unwrap();
    assert!(empty
        .scan_to_store(
            &root.0,
            &ScanOptions::full(),
            1,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
    assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
}

#[test]
fn live_progress_discovers_breadth_and_keeps_small_deep_files(){
    let root=Fixture::new();let workspace=Fixture::new();
    for name in ["left","right"]{let mut p=root.0.join(name);fs::create_dir(&p).unwrap();for _ in 0..9{p=p.join("d");fs::create_dir(&p).unwrap();}fs::write(p.join("tiny"),b"x").unwrap();}
    let store=Store::open(&workspace.0.join("history"),true).unwrap();let mut saw_breadth=false;let mut last=Vec::new();
    let id=store.scan_to_store(&root.0,&ScanOptions::full(),FULL_HISTORY_BUDGET,&AtomicBool::new(false),|p|{
        if p.entries_seen==1{saw_breadth=p.branches.iter().any(|b|b["name"]=="left"&&b["phase"]=="pending")&&p.branches.iter().any(|b|b["name"]=="right"&&b["phase"]=="pending");}
        assert!(p.branches.len()<=64);last=p.branches.clone();
    }).unwrap();
    assert!(saw_breadth);assert_eq!(store.manifest(&id).unwrap()["totals"]["logical_bytes"],"2");
    assert!(last.iter().all(|b|b["phase"]=="traversed"&&b["logical_bytes"]=="1"));
}

#[test]
#[ignore = "Creates and removes one million empty test files; run explicitly for release acceptance"]
fn scans_beyond_one_million_entries_with_bounded_memory() {
    let root = Fixture::new();
    let workspace = Fixture::new();
    for dir in 0..1001 {
        let path = root.0.join(format!("d{dir:04}"));
        fs::create_dir(&path).unwrap();
        for file in 0..1000 {
            fs::File::create(path.join(format!("f{file:04}"))).unwrap();
        }
    }
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let id = save(&store, &root.0, &ScanOptions::full());
    let m = store.manifest(&id).unwrap();
    assert_eq!(m["status"], "complete");
    assert_eq!(m["entry_count"], 1_002_002);
    let view = store
        .sunburst(&id, 0, Metric::Allocated, &AtomicBool::new(false))
        .unwrap();
    assert!(view["nodes"].as_array().unwrap().len() <= 320);
    let report = store
        .report(
            &id,
            &Query {
                kind: QueryKind::Summary,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(serde_json::to_vec(&report).unwrap().len() < 8192);
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let peak = unsafe { usage.assume_init() }.ru_maxrss;
    println!(
        "entries={} metadata_bytes={} peak_rss_bytes={peak}",
        m["entry_count"], report["scan_metadata_bytes"]
    );
    assert!(peak < 1024 * 1024 * 1024, "peak RSS exceeded 1 GiB: {peak}");
}

#[test]
fn restart_cleans_only_owned_unpublished_workspaces() {
    let root = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("history"), true).unwrap();
    let abandoned = store.path().join("1-2-3.partial");
    fs::create_dir(&abandoned).unwrap();
    fs::write(
        abandoned.join("stream-stage.json"),
        br#"{"owner":"macscan-streaming-v1","scan_id":"1-2-3"}"#,
    )
    .unwrap();
    fs::write(abandoned.join("scratch"), b"temporary data").unwrap();
    let unknown = store.path().join("4-5-6.partial");
    fs::create_dir(&unknown).unwrap();
    fs::write(unknown.join("keep"), b"unknown data").unwrap();
    let id = save(&store, &root.0, &ScanOptions::full());
    assert!(!abandoned.exists());
    assert_eq!(fs::read(unknown.join("keep")).unwrap(), b"unknown data");
    assert_eq!(store.ids().unwrap(), vec![id]);
}
