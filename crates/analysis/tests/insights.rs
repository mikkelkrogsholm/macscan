use macscan_analysis::{encode_response, InsightsQuery, Store, OUTPUT_BUDGET};
use macscan_engine::{scan, Metric, ScanOptions, STORE_BUDGET};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
struct Fixture(PathBuf);
impl Fixture {
    fn new(label: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("macscan-insights-{}-{label}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, p: &str, n: usize) {
        let p = self.0.join(p);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![1; n]).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn all(store: &Store, id: &str, mut q: InsightsQuery) -> Vec<Value> {
    let mut rows = vec![];
    loop {
        let page = store.insights(id, &q, &AtomicBool::new(false)).unwrap();
        assert!(encode_response(page.clone(), false).unwrap().len() <= OUTPUT_BUDGET);
        assert!(encode_response(page.clone(), true).unwrap().len() <= OUTPUT_BUDGET);
        rows.extend(page["records"].as_array().unwrap().clone());
        match page["next_cursor"].as_str() {
            Some(c) => q.cursor = Some(c.into()),
            None => break,
        }
    }
    rows
}
#[test]
fn exact_leaf_rollups_catalog_precedence_and_candidate_pagination() {
    let f = Fixture::new("tree");
    let work = Fixture::new("store");
    for n in 0..35 {
        f.file(&format!("project-{n:03}/Cargo.toml"), 1);
        f.file(&format!("project-{n:03}/target/child/object"), 3);
    }
    for p in [
        "Library/Developer/Xcode/DerivedData/a",
        "Library/Developer/Xcode/Archives/a",
        "Library/Developer/CoreSimulator/Devices/a",
        "Library/Containers/com.docker.docker/Data/vms/0/Docker.raw",
        "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img",
        ".ollama/models/a",
        ".cache/huggingface/hub/models/a",
        ".lmstudio/models/a",
        ".npm/_cacache/a",
    ] {
        f.file(p, 7);
    }
    f.file(".cache/huggingface/hub/models/.cache/nested", 2);
    f.file(".cache/project/package.json", 1);
    f.file(".cache/project/personal", 9);
    f.file("backups/a", 11);
    f.file("node/package.json", 1);
    f.file("node/node_modules/pkg/package.json", 1);
    f.file("node/node_modules/pkg/src/a", 5);
    f.file("build/hint", 2);
    f.file(".venv/hint", 3);
    for p in [
        "Library/Caches/Homebrew/a",
        ".bun/install/cache/a",
        ".cache/uv/a",
    ] {
        f.file(p, 7);
    }
    fs::hard_link(
        f.0.join("project-000/target/child/object"),
        f.0.join("project-000/target/child/link"),
    )
    .unwrap();
    let data = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
    let store = Store::open(&work.0.join("history"), true).unwrap();
    store
        .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let part = store
        .path()
        .join(&data.id)
        .join("entries/part-00000.parquet");
    let bytes = fs::read(&part).unwrap();
    let q = InsightsQuery {
        metric: Metric::Logical,
        limit: 100,
        ..Default::default()
    };
    let rows = all(&store, &data.id, q.clone());
    let bucket = |name: &str| rows.iter().find(|r| r["category"] == name).unwrap();
    assert_eq!(bucket("build_output")["bytes"], "108");
    assert_eq!(bucket("build_output")["file_path_count"], "36");
    assert_eq!(bucket("huggingface_models")["bytes"], "9");
    for c in [
        "xcode_derived_data",
        "xcode_archives",
        "simulator_devices",
        "docker_disk",
        "orbstack_disk",
        "ollama_models",
        "lmstudio_models",
        "package_cache",
    ] {
        assert_eq!(
            bucket(c)["bytes"],
            if c == "package_cache" { "28" } else { "7" }
        );
        assert!(bucket(c)["guidance"].is_string());
    }
    let sql = store
        .query_sql(
            &data.id,
            None,
            "SELECT sum(logical_bytes) AS bytes FROM entries WHERE kind='file'",
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    let total = rows
        .iter()
        .map(|r| r["bytes"].as_str().unwrap().parse::<u64>().unwrap())
        .sum::<u64>();
    assert_eq!(total.to_string(), sql["records"][0]["bytes"]);
    let details = all(
        &store,
        &data.id,
        InsightsQuery {
            category: Some("build_output".into()),
            ..q.clone()
        },
    );
    assert_eq!(details.len(), 35);
    let ids = details
        .iter()
        .map(|r| r["entry_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 35);
    assert!(details
        .iter()
        .all(|r| r["drilldown_entry_id"] == r["entry_id"]));
    let page = store
        .insights(
            &data.id,
            &InsightsQuery {
                category: Some("build_output".into()),
                limit: 1,
                ..q.clone()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    assert!(store
        .insights(
            &data.id,
            &InsightsQuery {
                cursor: Some(cursor),
                ..q.clone()
            },
            &AtomicBool::new(false)
        )
        .is_err());
    let project = data
        .entries
        .iter()
        .position(|e| e.name == b"project-000")
        .unwrap() as u32;
    let scoped = store
        .insights(
            &data.id,
            &InsightsQuery {
                entry: project,
                ..q.clone()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(scoped["totals"]["bytes"], "7");
    let subtree = data.entries.iter().position(|e| e.name == b"src").unwrap() as u32;
    let clipped = all(
        &store,
        &data.id,
        InsightsQuery {
            entry: subtree,
            category: Some("dependencies".into()),
            ..q.clone()
        },
    );
    assert_eq!(clipped.len(), 1);
    assert_eq!(clipped[0]["bytes"], "5");
    assert_eq!(clipped[0]["drilldown_entry_id"], subtree.to_string());
    assert_ne!(clipped[0]["entry_id"], clipped[0]["drilldown_entry_id"]);
    let allocated = store
        .insights(
            &data.id,
            &InsightsQuery {
                entry: project,
                metric: Metric::Allocated,
                ..q.clone()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    let expected=store.query_sql(&data.id,None,"SELECT sum(accounted_allocated_bytes) AS bytes FROM entries WHERE kind='file' AND contains(path_display,'/project-000/')",10,0,&AtomicBool::new(false)).unwrap();
    assert_eq!(
        allocated["totals"]["bytes"],
        expected["records"][0]["bytes"]
    );
    fs::remove_file(f.0.join("project-000/Cargo.toml")).unwrap();
    assert_eq!(all(&store, &data.id, q), rows);
    assert_eq!(fs::read(part).unwrap(), bytes);
    assert!(store
        .insights(&data.id, &Default::default(), &AtomicBool::new(true))
        .is_err());
}
#[test]
fn incomplete_scope_keeps_unknown_totals_and_observed_lower_bound() {
    let f = Fixture::new("partial");
    let work = Fixture::new("partial-store");
    f.file("Cargo.toml", 1);
    f.file("target/a", 13);
    let mut data = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
    data.entries[0].complete = false;
    data.entries[0].enumeration_complete = false;
    let store = Store::open(&work.0.join("history"), true).unwrap();
    store
        .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let page = store
        .insights(
            &data.id,
            &InsightsQuery {
                metric: Metric::Logical,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(page["totals"]["bytes"].is_null());
    assert_eq!(page["totals"]["observed_lower_bound_bytes"], "14");
    assert_eq!(page["coverage"]["complete"], false);
    let target = data
        .entries
        .iter()
        .position(|e| e.name == b"target")
        .unwrap() as u32;
    let child = store
        .insights(
            &data.id,
            &InsightsQuery {
                entry: target,
                metric: Metric::Logical,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(child["totals"]["bytes"], "13");
    assert!(page["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["bytes"].is_null()));
}
