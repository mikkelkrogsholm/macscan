//! Exercise installed discovery against the real executable, without user history.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "macscan-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_macscan"));
        c.current_dir(&self.0);
        c
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn object(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 8192);
    serde_json::from_slice(&output.stdout).unwrap()
}
fn error(output: &Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.len() <= 8192);
    let value: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema_version"], "3.0");
    assert_eq!(value["outcome"], "failed");
    value
}
fn sample(flag: &str, command: &str) -> Option<&'static str> {
    Some(match flag {
        "--save" | "--capture-apfs-clones" | "--no-extension" | "--dry-run" | "--apply" => {
            return None
        }
        "--format" => "json",
        "--command" => "scan",
        "--scan" | "--before" => "missing",
        "--cursor" => "opaque-invalid-cursor",
        "--metric" => "logical",
        "--entry" | "--offset" | "--min-bytes" | "--min-age-days" => "0",
        "--limit" | "--max-entries" | "--max-duration" | "--workers" => "1",
        "--memory-budget" => "262144",
        "--metadata-budget" => "4294967296",
        "--progress" => "off",
        "--search" => "needle",
        "--kind" => "file",
        "--extension" => "txt",
        "--type-key" => "none",
        "--view" => {
            if command == "profile" {
                "files"
            } else {
                "groups"
            }
        }
        "--sort" => "bytes",
        "--grouping" => "area",
        "--change" => "all",
        "--group" => "build_output",
        "--category" => "build_output",
        "--reason" => "scanner_output",
        "--sql" => "SELECT 1",
        _ => panic!("missing test value for advertised flag {flag}"),
    })
}
#[test]
fn installed_catalog_and_help_are_bounded_consistent_and_have_no_store_side_effects() {
    let f = Fixture::new();
    let full = object(&f.cmd().env_remove("HOME").arg("schema").output().unwrap());
    assert!(full["columns"].as_array().unwrap().len() > 30);
    let caps = object(
        &f.cmd()
            .env_remove("HOME")
            .arg("capabilities")
            .output()
            .unwrap(),
    );
    let version = object(
        &f.cmd()
            .env_remove("HOME")
            .arg("--version")
            .output()
            .unwrap(),
    );
    assert_eq!(full["identity"], caps["identity"]);
    assert_eq!(caps["identity"], version["identity"]);
    assert_eq!(caps["schema_version"], "3.0");
    assert_eq!(caps["identity"]["data_schema_version"], "2.0");
    assert!(caps["identity"]["build_revision"].as_str().unwrap().len() >= 7);
    assert_eq!(caps["scanned_files_cleanup"], false);
    assert_eq!(caps["metadata_retention"]["keep_runs"], 3);
    assert!(caps.get("implementation_stage").is_none());
    assert!(caps.get("warnings").is_none());
    assert!(caps["apfs_data_clone_limitations"]
        .as_str()
        .unwrap()
        .contains("not unique/private/reclaimable"));
    assert_eq!(caps["commands"], full["commands"]);
    for name in ["schema", "capabilities", "--version", "--help", "-h"] {
        let output = f
            .cmd()
            .env_remove("HOME")
            .args([name, "--format", "ndjson"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.len() <= 8192);
        let lines: Vec<Value> = output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(lines.first().unwrap()["type"], "start");
        assert_eq!(lines.last().unwrap()["type"], "end");
        assert_eq!(lines[0]["identity"], version["identity"]);
    }

    for name in full["commands"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        let scoped = object(
            &f.cmd()
                .env_remove("HOME")
                .args(["schema", "--command", name])
                .output()
                .unwrap(),
        );
        let help = object(
            &f.cmd()
                .env_remove("HOME")
                .args([name, "--help"])
                .output()
                .unwrap(),
        );
        assert_eq!(scoped, help);
        assert_eq!(help["command"], name);
        assert!(help.get("columns").is_none());
        assert!(help.get("query_views").is_none());
        assert_eq!(help["identity"], version["identity"]);
        assert!(help["options"].is_object());
        for args in [
            vec![name, "--help", "--format", "ndjson"],
            vec!["schema", "--command", name, "--format", "ndjson"],
        ] {
            let output = f.cmd().env_remove("HOME").args(args).output().unwrap();
            assert!(output.status.success());
            assert!(output.stdout.len() <= 8192);
            let lines: Vec<Value> = output
                .stdout
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::from_slice(l).unwrap())
                .collect();
            assert_eq!(lines.first().unwrap()["type"], "start");
            assert_eq!(lines.last().unwrap()["type"], "end");
            assert_eq!(lines[0]["schema_version"], "3.0");
        }
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
    for args in [
        vec!["unknown"],
        vec!["unknown", "--help"],
        vec!["schema", "--command", "unknown"],
        vec!["schema", "--command", "scan", "--typo", "x"],
        vec!["scan", "--help", "--store", "/missing"],
    ] {
        let e = error(&f.cmd().env_remove("HOME").args(args).output().unwrap(), 2);
        assert_eq!(e["error"]["code"], "INVALID_ARGUMENT");
    }
}
#[test]
fn each_advertised_option_passes_runtime_applicability_and_unadvertised_options_fail() {
    let f = Fixture::new();
    let missing = f.0.join("missing-history");
    let missing = missing.to_str().unwrap();
    let full = object(&f.cmd().arg("schema").output().unwrap());
    let mut all = std::collections::BTreeSet::new();
    let mut descriptions = Vec::new();
    for name in full["commands"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        let help = object(&f.cmd().args([name, "--help"]).output().unwrap());
        for flag in help["options"].as_object().unwrap().keys() {
            all.insert(flag.clone());
        }
        descriptions.push((name.to_owned(), help));
    }
    for (name, help) in descriptions {
        let flags = help["options"].as_object().unwrap();
        for flag in &all {
            let mut cmd = f.cmd();
            cmd.arg(&name);
            if flags.contains_key("--store") {
                cmd.args(["--store", missing]);
            }
            match name.as_str() {
                "scan" => {
                    cmd.arg(f.0.join("missing-root"));
                }
                "compare" | "growth" => {
                    cmd.args(["missing-before", "missing-after"]);
                }
                _ => {}
            }
            if flags.contains_key("--scan") {
                cmd.args(["--scan", "missing"]);
            }
            if name == "query" {
                cmd.args(["--sql", "SELECT 1"]);
            }
            cmd.arg(flag);
            if matches!(flag.as_str(), "--store" | "--output") {
                cmd.arg(missing);
            } else if let Some(v) = sample(flag, &name) {
                cmd.arg(v);
            }
            let output = cmd.output().unwrap();
            if flags.contains_key(flag) {
                if output.status.success() {
                    object(&output);
                } else {
                    let e: Value = serde_json::from_slice(&output.stderr).unwrap();
                    assert_ne!(
                        e["error"]["operation"], "arguments",
                        "advertised {name} {flag}: {e}"
                    );
                }
            } else {
                error(&output, 2);
            }
        }
    }
}
#[test]
fn advertised_ranges_and_mutual_exclusions_are_enforced_before_execution() {
    let f = Fixture::new();
    for flags in [
        vec!["--limit", "0"],
        vec!["--limit", "101"],
        vec!["--max-entries", "0"],
        vec!["--max-duration", "0"],
        vec!["--memory-budget", "262143"],
        vec!["--memory-budget", "268435457"],
        vec!["--progress", "bad"],
        vec!["--metadata-budget", "-1"],
        vec!["--metric", "apfs-data-clone"],
    ] {
        error(
            &f.cmd()
                .args(["scan", "missing"])
                .args(flags)
                .output()
                .unwrap(),
            2,
        );
    }
    for flags in [
        vec!["--memory-budget", "262144"],
        vec!["--memory-budget", "268435456"],
        vec!["--max-entries", "18446744073709551615"],
        vec!["--max-duration", "18446744073709551615"],
        vec!["--workers", "1"],
    ] {
        error(
            &f.cmd()
                .arg("scan")
                .arg(f.0.join("missing"))
                .args(flags)
                .output()
                .unwrap(),
            4,
        );
    }
    assert_eq!(
        error(
            &f.cmd()
                .args(["scan", "missing", "--workers", "2"])
                .output()
                .unwrap(),
            6
        )["error"]["code"],
        "UNSUPPORTED_WORKERS"
    );
    for flags in [
        vec!["--extension", "txt", "--no-extension"],
        vec!["--type-key", "none", "--extension", "txt"],
        vec!["--no-extension", "--type-key", "none"],
        vec!["--entry", "4294967296"],
        vec!["--view", "bad"],
        vec!["--sort", "bad"],
    ] {
        error(
            &f.cmd()
                .args(["profile", "--scan", "missing"])
                .args(flags)
                .output()
                .unwrap(),
            2,
        );
    }
    error(
        &f.cmd()
            .args(["history-prune", "--apply", "--dry-run"])
            .output()
            .unwrap(),
        2,
    );
    error(
        &f.cmd().args(["scans", "--run", "future"]).output().unwrap(),
        2,
    );
    error(&f.cmd().args(["scan", "--whole-mac"]).output().unwrap(), 2);
}
#[test]
fn copied_binary_discovery_needs_no_repository_home_or_path_tools() {
    let f = Fixture::new();
    let executable = f.0.join("macscan");
    fs::copy(env!("CARGO_BIN_EXE_macscan"), &executable).unwrap();
    // The bundled runtime library travels beside the executable in release packaging.
    let target = PathBuf::from(env!("CARGO_BIN_EXE_macscan"));
    fs::copy(
        target
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("duckdb/libduckdb.dylib"),
        f.0.join("libduckdb.dylib"),
    )
    .unwrap();
    let expected = object(&f.cmd().arg("--version").output().unwrap());
    for args in [
        vec!["--version"],
        vec!["capabilities"],
        vec!["schema"],
        vec!["schema", "--command", "scan"],
        vec!["scan", "--help"],
    ] {
        let actual = object(
            &Command::new(&executable)
                .current_dir(&f.0)
                .env_remove("HOME")
                .env("PATH", "")
                .args(args)
                .output()
                .unwrap(),
        );
        assert_eq!(actual["identity"], expected["identity"]);
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);
}

#[test]
fn help_tokens_in_values_or_after_delimiter_keep_their_operand_meaning() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("--help")).unwrap();
    fs::write(f.0.join("--help/file"), b"abc").unwrap();
    let value = object(&f.cmd().args(["scan", "--", "--help"]).output().unwrap());
    assert_eq!(value["command"], "scan");
    assert_eq!(value["totals"]["logical_bytes"], "3");
    assert_eq!(value["retained"], false);
    let missing = f.0.join("missing-history");
    let e = error(
        &f.cmd()
            .args(["query", "--scan", "missing", "--sql", "--help", "--store"])
            .arg(&missing)
            .output()
            .unwrap(),
        4,
    );
    assert_eq!(e["error"]["operation"], "open_store");
    error(
        &f.cmd()
            .args(["schema", "--command", "--help"])
            .output()
            .unwrap(),
        2,
    );
}
