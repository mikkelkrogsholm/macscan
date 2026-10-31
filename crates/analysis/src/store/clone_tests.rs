use super::*;
use macscan_engine::{scan, ScanOptions};
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "macscan-old-clone-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn old_parquet_optional_clone_and_mtime_columns_do_not_disable_classification() {
    let tree = Tree::new();
    let root = tree.0.join("fixture");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("package.json"), b"{}").unwrap();
    fs::create_dir(root.join("node_modules")).unwrap();
    fs::write(root.join("node_modules/file.js"), b"own fixture").unwrap();
    let store = Store::open(&tree.0.join("history"), true).unwrap();
    let cancel = AtomicBool::new(false);
    let measured=scan(&root,&ScanOptions::default(),&cancel).unwrap();
    store.save_with_cancel(&measured,FULL_HISTORY_BUDGET,&cancel).unwrap();
    let id=measured.id;
    let baseline=store.query_sql(&id,None,"SELECT category,category_coverage,coverage_complete FROM entries WHERE name_display='file.js'",10,0,&cancel).unwrap();
    assert_eq!(
        baseline["records"][0],
        json!({"category":"dependencies","category_coverage":"complete","coverage_complete":true})
    );
    // Manufacture a historical schema solely inside this owned test fixture.
    let entries = store.location(&id).unwrap().join("entries");
    let original = fs::read_dir(&entries)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let old = entries.join("old.parquet");
    let db = Db::new().unwrap();
    db.exec(&format!("COPY (SELECT * EXCLUDE(mtime_unix_seconds) FROM read_parquet({})) TO {} (FORMAT PARQUET)",path_literal(&original).unwrap(),path_literal(&old).unwrap())).unwrap();
    drop(db);
    fs::remove_file(&original).unwrap();
    let bytes = fs::read(&old).unwrap();
    let manifest = fs::read(store.location(&id).unwrap().join("manifest.json")).unwrap();
    let result=store.query_sql(&id,None,"SELECT category,category_coverage,coverage_complete,clone_status,clone_id,clone_extended_flags,clone_data_allocated_bytes,clone_device_id,mtime_unix_seconds FROM entries WHERE name_display='file.js'",10,0,&cancel).unwrap();
    let row = &result["records"][0];
    assert_eq!(row["category"], "dependencies");
    assert_eq!(row["coverage_complete"], true);
    assert_eq!(row["category_coverage"], "complete");
    for name in [
        "clone_status",
        "clone_id",
        "clone_extended_flags",
        "clone_data_allocated_bytes",
        "clone_device_id",
        "mtime_unix_seconds",
    ] {
        assert!(row[name].is_null());
    }
    assert_eq!(fs::read(&old).unwrap(), bytes);
    assert_eq!(
        fs::read(store.location(&id).unwrap().join("manifest.json")).unwrap(),
        manifest
    );
}
#[test]
fn raw_supplemental_fields_roundtrip_legacy_and_parquet_without_primary_changes() {
    let tree = Tree::new();
    let root = tree.0.join("fixture");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"own").unwrap();
    let mut scan = scan(&root, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
    scan.entries[1].clone_metadata = macscan_engine::CloneMetadata {
        status: macscan_engine::CloneStatus::Observed,
        clone_id: Some(0),
        extended_flags: Some(65),
        data_allocated: Some(0),
        device: Some(7),
    };
    let legacy = macscan_engine::Store::open(&tree.0.join("legacy"), true).unwrap();
    legacy.save(&scan, macscan_engine::STORE_BUDGET).unwrap();
    let loaded = legacy.load(&scan.id).unwrap();
    assert_eq!(
        loaded.entries[1].clone_metadata,
        scan.entries[1].clone_metadata
    );
    let store = Store::open(&tree.0.join("history"), true).unwrap();
    store
        .save_with_cancel(&loaded, FULL_HISTORY_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let row=store.query_sql(&scan.id,None,"SELECT clone_status,clone_id,clone_extended_flags,clone_data_allocated_bytes,clone_device_id,coverage_complete FROM entries WHERE kind='file'",10,0,&AtomicBool::new(false)).unwrap();
    assert_eq!(
        row["records"][0],
        json!({"clone_status":"observed","clone_id":"0","clone_extended_flags":"65","clone_data_allocated_bytes":"0","clone_device_id":"7","coverage_complete":true})
    );
}
