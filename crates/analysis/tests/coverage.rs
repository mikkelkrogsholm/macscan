use macscan_analysis::{encode_response, Store, FULL_HISTORY_BUDGET};
use macscan_engine::{
    coverage, CompareQuery, Entry, EntryKind, ExclusionReason, Metric, Query, QueryKind, Scan,
    ScanStatus,
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "macscan-coverage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn entry(parent: Option<u32>, name: &[u8], directory: bool) -> Entry {
    Entry {
        clone_metadata: macscan_engine::CloneMetadata::default(),
        parent,
        name: name.to_vec(),
        kind: if directory {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        logical: 7,
        allocated: 4096,
        accounted: 4096,
        own_allocated: 4096,
        device: 1,
        inode: 1,
        links: 1,
        observed_links: 1,
        charged_to: None,
        mtime: 0,
        complete: true,
        enumeration_complete: true,
    }
}
fn snapshot(id: &str, time: &str) -> Scan {
    Scan {
        id: id.into(),
        root: "/synthetic/macscan-coverage".into(),
        entries: vec![
            entry(None, b"", true),
            entry(Some(0), b"child", true),
            entry(Some(1), b"gone", false),
        ],
        issues: vec![],
        issue_count: 0,
        excluded_count: 0,
        status: ScanStatus::Complete,
        started_at: "2026-10-31T12:00:00Z".into(),
        ended_at: "2026-10-31T12:00:01Z".into(),
        duration_ms: 1000,
        policy: "synthetic-coverage-v1".into(),
        context: json!({"coverage_diagnostics":coverage::empty(),"started_unix_ns":time,"os_identity":"synthetic-os","volume_before":{"volume_uuid":"synthetic-volume"},"volume_after":{"volume_uuid":"synthetic-volume"}}),
    }
}
fn event(s: &mut Scan, code: &str, error: Option<&std::io::Error>, name: Option<&[u8]>) {
    coverage::record(
        &mut s.context["coverage_diagnostics"],
        1,
        code,
        "fstatat",
        &"æ".repeat(2000),
        error,
        name,
    );
    s.issue_count += 1;
    s.status = ScanStatus::Partial;
    s.entries[0].complete = false;
}
fn save(store: &Store, s: &Scan) {
    store
        .save_with_cancel(s, FULL_HISTORY_BUDGET, &AtomicBool::new(false))
        .unwrap();
}
#[test]
fn saved_events_paging_budgets_legacy_and_raw_child_identity() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let mut s = snapshot("610-1", "1");
    for _ in 0..150 {
        event(
            &mut s,
            "METADATA_ERROR",
            Some(&std::io::Error::from_raw_os_error(libc::EACCES)),
            Some(b"raw\xffchild"),
        );
    }
    event(&mut s, "CLOUD_EXCLUDED", None, None);
    s.excluded_count = 1;
    save(&store, &s);
    let report = store
        .coverage(&s.id, None, None, 1, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(report["quality"], "event_counts");
    assert_eq!(report["scope"], "whole_measurement");
    assert_eq!(report["reason_counts"]["access_denied"], "150");
    assert_eq!(report["samples_retained"], 3);
    assert_eq!(report["samples_truncated"], true);
    assert_eq!(
        report["records"][0]["child_name_bytes_hex"],
        "726177ff6368696c64"
    );
    assert!(report["records"][0]["path_bytes_hex"]
        .as_str()
        .unwrap()
        .ends_with("726177ff6368696c64"));
    assert_eq!(report["records"][0]["detail_truncated"], true);
    let cursor = report["next_cursor"].as_str().unwrap();
    let second = store
        .coverage(&s.id, None, Some(cursor), 1, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(second["offset"], 1);
    assert_eq!(second["reason_counts"], report["reason_counts"]);
    assert!(store
        .coverage(
            &s.id,
            Some("provider_excluded"),
            Some(cursor),
            1,
            &AtomicBool::new(false)
        )
        .is_err());
    for ndjson in [false, true] {
        assert!(
            encode_response(
                store
                    .coverage(&s.id, None, None, 100, &AtomicBool::new(false))
                    .unwrap(),
                ndjson
            )
            .unwrap()
            .len()
                <= 8192
        );
    }
    let mut m = store.manifest(&s.id).unwrap();
    m.as_object_mut().unwrap().remove("coverage_diagnostics");
    fs::write(
        store.path().join(&s.id).join("manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let legacy = store
        .coverage(&s.id, None, None, 10, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(legacy["quality"], "legacy_samples_only");
    assert!(legacy["reason_counts"].is_null());
    assert_eq!(legacy["error_count"], "151");
}
#[test]
fn global_volume_event_invalidates_children_and_prevents_false_deletion() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let before = snapshot("620-1", "1");
    save(&store, &before);
    let mut after = snapshot("620-2", "2");
    after.entries.pop();
    for _ in 0..150 {
        event(&mut after, "CLOUD_EXCLUDED", None, None);
    }
    event(&mut after, "VOLUME_CHANGED", None, None);
    save(&store, &after);
    let child = store
        .report(
            &after.id,
            &Query {
                kind: QueryKind::Inspect,
                entry: 1,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(child["records"][0]["logical_bytes"].is_null());
    let change = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery {
                metric: Metric::Logical,
                ..CompareQuery::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    for row in change["records"].as_array().unwrap() {
        assert!(row["delta_bytes"].is_null(), "{row}");
    }
    let rows = store
        .query_sql(
            &after.id,
            Some(&before.id),
            "SELECT path_display,logical_delta,logical_after,after_known FROM changes",
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    let gone = rows["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path_display"].as_str().unwrap().ends_with("/gone"))
        .unwrap();
    assert_eq!(gone["after_known"], false);
    assert!(gone["logical_delta"].is_null());
    assert!(gone["logical_after"].is_null());
}
#[test]
fn streaming_save_reload_matches_in_memory_event_distribution() {
    let temp = Temp::new();
    let root = temp.0.join("source");
    fs::create_dir(&root).unwrap();
    for name in ["a", "b", "c"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let mut options = macscan_engine::ScanOptions::default();
    options.exclusions = vec![root.join("a"), root.join("b"), root.join("c")];
    options.exclusion_reasons = vec![
        (root.join("a"), ExclusionReason::FirmlinkBoundary),
        (root.join("b"), ExclusionReason::VolumeBoundary),
        (root.join("c"), ExclusionReason::ScannerOutput),
    ];
    for i in 0..150 {
        let path = root.join(format!("excluded-{i}"));
        fs::create_dir(&path).unwrap();
        options.exclusions.push(path);
    }
    let legacy = macscan_engine::scan(&root, &options, &AtomicBool::new(false)).unwrap();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    save(&store, &legacy);
    let id = store
        .scan_to_store(
            &root,
            &options,
            FULL_HISTORY_BUDGET,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        store.manifest(&legacy.id).unwrap()["coverage_diagnostics"]["counts"],
        store.manifest(&id).unwrap()["coverage_diagnostics"]["counts"]
    );
    let report = store
        .coverage(&id, None, None, 10, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(report["reason_counts"]["firmlink_boundary"], "1");
    assert_eq!(report["reason_counts"]["volume_boundary"], "1");
    assert_eq!(report["reason_counts"]["scanner_output"], "1");
    assert_eq!(report["reason_counts"]["explicit_limit"], "150");
    assert_eq!(report["samples_truncated"], true);
    assert_eq!(report["samples_retained"], 5);
}
#[test]
fn long_saved_paths_and_all_reason_pages_stay_bounded_without_losing_counters() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let mut s = snapshot("630-1", "1");
    s.entries[1].name = vec![b'x'; 8000];
    for _ in 0..2 {
        for errno in [libc::EACCES, libc::ENOENT, libc::EIO] {
            event(
                &mut s,
                "METADATA_ERROR",
                Some(&std::io::Error::from_raw_os_error(errno)),
                Some(&vec![b'z'; 4096]),
            );
        }
        for code in [
            "CLOUD_EXCLUDED",
            "MOUNT_EXCLUDED",
            "FIRMLINK_EXCLUDED",
            "OUTPUT_EXCLUDED",
            "SCAN_LIMIT",
            "VOLUME_CHANGED",
        ] {
            event(&mut s, code, None, None);
        }
    }
    save(&store, &s);
    assert!(
        fs::metadata(store.path().join(&s.id).join("manifest.json"))
            .unwrap()
            .len()
            < 64 * 1024
    );
    let mut cursor = None;
    let mut total = 0;
    loop {
        let page = store
            .coverage(&s.id, None, cursor.as_deref(), 100, &AtomicBool::new(false))
            .unwrap();
        for r in coverage::REASONS {
            assert_eq!(page["reason_counts"][r], "2");
        }
        assert_eq!(page["samples_truncated"], false);
        for r in page["records"].as_array().unwrap() {
            assert_eq!(r["path_truncated"], true);
            assert!(r["path_bytes_hex"].is_null());
            if r["child_identity_truncated"] == true {
                assert!(r["child_name_bytes_hex"].is_null());
            }
        }
        for ndjson in [false, true] {
            assert!(encode_response(page.clone(), ndjson).unwrap().len() <= 8192);
        }
        total += page["records"].as_array().unwrap().len();
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(total, 18);
}
#[test]
fn unavailable_or_invalid_legacy_sample_context_does_not_invent_paths() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let mut s = snapshot("640-1", "1");
    event(&mut s, "CLOUD_EXCLUDED", None, None);
    save(&store, &s);
    let mut m = store.manifest(&s.id).unwrap();
    m.as_object_mut().unwrap().remove("coverage_diagnostics");
    m["issues"] = json!([
        {"entry_id":"999","code":"METADATA_ERROR","operation":"fstatat","detail":"old missing context","child_name_bytes_hex":"6368696c64"},
        {"entry_id":"1","code":"METADATA_ERROR","operation":"fstatat","detail":"invalid child","child_name_bytes_hex":"002f"}]);
    fs::write(
        store.path().join(&s.id).join("manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let report = store
        .coverage(&s.id, None, None, 10, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(report["records"][0]["path_available"], false);
    assert!(report["records"][0]["path_display"].is_null());
    assert_eq!(report["records"][1]["child_identity_invalid"], true);
    assert_eq!(
        report["records"][1]["path_display"],
        "/synthetic/macscan-coverage/child"
    );
    assert_eq!(report["error_count"], "1");
    assert!(report["reason_counts"].is_null());
    assert_eq!(report["started_at"], s.started_at);
    assert_eq!(report["ended_at"], s.ended_at);
}
#[test]
fn old_counterless_root_only_volume_invalidation_remains_global_on_read() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let before = snapshot("650-1", "1");
    save(&store, &before);
    let mut after = snapshot("650-2", "2");
    after.entries.pop();
    after.status = ScanStatus::Partial;
    after.entries[0].complete = false;
    after.entries[0].enumeration_complete = false;
    save(&store, &after);
    let path = store.path().join(&after.id).join("manifest.json");
    let mut m = store.manifest(&after.id).unwrap();
    m.as_object_mut().unwrap().remove("coverage_diagnostics");
    m["context"]["volume_before"]["fsid"] = json!("old-volume");
    m["context"]["volume_after"]["fsid"] = json!("changed-volume");
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let saved_bytes = fs::read(&path).unwrap();
    let diagnostics=store.coverage(&after.id,None,None,10,&AtomicBool::new(false)).unwrap();assert_eq!(diagnostics["status"],"partial");assert_eq!(diagnostics["read_time_global_invalidation"],true);assert!(diagnostics["reason_counts"].is_null());assert!(diagnostics["coverage_note_da"].as_str().unwrap().contains("ukendt"));
    let child = store
        .report(
            &after.id,
            &Query {
                kind: QueryKind::Inspect,
                entry: 1,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(child["records"][0]["logical_bytes"].is_null());
    assert_eq!(child["records"][0]["coverage_complete"], false);
    let profile = store
        .file_profile(
            &after.id,
            &macscan_analysis::FileProfileQuery {
                entry: 1,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(profile["coverage"]["complete"], false);
    let rows = store
        .query_sql(
            &after.id,
            Some(&before.id),
            "SELECT after_known,logical_delta FROM changes WHERE path_display LIKE '%/gone'",
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(rows["records"][0]["after_known"], false);
    assert!(rows["records"][0]["logical_delta"].is_null());
    assert_eq!(fs::read(&path).unwrap(), saved_bytes);
    m["context"]["volume_before"]
        .as_object_mut()
        .unwrap()
        .remove("fsid");
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let ordinary = store
        .report(
            &after.id,
            &Query {
                kind: QueryKind::Inspect,
                entry: 1,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(ordinary["records"][0]["coverage_complete"], true);
}

#[test]
#[ignore = "Writes directly constructed saved synthetic history for native run B"]
fn write_native_coverage_fixtures() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evidence/issue-6A");
    fs::create_dir_all(&base).unwrap();
    let store = Store::open(&base.join("native-history"), true).unwrap();
    let mut complete = snapshot("660-1", "1793448000000000000");
    complete.context["ended_unix_ns"] = json!("1793448001000000000");
    complete.entries[0].allocated = 12288;
    complete.entries[0].accounted = 12288;
    complete.entries[1].allocated = 8192;
    complete.entries[1].accounted = 8192;
    save(&store, &complete);
    let mut partial = snapshot("660-2", "1793448300000000000");
    partial.started_at = "2026-10-31T12:05:00Z".into();
    partial.ended_at = "2026-10-31T12:05:01Z".into();
    partial.context["ended_unix_ns"] = json!("1793448301000000000");
    partial.entries[0].allocated = 12288;
    partial.entries[0].accounted = 12288;
    partial.entries[1].allocated = 8192;
    partial.entries[1].accounted = 8192;
    for _ in 0..150 {
        event(
            &mut partial,
            "METADATA_ERROR",
            Some(&std::io::Error::from_raw_os_error(libc::EACCES)),
            Some(b"unavailable-child"),
        );
    }
    for code in [
        "CLOUD_EXCLUDED",
        "MOUNT_EXCLUDED",
        "FIRMLINK_EXCLUDED",
        "OUTPUT_EXCLUDED",
        "EXPLICIT_EXCLUDED",
    ] {
        event(&mut partial, code, None, None);
        partial.excluded_count += 1;
    }
    partial.entries[0].enumeration_complete = false;
    partial.entries[1].complete = false;
    partial.entries[1].enumeration_complete = false;
    save(&store, &partial);
    let mut legacy = snapshot("660-3", "1793448600000000000");
    legacy.started_at = "2026-10-31T12:10:00Z".into();
    legacy.ended_at = "2026-10-31T12:10:01Z".into();
    legacy.context["ended_unix_ns"] = json!("1793448601000000000");
    legacy.issue_count = 151;
    legacy.excluded_count = 1;
    legacy.status = ScanStatus::Partial;
    legacy.entries[0].allocated=12288;legacy.entries[0].accounted=12288;
    legacy.entries[1].allocated=8192;legacy.entries[1].accounted=8192;
    legacy.entries[0].complete = false;
    legacy.entries[0].enumeration_complete = false;
    legacy.issues.push(macscan_engine::Issue {
        entry_id: 0,
        code: "METADATA_ERROR".into(),
        operation: "fstatat".into(),
        detail: "Synthetic saved legacy permission failure".into(),
        name: None,
    });
    save(&store, &legacy);
    let mut old = store.manifest(&legacy.id).unwrap();
    old.as_object_mut().unwrap().remove("coverage_diagnostics");
    fs::write(
        store.path().join(&legacy.id).join("manifest.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let fixtures = json!({"source":"directly constructed synthetic metadata; no source scans","root":"/synthetic/macscan-coverage","scans":[
        {"scan_id":complete.id,"quality":"event_counts","status":"complete","event_total":"0","reason_counts":complete.context["coverage_diagnostics"]["counts"],"samples_retained":0,"samples_truncated":false,"started_at":complete.started_at,"ended_at":complete.ended_at},
        {"scan_id":partial.id,"quality":"event_counts","status":"partial","event_total":"155","reason_counts":partial.context["coverage_diagnostics"]["counts"],"samples_retained":7,"samples_truncated":true,"started_at":partial.started_at,"ended_at":partial.ended_at},
        {"scan_id":legacy.id,"quality":"legacy_samples_only","status":"partial","event_total":"151","reason_counts":null,"samples_retained":1,"samples_truncated":true,"started_at":legacy.started_at,"ended_at":legacy.ended_at}]});
    fs::write(
        base.join("fixtures.json"),
        serde_json::to_vec_pretty(&fixtures).unwrap(),
    )
    .unwrap();
}
#[test]
fn oversize_manifest_is_execution_failure_and_preserves_published_history() {
    let temp = Temp::new();
    let store = Store::open(&temp.0.join("history"), true).unwrap();
    let before = snapshot("670-1", "1");
    save(&store, &before);
    let original = fs::read(store.path().join(&before.id).join("manifest.json")).unwrap();
    let mut rejected = snapshot("670-2", "2");
    rejected.policy = "x".repeat(70 * 1024);
    let err = store
        .save_with_cancel(&rejected, FULL_HISTORY_BUDGET, &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::StorageFull);
    assert_eq!(store.ids().unwrap(), vec![before.id.clone()]);
    assert!(!store.path().join("670-2.partial").exists());
    assert_eq!(
        fs::read(store.path().join(&before.id).join("manifest.json")).unwrap(),
        original
    );
}
