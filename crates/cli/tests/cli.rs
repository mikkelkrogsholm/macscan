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
fn full_scan_is_default_and_explicit_large_limits_are_supported() {
    let f=Fixture::new();
    let schema=json(&f.cmd().arg("schema").output().unwrap());
    assert!(schema["limits"]["max_entries"].is_null());
    assert!(schema["limits"]["max_duration_seconds"].is_null());
    let out=f.cmd().args(["scan","/not-a-real-root","--max-entries","2000000"]).output().unwrap();
    assert_eq!(out.status.code(),Some(4));
}

#[test]
fn finder_style_256_file_limit_does_not_cut_off_a_full_scan(){
    use std::os::unix::process::CommandExt;
    let f=Fixture::new();let root=f.0.join("root");fs::create_dir(&root).unwrap();let mut p=root.clone();
    for _ in 0..300{p=p.join("d");fs::create_dir(&p).unwrap();}fs::write(p.join("tiny"),b"abc").unwrap();
    let mut cmd=f.cmd();cmd.arg("scan").arg(&root);
    // SAFETY: child-only resource syscalls, no allocation or shared-state mutation.
    unsafe {cmd.pre_exec(||{let mut limit=std::mem::MaybeUninit::<libc::rlimit>::uninit();if libc::getrlimit(libc::RLIMIT_NOFILE,limit.as_mut_ptr())!=0{return Err(std::io::Error::last_os_error());}let mut limit=limit.assume_init();limit.rlim_cur=256.min(limit.rlim_max);if libc::setrlimit(libc::RLIMIT_NOFILE,&limit)!=0{return Err(std::io::Error::last_os_error());}Ok(())});}
    let out=json(&cmd.output().unwrap());assert_eq!(out["status"],"complete");assert_eq!(out["totals"]["logical_bytes"],"3");
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

#[test]
fn contextual_classification_reaches_schema_navigation_and_sql() {
    let f=Fixture::new();let root=f.0.join("root");fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("Cargo.toml"),b"synthetic, not parsed").unwrap();fs::write(root.join("target/object"),b"x").unwrap();
    let store=f.0.join("history");let store_arg=store.to_str().unwrap();
    let scanned=json(&f.cmd().arg("scan").arg(&root).args(["--save","--store",store_arg]).output().unwrap());
    let id=scanned["scan_id"].as_str().unwrap();
    let answer=json(&f.cmd().args(["children","--scan",id,"--store",store_arg]).output().unwrap());
    let target=answer["records"].as_array().unwrap().iter().find(|r|r["name_display"]=="target").unwrap();
    assert_eq!(target["category_rule_id"],"rust_target");assert_eq!(target["category_rule_version"],"context-1");
    let entry=target["entry_id"].as_str().unwrap();
    let inspect=json(&f.cmd().args(["inspect","--scan",id,"--entry",entry,"--store",store_arg]).output().unwrap());
    let sql=format!("SELECT category,category_rule_id,category_rule_version,category_reason,category_coverage,category_evidence_source FROM entries_classified([{entry}])");
    let query=json(&f.cmd().args(["query","--scan",id,"--store",store_arg,"--sql",&sql]).output().unwrap());
    for field in ["category","category_rule_id","category_rule_version","category_reason","category_coverage","category_evidence_source"] {
        assert_eq!(target[field],inspect["records"][0][field]);assert_eq!(target[field],query["records"][0][field]);
    }
    let schema=json(&f.cmd().arg("schema").output().unwrap());assert_eq!(schema["category_contract"]["version"],"context-1");
}

