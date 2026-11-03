//! Real subprocess contract checks against bounded, owned fixtures only.
use macscan_analysis::Store;
use macscan_engine::{Entry, EntryKind, Scan, ScanStatus, STORE_BUDGET};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

// fork/exec can briefly inherit another thread's flock descriptor before CLOEXEC.
// Keep API publications and child creation serial inside this integration binary.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "macscan-contract-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn cmd(&self) -> Command {
        Command::new(env!("CARGO_BIN_EXE_macscan"))
    }
    fn store(&self) -> PathBuf {
        self.0.join("store")
    }
    fn saved(&self, id: &str, partial: bool, count: usize) {
        let entry = |parent, name, kind, complete| Entry {
            parent,
            name,
            kind,
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
            clone_metadata: Default::default(),
            complete,
            enumeration_complete: complete,
        };
        let mut entries = vec![
            entry(None, vec![], EntryKind::Directory, !partial),
            entry(
                Some(0),
                b"complete-child".to_vec(),
                EntryKind::Directory,
                true,
            ),
        ];
        for n in 0..count {
            entries.push(entry(
                Some(1),
                format!("{n:03}{}", "long-name-".repeat(12)).into_bytes(),
                EntryKind::File,
                true,
            ));
        }
        let scan = Scan {
            id: id.into(),
            root: PathBuf::from("/synthetic/contract"),
            entries,
            issues: vec![],
            issue_count: 0,
            excluded_count: 0,
            status: if partial {
                ScanStatus::Partial
            } else {
                ScanStatus::Complete
            },
            started_at: "2026-11-03T00:00:00Z".into(),
            ended_at: "2026-11-03T00:00:01Z".into(),
            duration_ms: 1,
            policy: "synthetic".into(),
            context: json!({"os_identity":"synthetic", "started_unix_ns":id.split('-').next().unwrap(),
                "ended_unix_ns":"9999999999999999999", "volume_before":{"volume_uuid":"synthetic"}, "volume_after":{"volume_uuid":"synthetic"}}),
        };
        Store::open(&self.store(), true)
            .unwrap()
            .save_with_cancel(&scan, STORE_BUDGET, &AtomicBool::new(false))
            .unwrap();
    }
    fn read(&self, args: &[&str], exit: i32) -> Value {
        let out = self
            .cmd()
            .args(args)
            .arg("--store")
            .arg(self.store())
            .output()
            .unwrap();
        response(&out, exit)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn response(out: &Output, exit: i32) -> Value {
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.len() <= 8192);
    assert!(out.stderr.len() <= 8192);
    let bytes = if exit == 0 || exit == 10 {
        assert!(out.stderr.is_empty());
        &out.stdout
    } else {
        assert!(out.stdout.is_empty());
        &out.stderr
    };
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(value["schema_version"], "3.0");
    assert_eq!(
        value["outcome"],
        match exit {
            0 => "ok",
            10 => "partial",
            130 => "cancelled",
            _ => "failed",
        }
    );
    value
}
#[test]
fn subprocess_exit_matrix_and_typed_busy() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    for format in ["json", "ndjson"] {
        for (extra, exit, code) in [
            (vec!["--typo", "yes"], 2, "INVALID_ARGUMENT"),
            (vec!["--workers", "2"], 6, "UNSUPPORTED_WORKERS"),
            (vec!["--metadata-budget", "1"], 5, "METADATA_LIMIT"),
        ] {
            let out = f
                .cmd()
                .arg("scan")
                .arg(&root)
                .args(extra)
                .args(["--format", format])
                .output()
                .unwrap();
            let v = response(&out, exit);
            assert_eq!(v["error"]["code"], code);
            assert_eq!(v["error"]["retryable"], false);
            if format == "ndjson" {
                assert_eq!(v["type"], "error");
            }
        }
        response(
            &f.cmd()
                .arg("scan")
                .arg(f.0.join("missing"))
                .args(["--format", format])
                .output()
                .unwrap(),
            4,
        );
        let file = f.0.join("file");
        fs::write(&file, b"x").unwrap();
        let io = response(
            &f.cmd()
                .arg("scan")
                .arg(&root)
                .arg("--output")
                .arg(file.join("nested"))
                .args(["--format", format])
                .output()
                .unwrap(),
            3,
        );
        assert_eq!(io["error"]["retryable"], false);
        let store = Store::open(&f.store(), true).unwrap();
        let guard = store.begin_run().unwrap();
        for command in ["scan", "history-prune"] {
            let mut c = f.cmd();
            c.arg(command);
            if command == "scan" {
                c.arg(&root).arg("--save");
            }
            let out = c
                .arg("--store")
                .arg(f.store())
                .args(["--format", format])
                .output()
                .unwrap();
            let v = response(&out, 7);
            assert_eq!(v["error"]["code"], "HISTORY_BUSY");
            assert_eq!(v["error"]["retryable"], true);
            assert!(v["error"]["hint"].is_string());
        }
        drop(guard);
    }
}
#[test]
fn selected_complete_subtree_and_whole_measurement_outcomes_are_distinct() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    f.saved("100-1", true, 2);
    for command in [
        "summary", "children", "largest", "inspect", "profile", "insights",
    ] {
        let root = f.read(&[command, "--scan", "100-1"], 10);
        assert_eq!(root["outcome"], "partial");
        let child = f.read(&[command, "--scan", "100-1", "--entry", "1"], 0);
        assert_eq!(child["measurement_status"], "partial");
        assert_eq!(child["outcome"], "ok");
    }
    let diagnostic = f.read(&["coverage", "--scan", "100-1"], 10);
    assert_eq!(diagnostic["scope"], "whole_measurement");
    f.read(
        &[
            "query",
            "--scan",
            "100-1",
            "--sql",
            "SELECT NULL AS unknown",
        ],
        10,
    );
    f.read(&["scans"], 0);
    let nd = f
        .cmd()
        .args([
            "summary", "--scan", "100-1", "--format", "ndjson", "--store",
        ])
        .arg(f.store())
        .output()
        .unwrap();
    assert_eq!(nd.status.code(), Some(10));
    assert!(nd.stderr.is_empty());
    for frame in nd.stdout.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let value: Value = serde_json::from_slice(frame).unwrap();
        assert_eq!(value["schema_version"], "3.0");
        assert_eq!(value["outcome"], "partial");
    }
    let sql = format!("SELECT \"{}\"", "\u{0001}".repeat(6000));
    for format in ["json", "ndjson"] {
        let out = f
            .cmd()
            .args([
                "query", "--scan", "100-1", "--sql", &sql, "--format", format, "--store",
            ])
            .arg(f.store())
            .output()
            .unwrap();
        let value = response(&out, 2);
        assert_eq!(value["error"]["detail_truncated"], true);
    }
}
#[test]
fn comparison_and_sql_consider_both_participating_measurements() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    f.saved("100-1", true, 1);
    f.saved("200-1", false, 1);
    f.saved("300-1", true, 1);
    for (before, after) in [("100-1", "200-1"), ("200-1", "300-1")] {
        let pair = f.read(&["compare", before, after], 10);
        assert_eq!(pair["coverage"]["complete"], false);
        f.read(
            &[
                "query",
                "--scan",
                after,
                "--before",
                before,
                "--sql",
                "SELECT NULL AS unknown",
            ],
            10,
        );
        f.read(&["growth", before, after], 10);
        f.read(&["growth", before, after, "--entry", "1"], 0);
    }
    // A null SQL value in a complete measurement is not evidence of partial coverage.
    f.read(
        &[
            "query",
            "--scan",
            "200-1",
            "--sql",
            "SELECT NULL AS unknown",
        ],
        0,
    );
}
#[test]
fn ndjson_long_row_pages_preserve_every_record_and_exact_values() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    f.saved("100-1", false, 80);
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let mut c = f.cmd();
        c.args([
            "children", "--scan", "100-1", "--entry", "1", "--limit", "100", "--format", "ndjson",
            "--store",
        ])
        .arg(f.store());
        if let Some(cursor) = &cursor {
            c.arg("--cursor").arg(cursor);
        }
        let out = c.output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stderr.is_empty());
        assert!(out.stdout.len() <= 8192);
        let lines: Vec<Value> = out
            .stdout
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        let start = &lines[0];
        let end = lines.last().unwrap();
        for envelope in [start, end] {
            assert_eq!(envelope["schema_version"], "3.0");
            assert_eq!(envelope["outcome"], "ok");
        }
        for row in &lines[1..lines.len() - 1] {
            assert_eq!(row["record"]["bytes"], "4096");
            assert!(seen.insert(row["record"]["entry_id"].as_str().unwrap().to_owned()));
        }
        pages += 1;
        assert!(pages < 81);
        cursor = end["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 80);
    assert!(pages > 1);
}
#[test]
fn published_scan_identity_survives_stdout_failure_and_retention_still_applies() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    for id in ["100-1", "200-1", "300-1"] {
        f.saved(id, false, 1);
    }
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    for saved in [true, false] {
        let mut c = f.cmd();
        c.arg("scan")
            .arg(&root)
            .arg("--store")
            .arg(f.store())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if saved {
            c.arg("--save");
        }
        let mut child = c.spawn().unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let error = response(&out, 3);
        assert_eq!(error["error"]["code"], "BROKEN_PIPE");
        assert_eq!(error["retained"], saved);
        let id = error["scan_id"].as_str().unwrap();
        assert_eq!(f.store().join(id).exists(), saved);
    }
    assert!(!f.store().join("100-1").exists());
    assert!(f.store().join("200-1").exists());
    assert!(f.store().join("300-1").exists());
}
#[test]
fn pruning_warning_keeps_published_scan_successful() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    Store::open(&f.store(), true).unwrap();
    fs::write(f.store().join("999-1.machine.json"), b"{}").unwrap();
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    for n in 0..80 {
        fs::write(
            root.join(format!("{n:03}{}", "long-name-".repeat(12))),
            b"synthetic",
        )
        .unwrap();
    }
    let out = f
        .cmd()
        .arg("scan")
        .arg(root)
        .args(["--limit", "100"])
        .arg("--output")
        .arg(f.store())
        .output()
        .unwrap();
    let value = response(&out, 0);
    assert_eq!(value["retained"], true);
    assert!(value["retention_warning"].is_string());
    assert!(f.store().join(value["scan_id"].as_str().unwrap()).exists());
    let id = value["scan_id"].as_str().unwrap().to_owned();
    let mut page = value;
    let mut seen = BTreeSet::new();
    loop {
        for row in page["records"].as_array().unwrap() {
            assert!(seen.insert(row["entry_id"].as_str().unwrap().to_owned()));
        }
        match page["next_cursor"].as_str() {
            Some(cursor) => {
                page = f.read(
                    &[
                        "children", "--scan", &id, "--limit", "100", "--cursor", cursor,
                    ],
                    0,
                )
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), 80);
}

