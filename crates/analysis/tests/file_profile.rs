use macscan_analysis::{encode_response, FileProfileQuery, Store, OUTPUT_BUDGET};
use macscan_engine::{Entry, EntryKind, Metric, Scan, ScanStatus, STORE_BUDGET};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
const C: AtomicBool = AtomicBool::new(false);
const REF: i64 = 1_793_404_800; // 2026-10-31T00:00:00Z
fn entry(parent: Option<u32>, name: &[u8], kind: EntryKind, bytes: u64, time: i64) -> Entry {
    Entry {
        clone_metadata: macscan_engine::CloneMetadata::default(),
        parent,
        name: name.to_vec(),
        kind,
        logical: bytes,
        allocated: bytes,
        accounted: bytes,
        own_allocated: bytes,
        device: 1,
        inode: 1,
        links: 1,
        observed_links: 1,
        charged_to: None,
        mtime: time,
        complete: true,
        enumeration_complete: true,
    }
}
fn synthetic() -> Scan {
    let mut entries = vec![
        entry(None, b"synthetic", EntryKind::Directory, 0, 0),
        entry(Some(0), b"old-directory", EntryKind::Directory, 0, 0),
    ];
    for (name, size, time) in [
        (b"new.PDF".as_slice(), 10, REF - 1),
        (b"boundary.pdf", 20, REF - 86_400),
        (b"near.pdf", 30, REF - 86_399),
        (b"zero", 0, 0),
        (b".hidden", 0, -1),
        (b"a.b.", 1, 0),
        (b".hidden.TXT", 2, 0),
        (b"future.txt", 3, REF + 1),
        (b"raw.\xff", 4, 0),
        (b"raw.\xfe", 5, 0),
    ] {
        entries.push(entry(Some(1), name, EntryKind::File, size, time));
    }
    entries.push(entry(
        Some(0),
        b"old-directory-sibling.pdf",
        EntryKind::File,
        100,
        0,
    ));
    let mut link = entry(Some(1), b"hardlink.pdf", EntryKind::File, 20, 0);
    link.accounted = 0;
    link.charged_to = Some(3);
    link.links = 2;
    entries.push(link);
    entries.push(entry(Some(0), b"symlink.pdf", EntryKind::Symlink, 999, 0));
    entries.push(entry(Some(0), b"huge.a", EntryKind::File, u64::MAX, 0));
    entries.push(entry(Some(0), b"huge.b", EntryKind::File, u64::MAX, 0));
    for n in 0..45 {
        entries.push(entry(
            Some(1),
            format!("tiny-{n}.type{n}").as_bytes(),
            EntryKind::File,
            0,
            0,
        ));
    }
    Scan {
        id: "500-1".into(),
        root: PathBuf::from("/synthetic/file-profile"),
        entries,
        issues: vec![],
        issue_count: 0,
        excluded_count: 0,
        status: ScanStatus::Complete,
        started_at: "2026-10-30T23:59:59Z".into(),
        ended_at: "2026-10-31T00:00:00Z".into(),
        duration_ms: 1000,
        policy: "synthetic".into(),
        context: json!({"started_unix_ns":((REF as i128-1)*1_000_000_000).to_string(),"ended_unix_ns":(REF as i128*1_000_000_000).to_string()}),
    }
}
fn store(label: &str) -> (Store, PathBuf) {
    let path = std::env::temp_dir().join(format!("macscan-profile-{}-{label}", std::process::id()));
    fs::create_dir(&path).unwrap();
    (Store::open(&path, false).unwrap(), path)
}
fn q() -> FileProfileQuery {
    FileProfileQuery {
        metric: Metric::Logical,
        ..Default::default()
    }
}
fn all(store: &Store, mut q: FileProfileQuery) -> Vec<Value> {
    let mut result = vec![];
    loop {
        let page = store.file_profile("500-1", &q, &C).unwrap();
        for nd in [false, true] {
            assert!(encode_response(page.clone(), nd).unwrap().len() <= OUTPUT_BUDGET);
        }
        result.extend(page["records"].as_array().unwrap().iter().cloned());
        if let Some(c) = page["next_cursor"].as_str() {
            q.cursor = Some(c.into());
        } else {
            break;
        }
    }
    result
}
#[test]
fn saved_file_age_type_scope_and_exact_accounting() {
    let (store, path) = store("semantics");
    let s = synthetic();
    store.save_with_cancel(&s, STORE_BUDGET, &C).unwrap();
    let old = all(
        &store,
        FileProfileQuery {
            entry: 1,
            min_age_days: Some(1),
            type_key: Some(FileProfileQuery::extension_key("PDF").unwrap()),
            ..q()
        },
    );
    assert_eq!(old.len(), 2);
    assert!(old.iter().any(|r| r["entry_id"] == "3"));
    assert!(!old
        .iter()
        .any(|r| r["entry_id"] == "2" || r["entry_id"] == "4"));
    let p = store
        .file_profile("500-1", &FileProfileQuery { entry: 1, ..q() }, &C)
        .unwrap();
    assert_eq!(
        p["reference_unix_ns"],
        (REF as i128 * 1_000_000_000).to_string()
    );
    assert_eq!(p["population"]["future_mtime_count"], "1");
    assert_eq!(p["population"]["file_path_count"], "56");
    let epoch = all(
        &store,
        FileProfileQuery {
            entry: 1,
            min_age_days: Some(1),
            type_key: Some("none".into()),
            ..q()
        },
    );
    assert_eq!(epoch.len(), 3);
    assert!(epoch.iter().any(|r| r["mtime_unix_seconds"] == "0"));
    assert!(epoch.iter().any(|r| r["mtime_unix_seconds"] == "-1"));
    let types = all(
        &store,
        FileProfileQuery {
            entry: 1,
            types: true,
            sort_count: true,
            limit: 100,
            ..q()
        },
    );
    assert_eq!(types[0]["type_key"], "ext:706466");
    assert_eq!(types[0]["file_path_count"], "4");
    assert!(types
        .iter()
        .any(|r| r["type_key"] == "none" && r["file_path_count"] == "3"));
    let nonutf: Vec<_> = types
        .iter()
        .filter(|r| r["type_key"] == "ext:FF" || r["type_key"] == "ext:FE")
        .collect();
    assert_eq!(nonutf.len(), 2);
    assert_eq!(nonutf[0]["type_display"], nonutf[1]["type_display"]);
    let raw = all(
        &store,
        FileProfileQuery {
            type_key: Some("ext:FF".into()),
            ..q()
        },
    );
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0]["entry_id"], "10");
    let root = store.file_profile("500-1", &q(), &C).unwrap();
    assert_eq!(
        root["totals"]["bytes"],
        (2 * u64::MAX as u128 + 195).to_string()
    );
    let alloc = store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                entry: 13,
                ..Default::default()
            },
            &C,
        )
        .unwrap();
    assert_eq!(alloc["totals"]["bytes"], "0");
    let zero = all(
        &store,
        FileProfileQuery {
            entry: 1,
            min_bytes: Some(1),
            ..q()
        },
    );
    assert_eq!(zero.len(), 9);
    assert!(store
        .file_profile("500-1", &FileProfileQuery { entry: 9999, ..q() }, &C)
        .is_err());
    assert!(store
        .file_profile("500-1", &q(), &AtomicBool::new(true))
        .is_err());
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn pagination_both_views_and_cursor_binding() {
    let (store, path) = store("pages");
    store
        .save_with_cancel(&synthetic(), STORE_BUDGET, &C)
        .unwrap();
    for types in [false, true] {
        let query = FileProfileQuery {
            types,
            limit: 100,
            ..q()
        };
        let a = all(&store, query.clone());
        let b = all(
            &store,
            FileProfileQuery {
                limit: 1,
                ..query.clone()
            },
        );
        assert_eq!(a, b);
        let p = store.file_profile("500-1", &query, &C).unwrap();
        let cursor = p["next_cursor"].as_str().unwrap().to_owned();
        for changed in [
            FileProfileQuery {
                min_bytes: Some(0),
                ..query.clone()
            },
            FileProfileQuery {
                types: !types,
                ..query.clone()
            },
            FileProfileQuery {
                entry: 1,
                ..query.clone()
            },
            FileProfileQuery {
                min_age_days: Some(1),
                ..query.clone()
            },
            FileProfileQuery {
                metric: Metric::Allocated,
                ..query.clone()
            },
            FileProfileQuery {
                type_key: Some("none".into()),
                ..query.clone()
            },
        ] {
            assert!(store
                .file_profile(
                    "500-1",
                    &FileProfileQuery {
                        cursor: Some(cursor.clone()),
                        ..changed
                    },
                    &C
                )
                .is_err());
        }
    }
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn partial_root_complete_child_and_missing_saved_reference() {
    let (store, path) = store("partial");
    let mut s = synthetic();
    s.status = ScanStatus::Partial;
    s.entries[0].complete = false;
    s.entries[0].enumeration_complete = false;
    store.save_with_cancel(&s, STORE_BUDGET, &C).unwrap();
    let p = store.file_profile("500-1", &q(), &C).unwrap();
    assert_eq!(p["coverage"]["complete"], false);
    assert!(p["totals"]["bytes"].is_null());
    let p = store
        .file_profile("500-1", &FileProfileQuery { entry: 1, ..q() }, &C)
        .unwrap();
    assert_eq!(p["coverage"]["complete"], true);
    assert!(p["totals"]["bytes"].is_string());
    let manifest = path.join("500-1/manifest.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    m["context"]["ended_unix_ns"] = Value::Null;
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                min_age_days: Some(1),
                ..q()
            },
            &C
        )
        .unwrap_err()
        .to_string()
        .contains("reference time unavailable"));
    assert!(store.file_profile("500-1", &q(), &C).unwrap()["reference_unix_ns"].is_null());
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn retain_native_synthetic_history_when_requested() {
    let Ok(path) = std::env::var("MACSCAN_PROFILE_EVIDENCE") else {
        return;
    };
    let store = Store::open(&PathBuf::from(path), true).unwrap();
    let mut s = synthetic();
    // Native UI evidence has internally consistent directory sizes, without the
    // intentionally overflowing HUGEINT test rows used by the API regression.
    s.entries
        .retain(|e| e.name != b"huge.a" && e.name != b"huge.b");
    let downloads = s.entries.len() as u32;
    s.entries.push(entry(
        Some(0),
        b"Downloads",
        EntryKind::Directory,
        300 * 1024 * 1024,
        0,
    ));
    s.entries.push(entry(
        Some(downloads),
        b"old-archive.ZIP",
        EntryKind::File,
        100 * 1024 * 1024,
        REF - 181 * 86_400,
    ));
    s.entries.push(entry(
        Some(downloads),
        b"new-archive.ZIP",
        EntryKind::File,
        200 * 1024 * 1024,
        REF - 1,
    ));
    for (i, logical, accounted) in [
        (0, 300 * 1024 * 1024 + 1194, 300 * 1024 * 1024 + 1174),
        (1, 95, 75),
    ] {
        s.entries[i].logical = logical;
        s.entries[i].allocated = logical;
        s.entries[i].accounted = accounted;
        s.entries[i].own_allocated = 0;
    }
    s.entries[downloads as usize].own_allocated = 0;
    for (i, e) in s.entries.iter_mut().enumerate() {
        e.inode = i as u64 + 1;
    }
    s.entries[3].links = 2;
    s.entries[3].observed_links = 2;
    s.entries[13].inode = s.entries[3].inode;
    s.entries[13].observed_links = 2;
    s.context["os_identity"] = json!("synthetic");
    s.context["volume_before"] = json!({"volume_uuid":"synthetic-profile-volume"});
    s.context["volume_after"] = s.context["volume_before"].clone();
    store.save_with_cancel(&s, STORE_BUDGET, &C).unwrap();
    let mut p = s;
    p.id = "500-2".into();
    p.status = ScanStatus::Partial;
    p.entries[0].complete = false;
    p.entries[0].enumeration_complete = false;
    store.save_with_cancel(&p, STORE_BUDGET, &C).unwrap();
}