#[test]
fn insights_routes_schema_scope_categories_and_validation(){
 let f=Fixture::new();let root=f.0.join("root");fs::create_dir_all(root.join("target")).unwrap();fs::write(root.join("Cargo.toml"),b"x").unwrap();fs::write(root.join("target/a"),b"12345").unwrap();let store=f.0.join("store");let saved=json(&f.cmd().arg("scan").arg(&root).arg("--save").arg("--store").arg(&store).output().unwrap());let id=saved["scan_id"].as_str().unwrap();
 let schema=json(&f.cmd().arg("schema").output().unwrap());assert_eq!(schema["insights_contract"]["version"],"insights-1");
 let page=json(&f.cmd().args(["insights","--scan",id,"--metric","logical","--limit","1"]).arg("--store").arg(&store).output().unwrap());assert_eq!(page["records"][0]["category"],"build_output");assert_eq!(page["records"][0]["bytes"],"5");assert!(page["next_cursor"].is_string());
 let details=json(&f.cmd().args(["insights","--scan",id,"--category","build_output","--metric","logical","--entry","0"]).arg("--store").arg(&store).output().unwrap());assert_eq!(details["mode"],"candidates");assert!(details["records"][0]["entry_id"].is_string());
 let out=f.cmd().args(["insights","--scan",id,"--category","build_output","--format","ndjson"]).arg("--store").arg(&store).output().unwrap();assert_eq!(out.status.code(),Some(0));assert!(out.stdout.len()<=8192);for line in String::from_utf8(out.stdout).unwrap().lines(){assert!(serde_json::from_str::<Value>(line).is_ok());}
 for flags in [vec!["--category","junk"],vec!["--limit","0"],vec!["--entry","99999"],vec!["--cursor","bad"]]{let out=f.cmd().args(["insights","--scan",id]).args(flags).arg("--store").arg(&store).output().unwrap();assert!(!out.status.success());}
 assert_eq!(f.cmd().arg("insights").output().unwrap().status.code(),Some(2));
}
#[test]
fn growth_routes_schema_scope_filters_cursor_and_ndjson(){
 let f=Fixture::new();let root=f.0.join("root");fs::create_dir_all(root.join("project/target")).unwrap();fs::write(root.join("project/Cargo.toml"),b"x").unwrap();fs::write(root.join("project/target/a"),b"123").unwrap();let store=f.0.join("history");
 let a=json(&f.cmd().arg("scan").arg(&root).args(["--save","--store"]).arg(&store).output().unwrap());fs::write(root.join("project/target/a"),b"12345678").unwrap();fs::write(root.join("project/new"),b"12345").unwrap();let b=json(&f.cmd().arg("scan").arg(&root).args(["--save","--store"]).arg(&store).output().unwrap());let before=a["scan_id"].as_str().unwrap();let after=b["scan_id"].as_str().unwrap();
 let schema=json(&f.cmd().arg("schema").output().unwrap());assert_eq!(schema["growth_contract"]["version"],"growth-1");
 let page=json(&f.cmd().args(["growth",before,after,"--metric","logical","--grouping","category","--limit","1","--entry","0","--view","groups","--change","all"]).arg("--store").arg(&store).output().unwrap());assert_eq!(page["command"],"growth");assert_eq!(page["rule_version"],"insights-1");assert_eq!(page["contributions"]["delta_bytes"],"10");assert!(page["next_cursor"].is_string());
 let next=json(&f.cmd().args(["growth",before,after,"--metric","logical","--grouping","category","--cursor",page["next_cursor"].as_str().unwrap()]).arg("--store").arg(&store).output().unwrap());assert_eq!(next["records"].as_array().unwrap().len(),1);
 let detail=json(&f.cmd().args(["growth",before,after,"--metric","logical","--grouping","category","--view","files","--group","build_output","--change","grown"]).arg("--store").arg(&store).output().unwrap());assert_eq!(detail["records"][0]["delta_bytes"],"5");assert_eq!(detail["records"][0]["drilldown_scan_id"],after);
 let nd=f.cmd().args(["growth",before,after,"--format","ndjson"]).arg("--store").arg(&store).output().unwrap();assert!(nd.status.success());assert!(nd.stdout.len()<=8192);for line in String::from_utf8(nd.stdout).unwrap().lines(){assert!(serde_json::from_str::<Value>(line).is_ok());}
 for flags in [vec!["--grouping","junk"],vec!["--view","junk"],vec!["--change","junk"],vec!["--limit","0"],vec!["--entry","99999"],vec!["--cursor","wrong"]]{assert!(!f.cmd().args(["growth",before,after]).args(flags).arg("--store").arg(&store).output().unwrap().status.success());}assert_eq!(f.cmd().arg("growth").output().unwrap().status.code(),Some(2));
}

