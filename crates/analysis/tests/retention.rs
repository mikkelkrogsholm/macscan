//! Owned synthetic filesystem only. Exercises real Parquet publication and planning.
use macscan_analysis::{Store, FULL_HISTORY_BUDGET};
use macscan_engine::{scan, Query, ScanOptions};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static STOP: AtomicBool = AtomicBool::new(false);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "macscan-retention-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::create_dir(p.join("source")).unwrap();
        fs::write(p.join("source/file"), b"owned synthetic bytes").unwrap();
        Self(p)
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("history"), true).unwrap()
    }
    fn save(&self, s: &Store, id: &str) {
        let mut scan = scan(&self.0.join("source"), &ScanOptions::default(), &STOP).unwrap();
        scan.id = id.into();
        s.save_with_cancel(&scan, FULL_HISTORY_BUDGET, &STOP)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn machine(s: &Store, id: &str, refs: &[&str]) {
    let rows: Vec<_> = refs
        .iter()
        .map(|r| json!({"phase":"done","scan_id":r}))
        .collect();
    fs::write(
        s.path().join(format!("{id}.machine.json")),
        serde_json::to_vec(
            &json!({"kind":"whole_mac_v1","session_id":id,"status":"complete","volumes":rows}),
        )
        .unwrap(),
    )
    .unwrap();
}
fn tree(p: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut a = vec![];
    for d in fs::read_dir(p).unwrap() {
        let d = d.unwrap();
        if d.file_type().unwrap().is_dir() {
            a.extend(tree(&d.path()));
        } else {
            a.push((d.path(), fs::read(d.path()).unwrap()));
        }
    }
    a.sort();
    a
}
#[test]
fn numerical_latest_three_and_dry_run_match_apply() {
    let f = Fixture::new();
    let s = f.store();
    for id in ["9-9", "10-2", "10-10", "11-1"] {
        f.save(&s, id);
    }
    let before = tree(s.path());
    let p = s.prune_history(false).unwrap();
    assert_eq!(tree(s.path()), before);
    assert_eq!(p["removed_runs"], 1);
    assert_eq!(p["removed_summary"][0], "9-9");
    let applied = s.prune_history(true).unwrap();
    assert_eq!(applied["removed_summary"], p["removed_summary"]);
    assert_eq!(s.ids().unwrap(), vec!["10-10", "10-2", "11-1"]);
    for id in s.ids().unwrap() {
        s.report(&id, &Query::default(), &STOP).unwrap();
    }
}
#[test]
fn groups_keep_all_children_and_shared_old_reference() {
    let f = Fixture::new();
    let s = f.store();
    for id in ["1-1", "1-2", "2-1", "2-2", "3-1", "3-2", "4-1", "4-2"] {
        f.save(&s, id);
    }
    machine(&s, "10-1", &["1-1", "1-2"]);
    machine(&s, "20-1", &["2-1", "2-2", "1-1"]);
    machine(&s, "30-1", &["3-1", "3-2"]);
    machine(&s, "40-1", &["4-1", "4-2"]);
    let p = s.prune_history(true).unwrap();
    assert_eq!(p["removed_runs"], 1);
    assert_eq!(p["removed_snapshots"], 1);
    assert!(!s.path().join("1-2").exists());
    assert!(s.manifest("1-1").is_ok());
    for id in ["20-1", "30-1", "40-1"] {
        let v: Value =
            serde_json::from_slice(&fs::read(s.path().join(format!("{id}.machine.json"))).unwrap())
                .unwrap();
        for r in v["volumes"].as_array().unwrap() {
            s.manifest(r["scan_id"].as_str().unwrap()).unwrap();
        }
    }
}
#[test]
fn malformed_references_fail_closed_and_warning_preserves_new_result() {
    let f = Fixture::new();
    let s = f.store();
    for id in ["1-1", "2-1", "3-1", "4-1"] {
        f.save(&s, id);
    }
    fs::write(s.path().join("5-1.machine.json"), b"{broken").unwrap();
    let before = tree(s.path());
    assert!(s.prune_history(true).is_err());
    assert_eq!(tree(s.path()), before);
    assert!(s.retention_warning("4-1").unwrap().contains("gemt"));
    s.report("4-1", &Query::default(), &STOP).unwrap();
    fs::remove_file(s.path().join("5-1.machine.json")).unwrap();
    machine(&s, "5-1", &["999-1"]);
    assert!(s.prune_history(true).is_err());
    assert!(s.manifest("1-1").is_ok());
}
#[test]
fn unknown_symlinks_and_active_stages_are_never_deleted() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let s = f.store();
    for id in ["1-1", "2-1", "3-1", "4-1"] {
        f.save(&s, id);
    }
    fs::create_dir(s.path().join("0-1")).unwrap();
    fs::write(s.path().join("0-1/user-owned"), b"keep").unwrap();
    fs::create_dir(s.path().join("0-2.partial")).unwrap();
    fs::write(s.path().join("0-2.partial/active"), b"keep").unwrap();
    symlink(f.0.join("source"), s.path().join("0-3")).unwrap();
    symlink(f.0.join("source/file"), s.path().join("1-1/foreign")).unwrap();
    fs::write(s.path().join("2-1/entries/user-extra.parquet"), b"unknown").unwrap();
    s.prune_history(true).unwrap();
    for p in [
        "0-1/user-owned",
        "0-2.partial/active",
        "0-3/file",
        "1-1/foreign",
        "2-1/entries/user-extra.parquet",
    ] {
        assert!(s.path().join(p).exists());
    }
    assert_eq!(
        fs::read(f.0.join("source/file")).unwrap(),
        b"owned synthetic bytes"
    );
}
#[test]
fn allowance_admits_new_parquet_above_old_total_but_explicit_limit_rejects() {
    let f = Fixture::new();
    let s = f.store();
    f.save(&s, "1-1");
    let bytes: u64 = tree(s.path()).iter().map(|(_, b)| b.len() as u64).sum();
    let guard = s.begin_run().unwrap();
    assert!(guard
        .scan_to_store(
            &f.0.join("source"),
            &ScanOptions::full(),
            bytes,
            &STOP,
            |_| {}
        )
        .is_err());
    let allowance = bytes * 2;
    let budget = guard.measurement_budget(allowance).unwrap();
    assert_eq!(budget, bytes + allowance);
    let id = guard
        .scan_to_store(
            &f.0.join("source"),
            &ScanOptions::full(),
            budget,
            &STOP,
            |_| {},
        )
        .unwrap();
    guard.report(&id, &Query::default(), &STOP).unwrap();
    assert!(guard
        .scan_to_store(
            &f.0.join("source"),
            &ScanOptions::full(),
            guard.measurement_budget(1).unwrap(),
            &STOP,
            |_| {}
        )
        .is_err());
    assert!(guard.measurement_budget(u64::MAX).is_err());
}
#[test]
fn concurrent_run_blocks_writes_and_pruning_and_stop_never_evicts() {
    let f = Fixture::new();
    let s = f.store();
    for id in ["1-1", "2-1", "3-1"] {
        f.save(&s, id);
    }
    let guard = s.begin_run().unwrap();
    let other = Store::open(s.path(), false).unwrap();
    assert!(other.prune_history(true).is_err());
    assert!(other
        .scan_to_store(
            &f.0.join("source"),
            &ScanOptions::full(),
            FULL_HISTORY_BUDGET,
            &STOP,
            |_| {}
        )
        .is_err());
    let stopped = AtomicBool::new(true);
    assert!(guard
        .scan_to_store(
            &f.0.join("source"),
            &ScanOptions::full(),
            guard.measurement_budget(FULL_HISTORY_BUDGET).unwrap(),
            &stopped,
            |_| {}
        )
        .is_err());
    assert_eq!(guard.ids().unwrap().len(), 3);
    drop(guard);
    assert!(other.prune_history(false).is_ok());
}

