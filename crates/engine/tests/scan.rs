use macscan_engine::{
    scan, CompareQuery, EntryKind, Metric, Query, QueryKind, ScanOptions, ScanStatus, Store,
    OUTPUT_BUDGET, STORE_BUDGET,
};
use std::fs::{self, File};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = PathBuf::from(format!(
                "/private/tmp/macscan-fixture-{}-{}",
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
    fn run(&self) -> macscan_engine::Scan {
        scan(&self.0, &ScanOptions::default(), &AtomicBool::new(false)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove only this fixture's owned root");
    }
}

#[test]
fn t01_logical_and_allocated_are_separate() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("a"), b"12345").unwrap();
    fs::write(f.0.join("nested/b"), b"123").unwrap();
    let data = f.run();
    assert_eq!(data.status, ScanStatus::Complete);
    assert_eq!(data.entries[0].logical, 8);
    assert_eq!(data.entries.len(), 4);
    let report = data.report(1);
    assert_eq!(report["totals"]["logical_bytes"], "8");
    assert_eq!(report["totals"]["private_bytes"], serde_json::Value::Null);
    assert_eq!(report["records"].as_array().unwrap().len(), 1);
    assert_eq!(report["truncated"], true);
}

#[test]
fn t02_sparse_file_does_not_use_logical_length_as_allocated() {
    let f = Fixture::new();
    let path = f.0.join("sparse");
    File::create(&path)
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024)
        .unwrap();
    let metadata = path.metadata().unwrap();
    let data = f.run();
    let entry = data
        .entries
        .iter()
        .find(|e| e.kind == EntryKind::File)
        .unwrap();
    assert_eq!(entry.logical, 2 * 1024 * 1024 * 1024);
    assert_eq!(entry.allocated, metadata.blocks() * 512);
    assert!(entry.allocated < entry.logical);
}

#[test]
fn t03_hardlink_is_accounted_once_and_has_a_stable_owner() {
    let f = Fixture::new();
    fs::write(f.0.join("z"), vec![1_u8; 16384]).unwrap();
    fs::hard_link(f.0.join("z"), f.0.join("a")).unwrap();
    let data = f.run();
    let a = data.entries.iter().find(|e| e.name == b"a").unwrap();
    let z = data.entries.iter().find(|e| e.name == b"z").unwrap();
    assert_eq!(z.accounted, 0);
    assert!(z.charged_to.is_some());
    assert_eq!(a.observed_links, 2);
    assert_eq!(data.entries[0].logical, 32768);
    assert_eq!(
        data.entries[0].accounted,
        f.0.metadata().unwrap().blocks() * 512 + a.allocated
    );
}

#[test]
fn t06_symlink_loop_and_external_target_are_not_traversed() {
    let f = Fixture::new();
    let outside = Fixture::new();
    fs::write(outside.0.join("secret"), b"not scanned").unwrap();
    symlink(&f.0, f.0.join("loop")).unwrap();
    symlink(&outside.0, f.0.join("outside")).unwrap();
    let data = f.run();
    assert_eq!(data.entries.len(), 3);
    assert!(data.entries[1..]
        .iter()
        .all(|e| e.kind == EntryKind::Symlink));
    assert_eq!(data.status, ScanStatus::Complete);
    assert_eq!(fs::read(outside.0.join("secret")).unwrap(), b"not scanned");
    assert!(scan(
        &f.0.join("outside"),
        &ScanOptions::default(),
        &AtomicBool::new(false)
    )
    .is_err());
}

#[test]
fn t08_permission_error_is_partial_not_zero_success() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let f = Fixture::new();
    let protected = f.0.join("protected");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("unreadable"), b"data").unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0)).unwrap();
    let result = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(false));
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o700)).unwrap();
    let data = result.unwrap();
    assert_eq!(data.status, ScanStatus::Partial);
    assert!(!data.entries[0].complete);
    assert_eq!(data.issues[0].code, "OPEN_DIRECTORY_ERROR");
    assert!(data.report(30)["totals"]["logical_bytes"].is_null());
    let protected_id = data
        .entries
        .iter()
        .position(|e| e.name == b"protected")
        .unwrap();
    assert!(data.entry_json(protected_id as u32)["allocated_bytes"].is_null());
}

