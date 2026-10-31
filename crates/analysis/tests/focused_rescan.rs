use macscan_analysis::{InsightsQuery, Store, FULL_HISTORY_BUDGET};
use macscan_engine::{CompareQuery, Metric, ScanOptions};
use serde_json::json;
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    fs::symlink,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "macscan-focused-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn save(store: &Store, root: &std::path::Path) -> String {
    store
        .scan_to_store(
            root,
            &ScanOptions::full(),
            FULL_HISTORY_BUDGET,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
}
fn entry(store: &Store, id: &str, name: &[u8]) -> u32 {
    let hex = name.iter().map(|b| format!("{b:02X}")).collect::<String>();
    let r = store
        .query_sql(
            id,
            None,
            &format!("SELECT entry_id FROM entries_saved WHERE hex(name_bytes)='{hex}'"),
            10,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
    r["records"][0]["entry_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
fn rescan(store: &Store, id: &str, entry: u32) -> std::io::Result<String> {
    store.rescan_to_store(
        id,
        entry,
        &ScanOptions::full(),
        FULL_HISTORY_BUDGET,
        &AtomicBool::new(false),
        |_| {},
    )
}
fn bytes(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    for e in fs::read_dir(root).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            result.extend(bytes(&e.path()))
        } else {
            result.push((e.path(), fs::read(e.path()).unwrap()));
        }
    }
    result.sort();
    result
}

#[test]
fn focused_raw_identity_origin_immutability_and_same_root_comparison() {
    let f = Fixture::new();
    let source = f.0.join("source");
    fs::create_dir(&source).unwrap();
    let raw = b"project-valid";
    let focused = source.join(std::ffi::OsString::from_vec(raw.to_vec()));
    fs::create_dir(&focused).unwrap();
    fs::write(focused.join("ordinary"), b"123").unwrap();
    fs::write(source.join("outside"), b"outside").unwrap();
    let store = Store::open(&f.0.join("history"), true).unwrap();
    let original = save(&store, &source);
    let selected = entry(&store, &original, raw);
    let old = bytes(&store.path().join(&original));
    let prepared = store.prepare_rescan(&original, selected).unwrap();
    assert_eq!(prepared["source_entry_id"], selected.to_string());
    assert_eq!(
        prepared["root_path_bytes_hex"]
            .as_str()
            .unwrap()
            .to_lowercase(),
        focused
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    // Preparation is saved-data-only, even after the live source disappears.
    fs::rename(&focused, source.join("moved")).unwrap();
    assert_eq!(store.prepare_rescan(&original, selected).unwrap(), prepared);
    assert!(rescan(&store, &original, selected).is_err());
    fs::rename(source.join("moved"), &focused).unwrap();
    let new = rescan(&store, &original, selected).unwrap();
    let manifest = store.manifest(&new).unwrap();
    assert_eq!(manifest["entry_count"], 2);
    assert_eq!(
        manifest["origin"],
        json!({"source_scan_id":original,"source_entry_id":selected.to_string(),"source_started_at":store.manifest(&original).unwrap()["started_at"]})
    );
    assert_eq!(
        manifest["scope"]["root_path_bytes_hex"]
            .as_str()
            .unwrap()
            .to_lowercase(),
        prepared["root_path_bytes_hex"]
            .as_str()
            .unwrap()
            .to_lowercase()
    );
    assert_eq!(bytes(&store.path().join(&original)), old);
    let view = store
        .sunburst(&new, 0, Metric::Logical, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(view["origin"], manifest["origin"]);
    assert!(store
        .compare(
            &original,
            &new,
            &CompareQuery::default(),
            &AtomicBool::new(false)
        )
        .is_err());
    let repeat = rescan(&store, &new, 0).unwrap();
    assert_eq!(
        store.manifest(&repeat).unwrap()["scope"]["policy_fingerprint"],
        manifest["scope"]["policy_fingerprint"]
    );
    assert!(store
        .compare(
            &new,
            &repeat,
            &CompareQuery::default(),
            &AtomicBool::new(false)
        )
        .is_ok());
    // Source link persists after reopening; deleting source metadata is explicit, not a broken result.
    let reopened = Store::open(store.path(), false).unwrap();
    assert_eq!(
        reopened.manifest(&new).unwrap()["origin"],
        manifest["origin"]
    );
    let file_id = entry(&store, &new, b"ordinary");
    let file = rescan(&store, &new, file_id).unwrap();
    assert_eq!(store.manifest(&file).unwrap()["entry_count"], 1);
    assert!(store.prepare_rescan(&original, u32::MAX).is_err());
}

#[test]
fn saved_non_utf8_root_is_lossless_and_missing_live_root_is_rejected() {
    let f = Fixture::new();
    let source = f.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("ordinary"), b"synthetic").unwrap();
    let store = Store::open(&f.0.join("history"), true).unwrap();
    let mut scan =
        macscan_engine::scan(&source, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
    let raw =
        f.0.join(std::ffi::OsString::from_vec(b"saved-\xff".to_vec()));
    scan.root = raw.clone();
    store
        .save_with_cancel(&scan, FULL_HISTORY_BUDGET, &AtomicBool::new(false))
        .unwrap();
    let prepared = store.prepare_rescan(&scan.id, 0).unwrap();
    let expected = raw
        .as_os_str()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        prepared["root_path_bytes_hex"]
            .as_str()
            .unwrap()
            .to_lowercase(),
        expected
    );
    assert!(prepared["root_display"]
        .as_str()
        .unwrap()
        .contains('\u{fffd}'));
    assert!(rescan(&store, &scan.id, 0).is_err());
    assert_eq!(store.ids().unwrap(), vec![scan.id]);
}

#[test]
fn focused_missing_symlink_changed_ancestor_and_failed_publication_preserve_history() {
    let f = Fixture::new();
    let source = f.0.join("source");
    let selected = source.join("selected");
    fs::create_dir_all(&selected).unwrap();
    for i in 0..8 {
        fs::write(selected.join(format!("file{i}")), b"synthetic").unwrap();
    }
    let store = Store::open(&f.0.join("history"), true).unwrap();
    let original = save(&store, &source);
    let id = entry(&store, &original, b"selected");
    let old = bytes(&store.path().join(&original));
    let ids = store.ids().unwrap();
    fs::rename(&selected, source.join("other")).unwrap();
    assert!(rescan(&store, &original, id).is_err());
    symlink(source.join("other"), &selected).unwrap();
    assert!(rescan(&store, &original, id).is_err());
    fs::remove_file(&selected).unwrap();
    fs::rename(source.join("other"), &selected).unwrap();
    fs::rename(&source, f.0.join("moved")).unwrap();
    symlink(f.0.join("moved"), &source).unwrap();
    assert!(rescan(&store, &original, id)
        .unwrap_err()
        .to_string()
        .contains("andet sted"));
    fs::remove_file(&source).unwrap();
    fs::rename(f.0.join("moved"), &source).unwrap();
    // An ancestor alias into a synthetic provider-like path must also fail,
    // even though that destination would be excluded from enumeration.
    let provider = f.0.join("Library/CloudStorage/owned-provider");
    fs::create_dir_all(provider.parent().unwrap()).unwrap();
    fs::rename(&source, &provider).unwrap();
    symlink(&provider, &source).unwrap();
    assert!(rescan(&store, &original, id)
        .unwrap_err()
        .to_string()
        .contains("andet sted"));
    fs::remove_file(&source).unwrap();
    fs::rename(&provider, &source).unwrap();
    let stop = AtomicBool::new(false);
    assert!(store
        .rescan_to_store(
            &original,
            id,
            &ScanOptions::full(),
            FULL_HISTORY_BUDGET,
            &stop,
            |p| {
                if p.entries_seen > 0 {
                    stop.store(true, Ordering::Relaxed)
                }
            }
        )
        .is_err());
    assert!(store
        .rescan_to_store(
            &original,
            id,
            &ScanOptions::full(),
            1,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
    // Enough for initial checks, too small for the resulting staged snapshot.
    let current = bytes(store.path())
        .iter()
        .map(|(_, b)| b.len() as u64)
        .sum::<u64>();
    assert!(store
        .rescan_to_store(
            &original,
            id,
            &ScanOptions::full(),
            current + 1,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
    assert_eq!(store.ids().unwrap(), ids);
    assert_eq!(bytes(&store.path().join(&original)), old);
    assert!(fs::read_dir(store.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".partial")));
}

/// Explicit native handoff only; retains this named owned source until acceptance.
#[test]
#[ignore = "opt-in actual synthetic native source/history fixture; never scans user files"]
fn prepare_native_focused_fixture() {
    let base = PathBuf::from(
        std::env::var_os("MACSCAN_FOCUSED_FIXTURE")
            .expect("set absolute evidence/issue-13 directory"),
    );
    assert!(base.is_absolute() && base.ends_with("evidence/issue-13"));
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let source = base.join("native-source");
    let history = base.join("native-history");
    assert!(
        !source.exists() && !history.exists(),
        "fixture must be fresh; do not overwrite"
    );
    fs::create_dir_all(source.join("project/target/debug")).unwrap();
    fs::write(
        source.join("project/Cargo.toml"),
        b"[package]\nname = \"owned-synthetic\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    for i in 0..4 {
        fs::write(
            source.join(format!("project/target/debug/generated-{i}.bin")),
            vec![i as u8; 4096 * (i + 1)],
        )
        .unwrap();
    }
    fs::write(
        source.join("ordinary.txt"),
        b"owned synthetic native fixture",
    )
    .unwrap();
    fs::write(
        source.join(".macscan-owned-fixture"),
        b"issue-13-native-source-v1\n",
    )
    .unwrap();
    let store = Store::open(&history, true).unwrap();
    let baseline = save(&store, &source);
    let project = entry(&store, &baseline, b"project");
    let target = entry(&store, &baseline, b"target");
    let candidates = store
        .insights(
            &baseline,
            &InsightsQuery {
                entry: project,
                category: Some("build_output".into()),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(!candidates["records"].as_array().unwrap().is_empty());
    assert_eq!(
        candidates["records"][0]["drilldown_entry_id"],
        target.to_string()
    );
    let manifest = store.manifest(&baseline).unwrap();
    let map = json!({"source":source.to_string_lossy(),"history":history.to_string_lossy(),"baseline_scan_id":baseline,"project_entry_id":project,"target_entry_id":target,"baseline_entry_count":manifest["entry_count"],"expected_target_entry_count":6,"baseline_manifest":store.path().join(&baseline).join("manifest.json").to_string_lossy(),"candidates":candidates});
    fs::write(
        base.join("native-fixture.json"),
        serde_json::to_vec_pretty(&map).unwrap(),
    )
    .unwrap();
    println!("{}", map);
}