#[test]
fn automatic_cleanup_protects_just_published_run_despite_future_ids() {
    let f = Fixture::new();
    let s = f.store();
    for id in [
        "999999999999999999999-1",
        "999999999999999999998-1",
        "999999999999999999997-1",
        "4-1",
    ] {
        f.save(&s, id);
    }
    assert_eq!(s.prune_history(false).unwrap()["removed_summary"][0], "4-1");
    assert!(s.retention_warning("4-1").is_none());
    assert_eq!(s.ids().unwrap().len(), 3);
    s.report("4-1", &Query::default(), &STOP).unwrap();
    assert!(s.manifest("999999999999999999997-1").is_err());
}

#[test]
fn abandoned_workspace_reclamation_does_not_enlarge_measurement_allowance() {
    let f = Fixture::new();
    let s = f.store();
    f.save(&s, "1-1");
    let old: u64 = tree(s.path()).iter().map(|(_, b)| b.len() as u64).sum();
    let stage = s.path().join("2-1.partial");
    fs::create_dir(&stage).unwrap();
    fs::write(
        stage.join("stream-stage.json"),
        br#"{"owner":"macscan-streaming-v1","scan_id":"2-1"}"#,
    )
    .unwrap();
    fs::write(stage.join("workspace"), vec![0; 1024]).unwrap();
    let guard = s.begin_run().unwrap();
    assert_eq!(guard.measurement_budget(100).unwrap(), old + 100);
    assert!(!stage.exists());
}