#[test]
fn saved_regular_file_root_uses_raw_basename() {
    let (store, path) = store("file-root");
    let mut s = synthetic();
    s.root = PathBuf::from("/synthetic/archive.PDF");
    s.entries = vec![entry(None, b"", EntryKind::File, 17, 0)];
    store.save_with_cancel(&s, STORE_BUDGET, &C).unwrap();
    let p = store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                type_key: Some("ext:706466".into()),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert_eq!(p["totals"]["file_path_count"], "1");
    assert_eq!(p["records"][0]["type_key"], "ext:706466");
    assert_eq!(p["records"][0]["drilldown_entry_id"], "0");
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn zero_reference_and_fractional_threshold_use_exact_saved_integers() {
    let (store, path) = store("reference");
    let mut s = synthetic();
    s.context["ended_unix_ns"] = json!("0");
    store.save_with_cancel(&s, STORE_BUDGET, &C).unwrap();
    let page = store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                entry: 5,
                min_age_days: Some(0),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert_eq!(page["reference_unix_ns"], "0");
    assert_eq!(page["age_threshold_unix_ns"], "0");
    assert_eq!(page["totals"]["file_path_count"], "1");
    let manifest = path.join("500-1/manifest.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    m["context"]["ended_unix_ns"] = json!((REF as i128 * 1_000_000_000 - 1).to_string());
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    let page = store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                entry: 3,
                min_age_days: Some(1),
                ..q()
            },
            &C,
        )
        .unwrap();
    assert_eq!(page["totals"]["file_path_count"], "0");
    let hardlink = store
        .file_profile(
            "500-1",
            &FileProfileQuery {
                entry: 13,
                min_bytes: Some(1),
                ..Default::default()
            },
            &C,
        )
        .unwrap();
    assert_eq!(hardlink["totals"]["file_path_count"], "0");
    fs::remove_dir_all(path).unwrap();
}
