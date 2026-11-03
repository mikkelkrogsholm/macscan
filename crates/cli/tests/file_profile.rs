//! CLI parity using synthetic saved metadata, without scanning the filesystem.
use macscan_analysis::{FileProfileQuery, Store, OUTPUT_BUDGET};
use macscan_engine::{Entry, EntryKind, Metric, Scan, ScanStatus, STORE_BUDGET};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command, sync::atomic::AtomicBool};
#[test]
fn profile_cli_matches_shared_api_and_bounded_encodings() {
    let path = std::env::temp_dir().join(format!("macscan-profile-cli-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    let store = Store::open(&path, false).unwrap();
    let file = |parent, name, kind, size| Entry {
        clone_metadata: macscan_engine::CloneMetadata::default(),
        parent,
        name,
        kind,
        logical: size,
        allocated: size,
        accounted: size,
        own_allocated: size,
        device: 1,
        inode: 1,
        links: 1,
        observed_links: 1,
        charged_to: None,
        mtime: 0,
        complete: true,
        enumeration_complete: true,
    };
    let mut entries = vec![file(None, vec![], EntryKind::Directory, 0)];
    for n in 0..35 {
        entries.push(file(
            Some(0),
            format!("saved-{n:02}.PDF").into_bytes(),
            EntryKind::File,
            n,
        ));
    }
    let scan = Scan {
        id: "502-1".into(),
        root: PathBuf::from("/synthetic/cli"),
        entries,
        issues: vec![],
        issue_count: 0,
        excluded_count: 0,
        status: ScanStatus::Complete,
        started_at: "2026-10-30T23:59:59Z".into(),
        ended_at: "2026-10-31T00:00:00Z".into(),
        duration_ms: 1000,
        policy: "synthetic".into(),
        context: json!({"ended_unix_ns":"1793404800000000000"}),
    };
    let cancel = AtomicBool::new(false);
    store
        .save_with_cancel(&scan, STORE_BUDGET, &cancel)
        .unwrap();
    let args = [
        "--scan",
        "502-1",
        "--metric",
        "logical",
        "--min-age-days",
        "1",
        "--min-bytes",
        "2",
        "--extension",
        "PDF",
        "--limit",
        "100",
    ];
    for types in [false, true] {
        let q = FileProfileQuery {
            metric: Metric::Logical,
            min_age_days: Some(1),
            min_bytes: Some(2),
            type_key: Some("ext:706466".into()),
            types,
            sort_count: types,
            limit: 100,
            ..Default::default()
        };
        let mut shared = store.file_profile("502-1", &q, &cancel).unwrap();
        assert_eq!(shared["schema_version"], "2.0");
        shared["schema_version"] = json!("3.0");
        shared["outcome"] = json!("ok");
        let mut cursor = None;
        let mut seen = 0;
        loop {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_macscan"));
            cmd.arg("profile").arg("--store").arg(&path).args(args);
            if types {
                cmd.args(["--view", "types", "--sort", "count"]);
            }
            if let Some(c) = &cursor {
                cmd.arg("--cursor").arg(c);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.stdout.len() <= OUTPUT_BUDGET);
            let page: Value = serde_json::from_slice(&out.stdout).unwrap();
            if cursor.is_none() {
                assert_eq!(page, shared);
            }
            seen += page["records"].as_array().unwrap().len();
            if let Some(c) = page["next_cursor"].as_str() {
                cursor = Some(c.to_owned());
            } else {
                break;
            }
        }
        assert_eq!(seen, if types { 1 } else { 33 });
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_macscan"));
        cmd.arg("profile")
            .arg("--store")
            .arg(&path)
            .args(args)
            .args(["--format", "ndjson"]);
        if types {
            cmd.args(["--view", "types", "--sort", "count"]);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success());
        assert!(out.stdout.len() <= OUTPUT_BUDGET);
        let lines: Vec<Value> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(lines[0]["type"], "start");
        assert_eq!(lines.last().unwrap()["type"], "end");
        assert_eq!(lines[0]["totals"], shared["totals"]);
    }
    for flags in [vec!["--no-extension"], vec!["--type-key", "ext:706466"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_macscan"))
            .arg("profile")
            .arg("--store")
            .arg(&path)
            .args(["--scan", "502-1"])
            .args(flags)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    for flags in [
        vec!["--extension", "pdf", "--no-extension"],
        vec!["--sort", "count"],
        vec!["--entry", "999"],
        vec!["--min-bytes", "-1"],
        vec!["--type-key", "ext:ZZ"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_macscan"))
            .arg("profile")
            .arg("--store")
            .arg(&path)
            .args(["--scan", "502-1"])
            .args(flags)
            .output()
            .unwrap();
        assert!(!out.status.success());
    }
    fs::remove_dir_all(path).unwrap();
}