#[test]
fn t10_known_provider_root_is_excluded_before_enumeration() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("Library/CloudStorage/provider")).unwrap();
    fs::write(
        f.0.join("Library/CloudStorage/provider/should-not-appear"),
        b"x",
    )
    .unwrap();
    let data = f.run();
    assert_eq!(data.status, ScanStatus::Partial);
    assert_eq!(data.excluded_count, 1);
    assert!(!data.entries.iter().any(|e| e.name == b"should-not-appear"));
}

#[test]
fn t10_ancestor_alias_into_provider_is_excluded_before_enumeration() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("Library/CloudStorage/provider")).unwrap();
    fs::write(
        f.0.join("Library/CloudStorage/provider/should-not-appear"),
        b"x",
    )
    .unwrap();
    symlink(f.0.join("Library/CloudStorage"), f.0.join("alias")).unwrap();
    let data = scan(
        &f.0.join("alias/provider"),
        &ScanOptions::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(data.entries.len(), 1);
    assert_eq!(data.issues[0].code, "CLOUD_EXCLUDED");
}

#[test]
fn t12_hostile_names_survive_real_filesystem_and_json() {
    let f = Fixture::new();
    let name = std::ffi::OsString::from("æøå🙂\n<img src=x>\x1b[31m");
    fs::write(f.0.join(&name), b"hello").unwrap();
    let data = f.run();
    let encoded = data.report(30).to_string();
    let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert!(decoded["records"][0]["path_bytes_hex"]
        .as_str()
        .unwrap()
        .contains("0a3c696d67"));
    assert_eq!(data.entries[1].name, name.into_vec());
}

#[test]
fn t12_raw_bytes_survive_synthetic_serialization() {
    // APFS rejects invalid UTF-8 on creation (EILSEQ); exercise parser/output
    // with synthetic raw metadata rather than pretending the OS created it.
    let f = Fixture::new();
    fs::write(f.0.join("a"), b"x").unwrap();
    let mut data = f.run();
    data.entries[1].name = b"\xff\n<img>".to_vec();
    let report = data.entry_json(1);
    assert!(report["path_bytes_hex"]
        .as_str()
        .unwrap()
        .ends_with("ff0a3c696d673e"));
    let _: serde_json::Value = serde_json::from_str(&report.to_string()).unwrap();
}

#[test]
fn explicit_file_root_and_exact_entry_limit_work() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), b"abc").unwrap();
    let file = scan(
        &f.0.join("a"),
        &ScanOptions::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(file.entries.len(), 1);
    assert_eq!(file.entries[0].logical, 3);
    let exact = scan(
        &f.0,
        &ScanOptions {
            max_entries: 2,
            ..ScanOptions::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(exact.status, ScanStatus::Complete);
}

#[test]
fn t13_cancelled_result_is_never_complete() {
    let f = Fixture::new();
    let data = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(true)).unwrap();
    assert_eq!(data.status, ScanStatus::Cancelled);
    assert!(!data.entries[0].complete);
}

#[test]
fn t14_ids_do_not_collide_within_one_process() {
    let f = Fixture::new();
    assert_ne!(f.run().id, f.run().id);
}

#[test]
fn entry_limit_is_partial_and_metrics_do_not_overflow() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), b"a").unwrap();
    let options = ScanOptions {
        max_entries: 1,
        ..ScanOptions::default()
    };
    let data = scan(&f.0, &options, &AtomicBool::new(false)).unwrap();
    assert_eq!(data.status, ScanStatus::Partial);
    assert_eq!(data.issues[0].code, "SCAN_LIMIT");
}

#[test]
fn unfinished_nested_directories_are_unknown_after_a_limit() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("nested/a"), b"x").unwrap();
    let data = scan(
        &f.0,
        &ScanOptions {
            max_entries: 2,
            ..ScanOptions::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!data.entries[1].complete);
    assert!(data.entry_json(1)["logical_bytes"].is_null());
}

#[test]
fn t18_pages_are_bounded_lossless_and_bound_to_query() {
    let f = Fixture::new();
    for id in 0..123 {
        fs::write(f.0.join(format!("file-{id:03}")), b"abc").unwrap();
    }
    let data = f.run();
    let mut query = Query {
        limit: 7,
        metric: Metric::Logical,
        ..Query::default()
    };
    let mut seen = std::collections::HashSet::new();
    loop {
        let result = data.query(&query).unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < OUTPUT_BUDGET);
        for record in result["records"].as_array().unwrap() {
            assert!(seen.insert(record["entry_id"].as_str().unwrap().to_owned()));
        }
        if result["next_cursor"].is_null() {
            break;
        }
        query.cursor = Some(result["next_cursor"].as_str().unwrap().into());
        let wrong = Query {
            metric: Metric::Allocated,
            cursor: query.cursor.clone(),
            ..Query::default()
        };
        assert!(data.query(&wrong).is_err());
    }
    assert_eq!(seen.len(), 123);
    assert!(data
        .query(&Query {
            metric: Metric::Private,
            ..Query::default()
        })
        .is_err());
}

