use macscan_analysis::Store;
use serde_json::Value;
use std::{fs, process::Command};
#[test]
fn saved_cli_retains_three_and_explicit_total_rejects() {
    let base = std::env::temp_dir().join(format!("macscan-retention-cli-{}", std::process::id()));
    fs::create_dir(&base).unwrap();
    let history = base.join("history");
    let call = |args: &[&std::ffi::OsStr]| {
        let out = Command::new(env!("CARGO_BIN_EXE_macscan"))
            .args(args)
            .output()
            .unwrap();
        assert!(out.stdout.len() <= 8192);
        out
    };
    let mut ids = vec![];
    for n in 0..4 {
        let root = base.join(format!("source{n}"));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("owned"), b"synthetic").unwrap();
        let out = call(&[
            "scan".as_ref(),
            root.as_os_str(),
            "--save".as_ref(),
            "--store".as_ref(),
            history.as_os_str(),
        ]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["retained"], true);
        assert!(v["retention_warning"].is_null());
        ids.push(v["scan_id"].as_str().unwrap().to_owned());
    }
    let s = Store::open(&history, false).unwrap();
    assert_eq!(s.ids().unwrap().len(), 3);
    assert!(s.manifest(&ids[0]).is_err());
    for id in ids.iter().skip(1) {
        let out = call(&[
            "summary".as_ref(),
            "--scan".as_ref(),
            id.as_ref(),
            "--store".as_ref(),
            history.as_os_str(),
        ]);
        assert!(out.status.success());
    }
    let out = call(&[
        "scan".as_ref(),
        base.join("source0").as_os_str(),
        "--save".as_ref(),
        "--metadata-budget".as_ref(),
        "1".as_ref(),
        "--store".as_ref(),
        history.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(s.ids().unwrap().len(), 3);
    let out = call(&[
        "history-prune".as_ref(),
        "--store".as_ref(),
        history.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let dry: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(dry["applied"], false);
    let out = call(&[
        "history-prune".as_ref(),
        "--apply".as_ref(),
        "--store".as_ref(),
        history.as_os_str(),
    ]);
    assert!(out.status.success());
    let apply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(dry["removed_runs"], apply["removed_runs"]);
    assert_eq!(apply["applied"], true);
    fs::remove_dir_all(base).unwrap();
}
