//! Actual CLI processes over owned synthetic immutable history only.
use macscan_analysis::{Store, FULL_HISTORY_BUDGET};
use macscan_engine::{scan, ScanOptions};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "macscan-cli-history-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::create_dir(p.join("source")).unwrap();
        fs::write(p.join("source/file.txt"), b"synthetic").unwrap();
        Self(p)
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("history"), true).unwrap()
    }
    fn save(&self, id: &str) {
        let mut s = scan(
            &self.0.join("source"),
            &ScanOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        s.id = id.into();
        self.store()
            .save_with_cancel(&s, FULL_HISTORY_BUDGET, &AtomicBool::new(false))
            .unwrap();
    }
    fn session(&self, id: &str, status: &str, volumes: Value) {
        fs::write(self.store().path().join(format!("{id}.machine.json")),serde_json::to_vec(&json!({"kind":"whole_mac_v1","session_id":id,"status":status,"started_at":"2026-11-03T00:00:00Z","volumes":volumes,"capacity_bytes":"10000","used_bytes":"6000","free_bytes":"4000"})).unwrap()).unwrap();
    }
    fn call(&self, args: &[&str]) -> Output {
        let o = Command::new(env!("CARGO_BIN_EXE_macscan"))
            .args(args)
            .arg("--store")
            .arg(self.0.join("history"))
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(o.stdout.len() <= 8192);
        assert!(o.stderr.len() <= 8192);
        o
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.call(args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn pages(&self, view: &[&str], ndjson: bool) -> Vec<Value> {
        let mut all = vec![];
        let mut cursor: Option<String> = None;
        loop {
            let mut args = vec!["scans"];
            args.extend(view);
            args.extend(["--limit", "100"]);
            if ndjson {
                args.extend(["--format", "ndjson"]);
            }
            if let Some(ref c) = cursor {
                args.extend(["--cursor", c]);
            }
            let o = self.call(&args);
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            let page: Value = if ndjson {
                let lines: Vec<Value> = String::from_utf8(o.stdout)
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect();
                assert_eq!(lines.first().unwrap()["type"], "start");
                let mut p = lines[0].clone();
                p["records"] = json!(lines
                    .iter()
                    .filter(|l| l["type"] == "record")
                    .map(|l| l["record"].clone())
                    .collect::<Vec<_>>());
                assert_eq!(lines.last().unwrap()["next_cursor"], p["next_cursor"]);
                p
            } else {
                serde_json::from_slice(&o.stdout).unwrap()
            };
            all.extend(page["records"].as_array().unwrap().clone());
            match page["next_cursor"].as_str() {
                Some(c) => {
                    assert_ne!(cursor.as_deref(), Some(c));
                    cursor = Some(c.to_owned())
                }
                None => break,
            }
            assert!(all.len() < 200);
        }
        all
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn mixed_runs_members_and_existing_analysis_commands() {
    let f = Fixture::new();
    for id in ["9-1", "10-1", "11-1"] {
        f.save(id);
    }
    f.session("12-1","partial",json!([{"phase":"done","scan_id":"10-1","uuid":"data"},{"phase":"unmounted","scan_id":null,"uuid":"off"},{"phase":"error","scan_id":null,"error":"access denied"},{"phase":"not_scanned","scan_id":null},{"phase":"cancelled","scan_id":null}]));
    f.session("13-1", "cancelled", json!([]));
    let p = f.ok(&["scans"]);
    let rows = p["records"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["run_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["13-1", "12-1", "11-1", "9-1"]
    );
    assert_eq!(rows[1]["kind"], "whole_mac");
    assert_eq!(rows[1]["snapshot_count"], 1);
    assert!(rows[1]["snapshot_id"].is_null());
    assert_eq!(rows[1]["os_capacity"]["used_bytes"], "6000");
    assert!(f.ok(&["scans", "--run", "13-1"])["records"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(f.ok(&["scans", "--run", "13-1"])["run_status"], "cancelled");
    let m = f.ok(&["scans", "--run", "12-1"]);
    assert_eq!(m["run_status"], "partial");
    let r = m["records"].as_array().unwrap();
    assert_eq!(r.len(), 5);
    assert_eq!(r[0]["snapshot_id"], "10-1");
    for row in &r[1..] {
        assert!(row["entries"].is_null());
        assert!(row["coverage_complete"].is_null());
    }
    assert_eq!(r[2]["reason"], "access denied");
    assert_eq!(
        f.ok(&["scans", "--run", "9-1"])["records"][0]["snapshot_id"],
        "9-1"
    );
    for cmd in [
        "summary", "children", "inspect", "insights", "profile", "coverage",
    ] {
        f.ok(&[cmd, "--scan", "10-1"]);
    }
    f.ok(&["compare", "9-1", "10-1"]);
    f.ok(&["growth", "9-1", "10-1"]);
    for cmd in [
        "summary", "children", "inspect", "insights", "profile", "coverage",
    ] {
        let o = f.call(&[cmd, "--scan", "12-1"]);
        assert_eq!(o.status.code(), Some(2));
        let e: Value = serde_json::from_slice(&o.stderr).unwrap();
        assert_eq!(e["error"]["code"], "WRONG_ID_TYPE");
        assert!(e["error"]["hint"].as_str().unwrap().contains("scans --run"));
    }
    f.session("14-1", "partial", json!([]));
    let empty = f.ok(&["scans", "--run", "14-1"]);
    assert_eq!(empty["run_status"], "partial");
    assert_eq!(empty["run_kind"], "whole_mac");
    assert_eq!(empty["outcome"], "ok");
    assert!(empty["records"].as_array().unwrap().is_empty());
    assert_eq!(
        f.ok(&["scans", "--view", "snapshots"])["records"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
#[test]
fn pagination_numerical_order_and_byte_compaction_json_ndjson() {
    let f = Fixture::new();
    for n in 1..=24 {
        f.save(&format!("{n}-1"));
    }
    // Long but compatible metadata forces byte-limited pages below the row limit.
    for id in f.store().ids().unwrap() {
        let p = f.store().path().join(&id).join("manifest.json");
        let mut m: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        m["scope"]["root_display"] = json!("r".repeat(900));
        fs::write(p, serde_json::to_vec(&m).unwrap()).unwrap();
    }
    assert_eq!(f.ok(&["scans"])["records"].as_array().unwrap().len(), 10);
    for view in [vec![], vec!["--view", "snapshots"]] {
        let a = f.pages(&view, false);
        let b = f.pages(&view, true);
        assert_eq!(a, b);
        assert_eq!(a.len(), 24);
        let field = if view.is_empty() { "run_id" } else { "scan_id" };
        assert_eq!(
            a.iter()
                .map(|r| r[field].as_str().unwrap())
                .collect::<Vec<_>>(),
            (1..=24).rev().map(|n| format!("{n}-1")).collect::<Vec<_>>()
        );
    }
    let volumes:Vec<_>=(0..24).map(|i|json!({"phase":"unmounted","scan_id":null,"uuid":format!("{i}-{}","x".repeat(600))})).collect();
    f.session("30-1", "partial", json!(volumes));
    let a = f.pages(&["--run", "30-1"], false);
    let b = f.pages(&["--run", "30-1"], true);
    assert_eq!(a, b);
    assert_eq!(a.len(), 24);
    for (i, r) in a.iter().enumerate() {
        assert_eq!(r["member_index"], i);
    }
    assert_eq!(f.call(&["scans", "--limit", "101"]).status.code(), Some(2));
    assert_eq!(
        f.call(&["scans", "--view", "snapshots", "--run", "30-1"])
            .status
            .code(),
        Some(2)
    );
    let c = f.ok(&["scans", "--limit", "1"])["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        f.call(&["scans", "--view", "snapshots", "--cursor", &c])
            .status
            .code(),
        Some(2)
    );
}
#[test]
fn groups_shared_references_retention_four_to_three() {
    let f = Fixture::new();
    for id in ["1-1", "2-1", "3-1", "4-1", "5-1"] {
        f.save(id);
    }
    f.session(
        "10-1",
        "complete",
        json!([{"phase":"done","scan_id":"1-1"},{"phase":"done","scan_id":"2-1"}]),
    );
    f.session(
        "20-1",
        "complete",
        json!([{"phase":"done","scan_id":"1-1"},{"phase":"done","scan_id":"3-1"}]),
    );
    f.session(
        "30-1",
        "complete",
        json!([{"phase":"done","scan_id":"4-1"}]),
    );
    f.session(
        "40-1",
        "complete",
        json!([{"phase":"done","scan_id":"5-1"}]),
    );
    assert_eq!(f.ok(&["scans"])["records"].as_array().unwrap().len(), 4);
    let dry = f.ok(&["history-prune"]);
    assert_eq!(dry["removed_runs"], 1);
    assert_eq!(dry["removed_snapshots"], 1);
    assert_eq!(f.ok(&["scans"])["records"].as_array().unwrap().len(), 4);
    f.ok(&["history-prune", "--apply"]);
    assert_eq!(f.ok(&["scans"])["records"].as_array().unwrap().len(), 3);
    f.ok(&["summary", "--scan", "1-1"]);
    assert!(!f.store().path().join("2-1").exists());
    assert_eq!(
        f.ok(&["scans", "--run", "20-1"])["records"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn missing_empty_corrupt_and_dangling_history_reads_do_not_mutate() {
    let f = Fixture::new();
    assert!(f.ok(&["scans"])["records"].as_array().unwrap().is_empty());
    assert!(!f.0.join("history").exists());
    assert_eq!(f.call(&["summary", "--scan", "1-1"]).status.code(), Some(4));
    f.store();
    assert!(f.ok(&["scans"])["records"].as_array().unwrap().is_empty());
    let p = f.store().path().join("2-1.machine.json");
    for data in [b"broken".to_vec(),serde_json::to_vec(&json!({"kind":"whole_mac_v1","session_id":"2-1","status":"complete","volumes":[]})).unwrap(),serde_json::to_vec(&json!({"kind":"whole_mac_v1","session_id":"2-1","status":"complete","volumes":[{"phase":"done","scan_id":"1-1"}]})).unwrap()]{fs::write(&p,&data).unwrap();let o=f.call(&["scans"]);assert!(!o.status.success());assert!(o.stdout.is_empty());assert_eq!(fs::read(&p).unwrap(),data);assert_eq!(fs::read_dir(f.store().path()).unwrap().count(),1);}
}

#[test]
fn inconsistent_session_status_never_claims_complete() {
    let f = Fixture::new();
    f.save("1-1");
    let manifest = f.store().path().join("1-1/manifest.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    m["status"] = json!("partial");
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    f.session("2-1", "complete", json!([{"phase":"done","scan_id":"1-1"}]));
    assert!(!f.call(&["scans"]).status.success());
    f.session(
        "2-1",
        "partial",
        json!([{"phase":"done","scan_id":"1-1","scan_status":"complete"}]),
    );
    assert!(!f.call(&["scans"]).status.success());
    f.session("2-1", "partial", json!([{"phase":"done","scan_id":"1-1"}]));
    let p = f.ok(&["scans"]);
    assert_eq!(p["records"][0]["status"], "partial");
    assert_eq!(p["records"][0]["coverage"]["complete"], false);
    assert_eq!(f.store().manifest("1-1").unwrap()["status"], "partial");
}

#[test]
fn legacy_unknown_partial_members_are_readable_but_never_pruned() {
    let f = Fixture::new();
    f.save("1-1");
    f.session(
        "2-1",
        "partial",
        json!([
            {"phase":"done","scan_id":"1-1"},
            {"phase":"pending","scan_id":null,"entries":0},
            {"phase":"future_phase","scan_id":null,"entries":0}
        ]),
    );
    let p = f.ok(&["scans", "--run", "2-1"]);
    assert_eq!(p["run_status"], "partial");
    assert_eq!(p["records"][1]["phase"], "pending");
    assert_eq!(p["records"][2]["phase"], "future_phase");
    for row in p["records"].as_array().unwrap().iter().skip(1) {
        assert!(row["snapshot_id"].is_null());
        assert!(row["entries"].is_null());
        assert!(row["coverage_complete"].is_null());
    }
    let before = fs::read(f.store().path().join("2-1.machine.json")).unwrap();
    assert!(!f.call(&["history-prune", "--apply"]).status.success());
    assert_eq!(
        fs::read(f.store().path().join("2-1.machine.json")).unwrap(),
        before
    );
    f.ok(&["summary", "--scan", "1-1"]);
    f.session(
        "2-1",
        "complete",
        json!([{"phase":"future_phase","scan_id":null}]),
    );
    assert!(!f.call(&["scans"]).status.success());
}