#[test]
fn t18_output_budget_returns_a_cursor_not_a_truncated_locator() {
    let f = Fixture::new();
    for id in 0..100 {
        fs::write(f.0.join(format!("{id:03}{}", "long-name".repeat(20))), b"x").unwrap();
    }
    let result = f
        .run()
        .query(&Query {
            limit: 500,
            ..Query::default()
        })
        .unwrap();
    assert!(serde_json::to_vec(&result).unwrap().len() < OUTPUT_BUDGET);
    assert_eq!(result["truncated"], true);
    assert!(result["next_cursor"].as_str().is_some());
    assert!(!result["records"].as_array().unwrap().is_empty());
}

#[test]
fn t14_t15_store_is_opt_in_private_atomic_and_does_not_overwrite() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let path = workspace.0.join("store");
    assert!(!path.exists());
    fs::write(f.0.join("a"), b"abcdef").unwrap();
    let data = f.run();
    let store = Store::open(&path, true).unwrap();
    store.save(&data, STORE_BUDGET).unwrap();
    let retained = fs::read(path.join(&data.id).join("entries.jsonl")).unwrap();
    assert!(store.save(&data, STORE_BUDGET).is_err());
    assert_eq!(
        fs::read(path.join(&data.id).join("entries.jsonl")).unwrap(),
        retained
    );
    assert!(fs::read_dir(&path).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(
        path.join(&data.id)
            .join("manifest.json")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let reloaded = store.load(&data.id).unwrap();
    assert_eq!(
        data.query(&Query::default()).unwrap(),
        reloaded.query(&Query::default()).unwrap()
    );
    assert_eq!(store.ids().unwrap(), vec![data.id]);
}

#[test]
fn t15_exhausted_budget_preserves_old_scans_and_cleans_only_own_stage() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let path = workspace.0.join("store");
    let store = Store::open(&path, true).unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    let after = f.run();
    assert!(store.save(&after, 1).is_err());
    let used = fs::metadata(path.join(&before.id).join("manifest.json"))
        .unwrap()
        .len()
        + fs::metadata(path.join(&before.id).join("entries.jsonl"))
            .unwrap()
            .len()
        + fs::metadata(path.join(&before.id).join("paths.jsonl"))
            .unwrap()
            .len();
    assert!(store.save(&after, used + 1).is_err());
    assert!(store.load(&before.id).is_ok());
    assert_eq!(store.ids().unwrap(), vec![before.id]);
    assert!(!path.join(format!("{}.partial", after.id)).exists());
}

#[test]
fn t13_cancelled_save_never_publishes_a_partial_scan() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let path = workspace.0.join("store");
    let store = Store::open(&path, true).unwrap();
    let data = f.run();
    let error = store
        .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(true))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn t26_corrupt_and_oversize_metadata_is_rejected_without_modification() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let path = workspace.0.join("store");
    let store = Store::open(&path, true).unwrap();
    let data = f.run();
    store.save(&data, STORE_BUDGET).unwrap();
    let rows = path.join(&data.id).join("entries.jsonl");
    let corrupt = vec![b'x'; 65537];
    fs::write(&rows, &corrupt).unwrap();
    assert!(store.load(&data.id).is_err());
    assert_eq!(fs::read(&rows).unwrap(), corrupt);
    assert!(store.load("../../outside").is_err());
}

