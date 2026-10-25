use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = PathBuf::from(format!(
                "/private/tmp/macscan-cli-{}-{}",
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
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_macscan"));
        c.env("HOME", &self.0);
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn json(out: &Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn no_default_scan_or_implicit_history() {
    let f = Fixture::new();
    assert!(json(&f.cmd().output().unwrap())["commands"].is_array());
    fs::write(f.0.join("a"), b"12345").unwrap();
    let result = json(&f.cmd().arg("scan").arg(&f.0).output().unwrap());
    assert_eq!(result["totals"]["logical_bytes"], "5");
    assert_eq!(result["retained"], false);
    assert!(!f.0.join("Library").exists());
    let missing = f.cmd().arg("scan").output().unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
}

#[test]
fn saved_scan_round_trips_through_queries_and_cursor() {
    let f = Fixture::new();
    let root = f.0.join("target");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), b"12345").unwrap();
    fs::write(root.join("b"), b"12").unwrap();
    let store = f.0.join("store");
    let scanned = json(
        &f.cmd()
            .arg("scan")
            .arg(&root)
            .arg("--save")
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let id = scanned["scan_id"].as_str().unwrap();
    let summary = json(
        &f.cmd()
            .args(["summary", "--scan", id, "--metric", "logical", "--store"])
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(summary["totals"]["logical_bytes"], "7");
    let first = json(
        &f.cmd()
            .args([
                "children", "--scan", id, "--metric", "logical", "--limit", "1", "--store",
            ])
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(first["records"][0]["name_display"], "a");
    let second = json(
        &f.cmd()
            .args([
                "children",
                "--scan",
                id,
                "--metric",
                "logical",
                "--cursor",
                first["next_cursor"].as_str().unwrap(),
                "--store",
            ])
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(second["records"][0]["name_display"], "b");
    assert_eq!(second["truncated"], false);
    assert_eq!(
        store.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn unsupported_metrics_and_unknown_flags_are_never_ignored() {
    let f = Fixture::new();
    for (args, exit) in [
        (vec!["scan", "/not-a-real-root", "--metric", "private"], 6),
        (vec!["scan", "/not-a-real-root", "--typo", "yes"], 2),
        (vec!["scan", "/not-a-real-root"], 4),
    ] {
        let out = f.cmd().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(exit));
        assert!(out.stdout.is_empty());
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert!(error["error"]["code"].as_str().is_some());
    }
}

#[test]
fn ndjson_stdout_and_progress_stderr_are_distinct() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), b"x").unwrap();
    let out = f
        .cmd()
        .arg("scan")
        .arg(&f.0)
        .args(["--format", "ndjson", "--progress", "ndjson"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let lines: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.first().unwrap()["type"], "start");
    assert_eq!(lines.last().unwrap()["type"], "end");
    assert!(lines.iter().any(|v| v["type"] == "record"));
    let progress: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(progress.iter().all(|v| v["type"] == "progress"));
    assert_eq!(progress.last().unwrap()["finished"], true);
}

#[test]
fn ndjson_framing_also_respects_the_output_budget() {
    let f = Fixture::new();
    let workspace = Fixture::new();
    for id in 0..100 {
        fs::write(f.0.join(format!("{id:03}{}", "name".repeat(40))), b"x").unwrap();
    }
    let out = f
        .cmd()
        .arg("scan")
        .arg(&f.0)
        .args(["--limit", "100", "--format", "ndjson"])
        .arg("--save")
        .arg("--store")
        .arg(workspace.0.join("store"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.len() <= 8192);
    let lines: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.last().unwrap()["truncated"], true);
    assert!(lines.last().unwrap()["next_cursor"].as_str().is_some());
}

#[test]
fn partial_scan_has_code_five_and_queries_preserve_coverage() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), b"x").unwrap();
    let store = f.0.join("store");
    let out = f
        .cmd()
        .arg("scan")
        .arg(&f.0)
        .args(["--max-entries", "1", "--save", "--store"])
        .arg(&store)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["status"], "partial");
    assert!(result["totals"]["logical_bytes"].is_null());
    let summary = json(
        &f.cmd()
            .args([
                "summary",
                "--scan",
                result["scan_id"].as_str().unwrap(),
                "--store",
            ])
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(summary["coverage"]["complete"], false);
}

#[test]
fn t13_broken_pipe_does_not_panic_or_start_background_work() {
    let f = Fixture::new();
    let mut child = f
        .cmd()
        .arg("schema")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "BROKEN_PIPE");
}

#[test]
fn volumes_and_compare_are_available_without_a_gui_or_runtime_service() {
    let f = Fixture::new();
    let volumes = json(&f.cmd().args(["volumes", "--limit", "1"]).output().unwrap());
    assert!(volumes["records"].as_array().unwrap().len() <= 1);
    assert_eq!(volumes["records"][0]["free_space_sum_allowed"], false);
    let root = f.0.join("target");
    fs::create_dir(&root).unwrap();
    let store = f.0.join("store");
    fs::write(root.join("a"), b"123").unwrap();
    let before = json(
        &f.cmd()
            .arg("scan")
            .arg(&root)
            .args(["--save", "--store"])
            .arg(&store)
            .output()
            .unwrap(),
    );
    fs::write(root.join("a"), b"12345678").unwrap();
    let after = json(
        &f.cmd()
            .arg("scan")
            .arg(&root)
            .args(["--save", "--store"])
            .arg(&store)
            .output()
            .unwrap(),
    );
    let out = f
        .cmd()
        .args([
            "compare",
            before["scan_id"].as_str().unwrap(),
            after["scan_id"].as_str().unwrap(),
            "--metric",
            "logical",
            "--store",
        ])
        .arg(&store)
        .output()
        .unwrap();
    let compared = json(&out);
    assert_eq!(compared["totals"]["delta_bytes"], "5");
    assert_eq!(compared["records"][0]["delta_bytes"], "5");
    assert!(out.stdout.len() <= 8192);
    let sql = json(&f.cmd()
        .args(["query", "--scan", after["scan_id"].as_str().unwrap(),
            "--before", before["scan_id"].as_str().unwrap(), "--sql",
            "SELECT path_display,logical_delta FROM changes WHERE kind='file' ORDER BY path_key",
            "--limit", "1", "--store"])
        .arg(&store).output().unwrap());
    assert_eq!(sql["records"][0]["logical_delta"], "5");
    assert_eq!(
        sql["records"][0]["path_display"],
        root.join("a").to_str().unwrap()
    );
    let incompatible = f
        .cmd()
        .args([
            "compare",
            after["scan_id"].as_str().unwrap(),
            before["scan_id"].as_str().unwrap(),
            "--store",
        ])
        .arg(&store)
        .output()
        .unwrap();
    assert_eq!(incompatible.status.code(), Some(6));
    assert!(incompatible.stdout.is_empty());
}

#[test]
fn relative_provider_root_is_excluded_before_directory_enumeration() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("Library/CloudStorage/provider")).unwrap();
    fs::write(
        f.0.join("Library/CloudStorage/provider/should-not-appear"),
        b"x",
    )
    .unwrap();
    let out = f
        .cmd()
        .current_dir(&f.0)
        .args(["scan", "Library/CloudStorage/provider"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["coverage"]["entries_seen"], 1);
    assert!(result["totals"]["logical_bytes"].is_null());
}
