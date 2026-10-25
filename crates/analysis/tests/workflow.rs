use macscan_analysis::{Store, OUTPUT_BUDGET};
use macscan_engine::{scan, CompareQuery, Metric, Query, QueryKind, ScanOptions, STORE_BUDGET};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = PathBuf::from(format!(
                "/private/tmp/macscan-parquet-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        }
    }
    fn scan(&self) -> macscan_engine::Scan {
        scan(&self.0, &ScanOptions::default(), &AtomicBool::new(false)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn human_sunburst_matches_cli_and_is_bounded() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    for a in 0..15 {
        let folder = f.0.join(format!("folder-{a:02}"));
        fs::create_dir(&folder).unwrap();
        for b in 0..15 {
            let child = folder.join(format!("child-{b:02}"));
            fs::create_dir(&child).unwrap();
            for c in 0..3 {
                fs::write(child.join(format!("file-{c}")), b"12345").unwrap();
            }
        }
    }
    let scan = f.scan();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    store
        .save_with_cancel(&scan, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let view = store
        .sunburst(&scan.id, 0, Metric::Logical, &AtomicBool::new(false))
        .unwrap();
    let cli = store
        .report(
            &scan.id,
            &Query {
                kind: QueryKind::Summary,
                metric: Metric::Logical,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(view["selected"]["bytes"], cli["totals"]["logical_bytes"]);
    assert_eq!(view["child_count"], "15");
    assert!(view["nodes"].as_array().unwrap().len() <= 320);
    assert_eq!(view["ancestors"].as_array().unwrap().len(), 1);
    let id = view["nodes"][1]["entry_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let child = store
        .sunburst(&scan.id, id, Metric::Logical, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(child["ancestors"].as_array().unwrap().len(), 2);
    assert!(store
        .sunburst(&scan.id, u32::MAX, Metric::Logical, &AtomicBool::new(false))
        .is_err());
    let newer = f.scan();
    store
        .save_with_cancel(&newer, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let unchanged = store
        .query_sql(
            &newer.id,
            Some(&scan.id),
            "SELECT count(*) AS count FROM changes",
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(unchanged["records"][0]["count"], "0");
}

#[test]
fn parquet_budget_and_cancellation_preserve_retained_history() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::write(f.0.join("file"), b"content").unwrap();
    let first = f.scan();
    assert!(store
        .save_with_cancel(&first, 1, &AtomicBool::new(false))
        .is_err());
    assert_eq!(fs::read_dir(store.path()).unwrap().count(), 0);
    store
        .save_with_cancel(&first, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let second = f.scan();
    assert!(store
        .save_with_cancel(&second, 1, &AtomicBool::new(false))
        .is_err());
    assert!(store
        .save_with_cancel(&second, STORE_BUDGET, &AtomicBool::new(true))
        .is_err());
    assert_eq!(store.ids().unwrap(), vec![first.id.clone()]);
    assert_eq!(fs::read_dir(store.path()).unwrap().count(), 1);
    assert_eq!(store.manifest(&first.id).unwrap()["scan_id"], first.id);
    assert_eq!(fs::read(f.0.join("file")).unwrap(), b"content");
}

#[test]
fn scan_save_query_and_compare_parquet() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::write(f.0.join("grow"), b"123").unwrap();
    fs::write(f.0.join("gone"), b"1234").unwrap();
    let before = f.scan();
    store
        .save_with_cancel(&before, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let bytes = fs::read(
        store
            .path()
            .join(&before.id)
            .join("entries/part-00000.parquet"),
    )
    .unwrap();
    assert_eq!(&bytes[..4], b"PAR1");
    let count=store.query_sql(&before.id,None,"SELECT count(*) AS count, sum(CASE WHEN kind='file' THEN logical_bytes ELSE 0 END) AS bytes FROM entries",10,0,&AtomicBool::new(false)).unwrap();
    assert_eq!(count["records"][0]["count"], "3");
    assert_eq!(count["records"][0]["bytes"], "7");
    let children = store
        .report(
            &before.id,
            &Query {
                limit: 10,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(children["records"].as_array().unwrap().len(), 2);
    fs::write(f.0.join("grow"), b"12345678").unwrap();
    fs::remove_file(f.0.join("gone")).unwrap();
    fs::write(f.0.join("new"), b"12").unwrap();
    let after = f.scan();
    store
        .save_with_cancel(&after, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let compared = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery {
                metric: Metric::Logical,
                limit: 10,
                cursor: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(compared["totals"]["delta_bytes"], "3");
    assert!(serde_json::to_vec(&compared).unwrap().len() <= OUTPUT_BUDGET);
    let rows = compared["records"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["change"] == "removed"));
    let detailed = store
        .report(
            &before.id,
            &Query {
                kind: QueryKind::Inspect,
                entry: 1,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(detailed["records"][0]["path_bytes_hex"].is_string());
    assert!(store
        .query_sql(
            &after.id,
            None,
            "DELETE FROM entries",
            10,
            0,
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(store
        .save_with_cancel(&after, STORE_BUDGET, &AtomicBool::new(false))
        .is_err());
}
#[test]
fn missing_partial_subtree_is_unknown_not_deleted() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::create_dir(f.0.join("a")).unwrap();
    fs::write(f.0.join("a/keep"), b"x").unwrap();
    fs::write(f.0.join("a!"), b"y").unwrap();
    fs::write(f.0.join("gone"), b"z").unwrap();
    let before = f.scan();
    store
        .save_with_cancel(&before, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    fs::remove_file(f.0.join("gone")).unwrap();
    fs::set_permissions(f.0.join("a"), fs::Permissions::from_mode(0)).unwrap();
    let result = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(false));
    fs::set_permissions(f.0.join("a"), fs::Permissions::from_mode(0o700)).unwrap();
    let after = result.unwrap();
    store
        .save_with_cancel(&after, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let compared = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery {
                metric: Metric::Logical,
                limit: 10,
                cursor: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    let rows = compared["records"].as_array().unwrap();
    let unknown = rows
        .iter()
        .find(|r| r["path_display"].as_str().unwrap().ends_with("/a/keep"))
        .unwrap();
    assert_eq!(unknown["change"], "not_observed_or_incomplete");
    assert!(unknown["delta_bytes"].is_null());
    assert!(rows.iter().any(
        |r| r["path_display"].as_str().unwrap().ends_with("/gone") && r["change"] == "removed"
    ));
}
#[test]
fn agent_answers_are_small_and_paginated() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    for n in 0..45 {
        fs::write(f.0.join(format!("file-{n:03}")), vec![1_u8; n + 1]).unwrap();
    }
    let data = f.scan();
    store
        .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut q = Query {
        limit: 10,
        ..Query::default()
    };
    loop {
        let result = store.report(&data.id, &q, &AtomicBool::new(false)).unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() <= OUTPUT_BUDGET);
        for row in result["records"].as_array().unwrap() {
            assert!(seen.insert(row["entry_id"].as_str().unwrap().to_owned()));
        }
        if result["next_cursor"].is_null() {
            break;
        }
        q.cursor = Some(result["next_cursor"].as_str().unwrap().into());
    }
    assert_eq!(seen.len(), 45);
}

#[test]
fn sparse_hardlinks_and_inherited_categories_remain_exact() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::create_dir(f.0.join("target")).unwrap();
    fs::write(f.0.join("target/z"), vec![1_u8; 16384]).unwrap();
    fs::hard_link(f.0.join("target/z"), f.0.join("target/a")).unwrap();
    fs::File::create(f.0.join("sparse"))
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024)
        .unwrap();
    let data = f.scan();
    store
        .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let answer=store.query_sql(&data.id,None,"SELECT name_display,logical_bytes,allocated_bytes,accounted_allocated_bytes,category FROM entries WHERE kind='file' ORDER BY name_display",10,0,&AtomicBool::new(false)).unwrap();
    let rows = answer["records"].as_array().unwrap();
    let a = &rows[0];
    let sparse = &rows[1];
    let z = &rows[2];
    assert_eq!(a["category"], "build_output");
    assert_eq!(z["category"], "build_output");
    assert_eq!(z["accounted_allocated_bytes"], "0");
    assert_eq!(sparse["logical_bytes"], "2147483648");
    assert!(
        sparse["allocated_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            < 2147483648
    );
    let huge = store
        .query_sql(
            &data.id,
            None,
            "SELECT 170141183460469231731687303715884105727::HUGEINT AS exact",
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        huge["records"][0]["exact"],
        "170141183460469231731687303715884105727"
    );
}