#[test]
fn t21_store_inside_scan_is_explicitly_excluded() {
    let f = Fixture::new();
    let path = f.0.join("store");
    let _store = Store::open(&path, true).unwrap();
    let data = scan(
        &f.0,
        &ScanOptions {
            exclusions: vec![path],
            ..ScanOptions::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(data.status, ScanStatus::Partial);
    assert_eq!(data.issues[0].code, "OUTPUT_EXCLUDED");
}

#[test]
fn query_largest_only_returns_files_in_selected_subtree() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("nested/b"), b"small").unwrap();
    fs::write(f.0.join("a"), [1_u8; 100]).unwrap();
    let data = f.run();
    let nested = data
        .entries
        .iter()
        .position(|e| e.name == b"nested")
        .unwrap();
    let result = data
        .query(&Query {
            kind: QueryKind::Largest,
            entry: nested as u32,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(result["records"].as_array().unwrap().len(), 1);
    assert_eq!(result["records"][0]["name_display"], "b");
}

#[test]
fn t27_directory_symlink_swap_never_reads_external_tree() {
    let f = Fixture::new();
    let outside = Fixture::new();
    fs::write(outside.0.join("outside-only"), b"never traversed").unwrap();
    fs::create_dir(f.0.join("slot")).unwrap();
    let root = f.0.clone();
    let external = outside.0.clone();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            fs::rename(root.join("slot"), root.join("parked")).unwrap();
            symlink(&external, root.join("slot")).unwrap();
            std::thread::yield_now();
            fs::remove_file(root.join("slot")).unwrap();
            fs::rename(root.join("parked"), root.join("slot")).unwrap();
        }
    });
    let mut escaped = false;
    for _ in 0..100 {
        let data = f.run();
        escaped |= data.entries.iter().any(|e| e.name == b"outside-only");
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(!escaped);
    assert_eq!(
        fs::read(outside.0.join("outside-only")).unwrap(),
        b"never traversed"
    );
}

#[test]
fn t16_compare_reports_exact_logical_deltas_and_respects_allocated_metric() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::write(f.0.join("grow"), b"123").unwrap();
    fs::write(f.0.join("shrink"), b"1234567890").unwrap();
    fs::write(f.0.join("removed"), b"1234").unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    fs::write(f.0.join("grow"), b"12345678").unwrap();
    fs::write(f.0.join("shrink"), b"12").unwrap();
    fs::remove_file(f.0.join("removed")).unwrap();
    fs::write(f.0.join("added"), b"123456").unwrap();
    let after = f.run();
    store.save(&after, STORE_BUDGET).unwrap();
    let logical = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery {
                metric: Metric::Logical,
                limit: 500,
                cursor: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(logical["totals"]["delta_bytes"], "-1");
    let records = logical["records"].as_array().unwrap();
    let find = |name: &str| {
        records
            .iter()
            .find(|r| {
                r["path_display"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("/{name}"))
            })
            .unwrap()
    };
    assert_eq!(find("grow")["delta_bytes"], "5");
    assert_eq!(find("shrink")["delta_bytes"], "-8");
    assert_eq!(find("removed")["change"], "removed");
    assert_eq!(find("added")["before_bytes"], "0");
    assert_eq!(find("removed")["after_bytes"], "0");
    let allocated = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        allocated["totals"]["delta_bytes"],
        (after.entries[0].accounted as i128 - before.entries[0].accounted as i128).to_string()
    );
    assert_eq!(allocated["metric"], "allocated");
}

#[test]
fn t17_partial_coverage_is_not_reported_as_deletions() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    fs::create_dir(f.0.join("a")).unwrap();
    fs::write(f.0.join("a/keep"), b"retained").unwrap();
    // Punctuation sibling exposes incorrect raw-path (rather than component)
    // ordering/ancestor tracking in streaming comparisons.
    fs::write(f.0.join("a!"), b"sibling").unwrap();
    fs::write(f.0.join("known-remove"), b"gone").unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    fs::remove_file(f.0.join("known-remove")).unwrap();
    fs::set_permissions(f.0.join("a"), fs::Permissions::from_mode(0)).unwrap();
    let result = scan(&f.0, &ScanOptions::default(), &AtomicBool::new(false));
    fs::set_permissions(f.0.join("a"), fs::Permissions::from_mode(0o700)).unwrap();
    let after = result.unwrap();
    store.save(&after, STORE_BUDGET).unwrap();
    let compared = store
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
    assert_eq!(compared["status"], "partial");
    assert!(compared["totals"]["delta_bytes"].is_null());
    let records = compared["records"].as_array().unwrap();
    let unknown = records
        .iter()
        .find(|r| r["path_display"].as_str().unwrap().ends_with("/a/keep"))
        .unwrap();
    assert_eq!(unknown["change"], "not_observed_or_incomplete");
    assert!(unknown["after_bytes"].is_null());
    assert!(unknown["delta_bytes"].is_null());
    let removed = records
        .iter()
        .find(|r| {
            r["path_display"]
                .as_str()
                .unwrap()
                .ends_with("/known-remove")
        })
        .unwrap();
    assert_eq!(removed["change"], "removed");
    assert!(!records
        .iter()
        .any(|r| r["path_display"].as_str().unwrap().ends_with("/a!") && r["change"] == "removed"));
}