#[test]
fn coverage_command_reports_saved_whole_measurement_in_json_and_ndjson(){
    let f=Fixture::new();let source=f.0.join("source");fs::create_dir(&source).unwrap();let store=source.join("history");
    let out=f.cmd().args(["scan"]).arg(&source).arg("--output").arg(&store).output().unwrap();assert_eq!(out.status.code(),Some(5));
    let saved:Value=serde_json::from_slice(&out.stdout).unwrap();let id=saved["scan_id"].as_str().unwrap();
    for format in ["json","ndjson"] {
        let out=f.cmd().args(["coverage","--scan",id,"--reason","scanner_output","--format",format,"--store"]).arg(&store).output().unwrap();assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));assert!(out.stdout.len()<=8192);
        let start:Value=serde_json::from_slice(if format=="json"{&out.stdout}else{out.stdout.split(|b|*b==b'\n').next().unwrap()}).unwrap();assert_eq!(start["scope"],"whole_measurement");assert_eq!(start["reason_counts"]["scanner_output"],"1");assert_eq!(start["quality"],"event_counts");
    }
    let rejected=f.cmd().args(["coverage","--scan",id,"--entry","0","--store"]).arg(&store).output().unwrap();assert_eq!(rejected.status.code(),Some(2));
    let mut m:Value=serde_json::from_slice(&fs::read(store.join(id).join("manifest.json")).unwrap()).unwrap();m.as_object_mut().unwrap().remove("coverage_diagnostics");fs::write(store.join(id).join("manifest.json"),serde_json::to_vec(&m).unwrap()).unwrap();
    let legacy=json(&f.cmd().args(["coverage","--scan",id,"--store"]).arg(&store).output().unwrap());assert_eq!(legacy["quality"],"legacy_samples_only");assert!(legacy["reason_counts"].is_null());assert_eq!(legacy["error_count"],"1");
}

#[test]
fn apfs_data_clone_requires_explicit_scan_capture_and_routes_saved_consumers() {
    let f=Fixture::new();let root=f.0.join("root");fs::create_dir(&root).unwrap();fs::write(root.join("file.bin"),vec![3;4096]).unwrap();let history=f.0.join("history");
    let reject=f.cmd().args(["scan","/does-not-exist","--metric","apfs-data-clone"]).output().unwrap();
    assert_eq!(reject.status.code(),Some(2));assert!(String::from_utf8_lossy(&reject.stderr).contains("--capture-apfs-clones"));assert!(!history.exists());
    let off=json(&f.cmd().arg("scan").arg(&root).arg("--output").arg(&history).output().unwrap());
    let on=json(&f.cmd().arg("scan").arg(&root).arg("--output").arg(&history).args(["--capture-apfs-clones","--metric","apfs-data-clone"]).output().unwrap());
    assert_eq!(on["metric"],"apfs-data-clone");assert_eq!(on["apfs_data_clone"]["algorithm_version"],"apfs-data-clone-1");
    for command in ["summary","children","largest","inspect","insights","profile"] {
        let out=f.cmd().args([command,"--scan",on["scan_id"].as_str().unwrap(),"--metric","apfs-data-clone"]).arg("--store").arg(&history).output().unwrap();
        assert!(out.stdout.len()<=8192);let value=json(&out);assert_eq!(value["metric"],"apfs-data-clone");
        assert_eq!(value["apfs_data_clone"]["label"],"Begrænset DATA-klonkorrigeret allokering");
    }
    let out=f.cmd().args(["summary","--scan",off["scan_id"].as_str().unwrap(),"--metric","apfs-data-clone"]).arg("--store").arg(&history).output().unwrap();
    assert_eq!(out.status.code(),Some(6));assert!(String::from_utf8_lossy(&out.stderr).contains("rescan"));
    let schema=json(&f.cmd().arg("schema").output().unwrap());assert!(schema["options"]["metric"].as_array().unwrap().iter().any(|v|v=="apfs-data-clone"));
}
