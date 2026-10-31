use macscan_analysis::{encode_response, GrowthGrouping, GrowthQuery, Store, OUTPUT_BUDGET};
use macscan_engine::{scan, Metric, Scan, ScanOptions, ScanStatus, STORE_BUDGET};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
const C: AtomicBool = AtomicBool::new(false);
struct Fixture(PathBuf);
impl Fixture {
    fn new(label: &str) -> Self {
        let p = std::env::temp_dir().join(format!("macscan-growth-{}-{label}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, p: &str, n: usize) {
        let p = self.0.join(p);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0; n]).unwrap();
    }
    fn scan(&self, n: u32) -> Scan {
        let mut s = scan(&self.0, &ScanOptions::default(), &C).unwrap();
        s.id = format!("400-{n}");
        s.started_at = format!("2026-10-{}T10:00:00Z", 20 + n);
        s.ended_at = s.started_at.clone();
        s.context["started_unix_ns"] =
            Value::String(((1_792_490_400_u128 + 86_400 * n as u128) * 1_000_000_000).to_string());
        s.context["ended_unix_ns"] =
            Value::String(((1_792_490_400_u128 + 86_400 * n as u128) * 1_000_000_000).to_string());
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn save(store: &Store, s: &Scan) {
    store.save_with_cancel(s, STORE_BUDGET, &C).unwrap();
}
fn q() -> GrowthQuery {
    GrowthQuery {
        metric: Metric::Logical,
        ..Default::default()
    }
}
fn rows(store: &Store, mut q: GrowthQuery) -> Vec<Value> {
    let mut rows = vec![];
    loop {
        let p = store.growth("400-1", "400-2", &q, &C).unwrap();
        assert!(encode_response(p.clone(), false).unwrap().len() <= OUTPUT_BUDGET);
        assert!(encode_response(p.clone(), true).unwrap().len() <= OUTPUT_BUDGET);
        rows.extend(p["records"].as_array().unwrap().clone());
        match p["next_cursor"].as_str() {
            Some(c) => q.cursor = Some(c.into()),
            None => break,
        }
    }
    rows
}
fn id(s: &Scan, name: &[u8]) -> u32 {
    s.entries.iter().position(|e| e.name == name).unwrap() as u32
}
#[test]
fn leaf_accounting_types_moves_context_and_scoped_ids() {
    let f = Fixture::new("tree");
    let w = Fixture::new("store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    f.file("project/target/unchanged", 20);
    f.file("project/target/grow", 10);
    f.file("project/shrink", 30);
    f.file("project/remove", 9);
    f.file("project/move-from", 7);
    f.file("project/file-to-dir", 13);
    f.file("project/dir-to-file/nested", 4);
    let a = f.scan(1);
    save(&store, &a);
    f.file("aaa/shift-ids", 1);
    f.file("project/Cargo.toml", 1);
    f.file("project/target/grow", 40);
    f.file("project/shrink", 3);
    f.file("project/new", 12);
    fs::remove_file(f.0.join("project/remove")).unwrap();
    fs::rename(f.0.join("project/move-from"), f.0.join("project/move-to")).unwrap();
    fs::remove_file(f.0.join("project/file-to-dir")).unwrap();
    f.file("project/file-to-dir/nested", 2);
    fs::remove_dir_all(f.0.join("project/dir-to-file")).unwrap();
    f.file("project/dir-to-file", 5);
    let b = f.scan(2);
    save(&store, &b);
    let project = id(&b, b"project");
    assert_ne!(id(&a, b"project"), project);
    let query = GrowthQuery {
        entry: project,
        limit: 1,
        ..q()
    };
    let files = rows(
        &store,
        GrowthQuery {
            files: true,
            ..query.clone()
        },
    );
    assert_eq!(files.len(), 11);
    assert!(!files
        .iter()
        .any(|r| r["path_display"].as_str().unwrap().ends_with("unchanged")));
    assert!(files
        .iter()
        .any(|r| r["change"] == "removed" && r["delta_bytes"] == "-13"));
    assert!(files
        .iter()
        .any(|r| r["change"] == "added" && r["delta_bytes"] == "5"));
    let sum: i128 = files
        .iter()
        .map(|r| r["delta_bytes"].as_str().unwrap().parse::<i128>().unwrap())
        .sum();
    assert_eq!(sum, -3);
    let groups = rows(&store, query.clone());
    assert_eq!(
        groups
            .iter()
            .map(|r| r["delta_bytes"].as_str().unwrap().parse::<i128>().unwrap())
            .sum::<i128>(),
        sum
    );
    assert!(groups.iter().all(|r| r["path_display"].is_string()));
    let categories = rows(
        &store,
        GrowthQuery {
            grouping: GrowthGrouping::Category,
            ..query.clone()
        },
    );
    assert_eq!(
        categories
            .iter()
            .map(|r| r["delta_bytes"].as_str().unwrap().parse::<i128>().unwrap())
            .sum::<i128>(),
        sum
    );
    assert_eq!(
        categories
            .iter()
            .find(|r| r["group_key"] == "build_output")
            .unwrap()["delta_bytes"],
        "30"
    );
    let filtered = rows(
        &store,
        GrowthQuery {
            files: true,
            grouping: GrowthGrouping::Category,
            group: Some("build_output".into()),
            ..query.clone()
        },
    );
    assert_eq!(filtered.len(), 1);
    let page = store.growth("400-1", "400-2", &query, &C).unwrap();
    assert_eq!(page["interval"]["elapsed_ns"], "86400000000000");
    assert_eq!(
        page["interval"]["before_measured_at"],
        "2026-10-21T10:00:00Z"
    );
    assert_eq!(
        page["interval"]["after_measured_at"],
        "2026-10-22T10:00:00Z"
    );
    assert_eq!(
        page["interval"]["before_ended_unix_ns"],
        "1792576800000000000"
    );
    assert_eq!(
        page["interval"]["after_ended_unix_ns"],
        "1792663200000000000"
    );
    assert_eq!(
        page["scope"]["before"]["entry_id"],
        id(&a, b"project").to_string()
    );
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    assert!(store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                cursor: Some(cursor),
                files: true,
                ..query
            },
            &C
        )
        .is_err());
    assert!(store.growth("400-2", "400-1", &q(), &C).is_err());
    assert!(store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                entry: u32::MAX,
                ..q()
            },
            &C
        )
        .is_err());
    assert!(store
        .growth("400-1", "400-2", &q(), &AtomicBool::new(true))
        .is_err());
    // Optional retained, synthetic-only history for later native UI smoke.
    if let Some(p) = std::env::var_os("MACSCAN_GROWTH_FIXTURE") {
        let dest = PathBuf::from(p);
        fs::create_dir_all(&dest).unwrap();
        for s in [&a, &b] {
            copy_tree(&store.path().join(&s.id), &dest.join(&s.id));
        }
        fs::write(dest.join("fixture.json"),serde_json::to_vec_pretty(&serde_json::json!({"before":a.id,"after":b.id,"before_project_entry":id(&a,b"project"),"after_project_entry":project})).unwrap()).unwrap();
    }
}
fn copy_tree(a: &std::path::Path, b: &std::path::Path) {
    fs::create_dir_all(b).unwrap();
    for e in fs::read_dir(a).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &b.join(e.file_name()));
        } else {
            fs::copy(e.path(), b.join(e.file_name())).unwrap();
        }
    }
}
#[test]
fn partial_scope_no_false_removals_and_complete_child() {
    let f = Fixture::new("partial");
    let w = Fixture::new("partial-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    f.file("gone", 18);
    f.file("known/a", 3);
    let a = f.scan(1);
    save(&store, &a);
    fs::remove_file(f.0.join("gone")).unwrap();
    f.file("known/a", 8);
    let mut b = f.scan(2);
    b.status = ScanStatus::Partial;
    b.entries[0].complete = false;
    b.entries[0].enumeration_complete = false;
    save(&store, &b);
    let page = store
        .growth("400-1", "400-2", &GrowthQuery { files: true, ..q() }, &C)
        .unwrap();
    let gone = page["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path_display"].as_str().unwrap().ends_with("/gone"))
        .unwrap();
    assert_eq!(gone["change"], "unknown");
    assert!(gone["after_bytes"].is_null());
    assert!(gone["delta_bytes"].is_null());
    assert!(page["totals"]["after"]["bytes"].is_null());
    let empty = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                group: Some("no-observations".into()),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert!(empty["records"].as_array().unwrap().is_empty());
    assert!(empty["contributions"]["delta_bytes"].is_null());
    assert_eq!(empty["coverage"]["complete"], false);
    let child = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                entry: id(&b, b"known"),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert_eq!(child["coverage"]["complete"], true);
    assert_eq!(child["contributions"]["delta_bytes"], "5");
}
#[test]
fn hardlink_owner_and_exact_large_signed_values() {
    let f = Fixture::new("large");
    let w = Fixture::new("large-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    f.file("a", 1);
    fs::hard_link(f.0.join("a"), f.0.join("b")).unwrap();
    let mut a = f.scan(1);
    let huge = u64::MAX - 1;
    for e in a.entries.iter_mut().skip(1) {
        e.logical = huge;
        e.allocated = huge;
        if e.accounted > 0 {
            e.accounted = huge;
        }
    }
    save(&store, &a);
    let mut b = f.scan(2);
    for e in b.entries.iter_mut().skip(1) {
        e.logical = 1;
        e.allocated = 1;
        if e.accounted > 0 {
            e.accounted = 1;
        }
    }
    save(&store, &b);
    let page = store.growth("400-1", "400-2", &q(), &C).unwrap();
    assert_eq!(
        page["contributions"]["delta_bytes"],
        (2_i128 * (1_i128 - huge as i128)).to_string()
    );
    let allocated = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                metric: Metric::Allocated,
                ..q()
            },
            &C,
        )
        .unwrap();
    assert_eq!(
        allocated["contributions"]["delta_bytes"],
        (1_i128 - huge as i128).to_string()
    );
    assert_eq!(allocated["contributions"]["file_path_count"], "1");
}
#[test]
fn byte_boundary_area_keys_and_new_scopes() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new("raw");
    let w = Fixture::new("raw-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    let a = f.scan(1);
    save(&store, &a);
    let raw = std::ffi::OsString::from_vec(vec![0x10, 0x0a, b'x']);
    let dir = f.0.join(raw);
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("file"), [1, 2, 3]).unwrap();
    f.file("new-project/file", 4);
    let b = f.scan(2);
    save(&store, &b);
    let groups = rows(&store, q());
    assert!(groups.iter().any(|r| r["group_key"] == "100A7800"));
    let scoped = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                entry: id(&b, b"new-project"),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert!(scoped["scope"]["before"]["entry_id"].is_null());
    assert_eq!(scoped["totals"]["before"]["bytes"], "0");
    assert_eq!(scoped["contributions"]["delta_bytes"], "4");
}
#[test]
fn output_budget_pagination_no_gaps_and_filter_binding() {
    let f = Fixture::new("pages");
    let w = Fixture::new("pages-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    let a = f.scan(1);
    save(&store, &a);
    for i in 0..40 {
        f.file(&format!("project-{i:03}-{}", "x".repeat(70)), i + 1);
    }
    let b = f.scan(2);
    save(&store, &b);
    for files in [true, false] {
        let query = GrowthQuery {
            files,
            limit: 100,
            ..q()
        };
        let page = store.growth("400-1", "400-2", &query, &C).unwrap();
        assert!(page["records"].as_array().unwrap().len() < 40);
        assert!(page["truncated"] == true);
        let all = rows(&store, query.clone());
        assert_eq!(all.len(), 40);
        let mut keys = std::collections::HashSet::new();
        for row in all {
            assert!(keys.insert(row["group_key"].as_str().unwrap().to_owned()));
        }
        for changed in [
            GrowthQuery {
                change: "added".into(),
                ..query.clone()
            },
            GrowthQuery {
                group: Some("different".into()),
                ..query.clone()
            },
            GrowthQuery {
                metric: Metric::Allocated,
                ..query.clone()
            },
            GrowthQuery {
                grouping: GrowthGrouping::Category,
                ..query.clone()
            },
        ] {
            assert!(store
                .growth(
                    "400-1",
                    "400-2",
                    &GrowthQuery {
                        cursor: page["next_cursor"].as_str().map(String::from),
                        ..changed
                    },
                    &C
                )
                .is_err());
        }
    }
}
#[test]
fn removed_files_use_before_category_and_unknown_files_keep_observed_values() {
    let f = Fixture::new("category-before");
    let w = Fixture::new("category-before-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    f.file("Cargo.toml", 1);
    f.file("target/removed", 11);
    f.file("target/unchanged", 9);
    f.file("unreadable", 7);
    let a = f.scan(1);
    save(&store, &a);
    fs::remove_file(f.0.join("target/removed")).unwrap();
    fs::remove_file(f.0.join("Cargo.toml")).unwrap();
    let mut b = f.scan(2);
    let unknown = id(&b, b"unreadable");
    b.entries[unknown as usize].complete = false;
    b.entries[0].complete = false;
    b.status = ScanStatus::Partial;
    save(&store, &b);
    let page = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                grouping: GrowthGrouping::Category,
                files: true,
                ..q()
            },
            &C,
        )
        .unwrap();
    let records = page["records"].as_array().unwrap();
    let removed = records
        .iter()
        .find(|r| {
            r["path_display"]
                .as_str()
                .unwrap()
                .ends_with("target/removed")
        })
        .unwrap();
    assert_eq!(removed["group_key"], "build_output");
    assert_eq!(removed["delta_bytes"], "-11");
    assert_eq!(removed["drilldown_scan_id"], "400-1");
    assert!(removed["after_entry_id"].is_null());
    assert!(!records
        .iter()
        .any(|r| r["path_display"].as_str().unwrap().ends_with("unchanged")));
    let unknown = records.iter().find(|r| r["change"] == "unknown").unwrap();
    assert_eq!(unknown["observed_after_bytes"], "7");
    assert!(unknown["after_bytes"].is_null());
}
#[test]
fn zero_byte_new_and_removed_paths_remain_counted() {
    let f = Fixture::new("zero");
    let w = Fixture::new("zero-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    f.file("removed-empty", 0);
    let a = f.scan(1);
    save(&store, &a);
    fs::remove_file(f.0.join("removed-empty")).unwrap();
    f.file("new-empty", 0);
    let b = f.scan(2);
    save(&store, &b);
    let page = store.growth("400-1", "400-2", &q(), &C).unwrap();
    assert_eq!(page["contributions"]["delta_bytes"], "0");
    assert_eq!(page["contributions"]["new_count"], "1");
    assert_eq!(page["contributions"]["removed_count"], "1");
    let files = rows(&store, GrowthQuery { files: true, ..q() });
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|r| r["delta_bytes"] == "0"));
}
#[test]
fn new_scope_under_unobserved_before_region_is_unknown_not_zero() {
    let f = Fixture::new("missing-before");
    let w = Fixture::new("missing-before-store");
    let store = Store::open(&w.0.join("history"), true).unwrap();
    let mut a = f.scan(1);
    a.entries[0].complete = false;
    a.entries[0].enumeration_complete = false;
    a.status = ScanStatus::Partial;
    save(&store, &a);
    f.file("new-project/file", 6);
    let b = f.scan(2);
    save(&store, &b);
    let page = store
        .growth(
            "400-1",
            "400-2",
            &GrowthQuery {
                entry: id(&b, b"new-project"),
                files: true,
                ..q()
            },
            &C,
        )
        .unwrap();
    assert!(page["scope"]["before"]["entry_id"].is_null());
    assert_eq!(page["coverage"]["before_complete"], false);
    assert!(page["totals"]["before"]["bytes"].is_null());
    assert!(page["contributions"]["delta_bytes"].is_null());
    assert_eq!(page["records"][0]["change"], "unknown");
    assert!(page["records"][0]["before_bytes"].is_null());
    assert_eq!(page["records"][0]["observed_after_bytes"], "6");
}