#[test]
fn t17_compare_rejects_different_root_policy_os_or_volume() {
    let f = Fixture::new();
    let other = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    let different_root = other.run();
    store.save(&different_root, STORE_BUDGET).unwrap();
    assert_eq!(
        store
            .compare(
                &before.id,
                &different_root.id,
                &CompareQuery::default(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    for variation in ["policy", "os", "volume"] {
        let mut after = f.run();
        match variation {
            "policy" => after.policy.push_str("different"),
            "os" => after.context["os_identity"] = serde_json::json!("different"),
            _ => after.context["volume_before"]["volume_uuid"] = serde_json::json!("different"),
        };
        store.save(&after, STORE_BUDGET).unwrap();
        assert_eq!(
            store
                .compare(
                    &before.id,
                    &after.id,
                    &CompareQuery::default(),
                    &AtomicBool::new(false)
                )
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
    }
}

#[test]
fn t18_compare_pages_are_stable_and_have_no_duplicates_or_loss() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    for n in 0..100 {
        fs::write(f.0.join(format!("f{n:03}")), b"x").unwrap();
    }
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    for n in 0..100 {
        fs::write(f.0.join(format!("f{n:03}")), vec![b'x'; n + 2]).unwrap();
    }
    let after = f.run();
    store.save(&after, STORE_BUDGET).unwrap();
    let mut query = CompareQuery {
        metric: Metric::Logical,
        limit: 7,
        cursor: None,
    };
    let mut seen = std::collections::HashSet::new();
    let mut deltas = Vec::new();
    loop {
        let result = store
            .compare(&before.id, &after.id, &query, &AtomicBool::new(false))
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() <= OUTPUT_BUDGET);
        for row in result["records"].as_array().unwrap() {
            assert!(seen.insert(row["path_bytes_hex"].as_str().unwrap().to_owned()));
            deltas.push(row["delta_bytes"].as_str().unwrap().parse::<i64>().unwrap());
        }
        if result["next_cursor"].is_null() {
            break;
        }
        query.cursor = Some(result["next_cursor"].as_str().unwrap().into());
        assert!(store
            .compare(
                &before.id,
                &after.id,
                &CompareQuery {
                    metric: Metric::Allocated,
                    limit: 7,
                    cursor: query.cursor.clone()
                },
                &AtomicBool::new(false)
            )
            .is_err());
    }
    assert_eq!(seen.len(), 100);
    assert_eq!(deltas, (1..=100).rev().collect::<Vec<_>>());
}

#[test]
fn t26_valid_json_corruption_is_rejected_by_metadata_checksums() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let path = workspace.0.join("store");
    let store = Store::open(&path, true).unwrap();
    fs::write(f.0.join("a"), b"123").unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    fs::write(f.0.join("a"), b"1234").unwrap();
    let after = f.run();
    store.save(&after, STORE_BUDGET).unwrap();
    let index = path.join(&after.id).join("paths.jsonl");
    let original = fs::read_to_string(&index).unwrap();
    let tampered = original.replace("\"4\"", "\"9\"");
    assert_ne!(original, tampered);
    fs::write(&index, &tampered).unwrap();
    assert_eq!(
        store
            .compare(
                &before.id,
                &after.id,
                &CompareQuery::default(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(fs::read_to_string(&index).unwrap(), tampered);
    let entries = path.join(&before.id).join("entries.jsonl");
    let original = fs::read_to_string(&entries).unwrap();
    let tampered = original.replace("\"3\"", "\"9\"");
    assert_ne!(original, tampered);
    fs::write(&entries, &tampered).unwrap();
    assert_eq!(
        store.load(&before.id).err().unwrap().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn t13_streaming_compare_obeys_cancellation() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    let store = Store::open(&workspace.0.join("store"), true).unwrap();
    let before = f.run();
    store.save(&before, STORE_BUDGET).unwrap();
    let after = f.run();
    store.save(&after, STORE_BUDGET).unwrap();
    let e = store
        .compare(
            &before.id,
            &after.id,
            &CompareQuery::default(),
            &AtomicBool::new(true),
        )
        .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted);
}