#[test]
fn sigint_before_publication_does_not_evict_history() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    for id in ["100-1", "200-1", "300-1"] {
        f.saved(id, false, 1);
    }
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    for n in 0..2000 {
        fs::write(root.join(format!("{n:04}")), b"synthetic").unwrap();
    }
    let mut child = f
        .cmd()
        .arg("scan")
        .arg(&root)
        .args([
            "--save",
            "--progress",
            "ndjson",
            "--format",
            "ndjson",
            "--store",
        ])
        .arg(f.store())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    // Initial progress is emitted after signal-handler installation and before traversal.
    reader.read_line(&mut line).unwrap();
    let progress: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(progress["type"], "progress");
    assert_eq!(progress["finished"], false);
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() >= Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("owned scan did not observe SIGINT within 15 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    assert!(out.stdout.is_empty());
    let mut errors = String::new();
    std::io::Read::read_to_string(&mut reader, &mut errors).unwrap();
    let last: Value = serde_json::from_str(errors.lines().last().unwrap()).unwrap();
    assert_eq!(last["outcome"], "cancelled");
    assert_eq!(last["schema_version"], "3.0");
    assert_eq!(last["type"], "error");
    for id in ["100-1", "200-1", "300-1"] {
        assert!(f.store().join(id).exists());
    }
    assert_eq!(fs::read_dir(f.store()).unwrap().count(), 3);
}
