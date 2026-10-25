//! Explicit developer benchmark. Creates/removes ONLY its own unique fixture.
use macscan_engine::{
    scan_with_progress, CompareQuery, Metric, Query, ScanOptions, ScanStatus, Store, STORE_BUDGET,
};
use serde_json::json;
use std::fs::{self, File};
use std::io;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        eprintln!("Removing only owned benchmark fixture {}", self.0.display());
        if let Err(e) = fs::remove_dir_all(&self.0) {
            eprintln!("Owned fixture remains: {e}");
        }
    }
}
fn main() -> io::Result<()> {
    let count = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "100000".into())
        .parse::<usize>()
        .map_err(|_| io::Error::other("count must be 100000 or 1000000"))?;
    if ![100_000, 1_000_000].contains(&count) {
        return Err(io::Error::other("count must be 100000 or 1000000"));
    }
    let root = PathBuf::from(format!(
        "/private/tmp/macscan-benchmark-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    let history_enabled = std::env::args().any(|a| a == "--with-history");
    let history_fixture = if history_enabled {
        let path = fixture.0.with_file_name(format!(
            "{}-history",
            fixture.0.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir(&path)?;
        Some(Fixture(path))
    } else {
        None
    };
    let store = history_fixture
        .as_ref()
        .map(|f| Store::open(&f.0.join("store"), true))
        .transpose()?;
    let generation = Instant::now();
    let directories = count / 1000;
    for dir in 0..directories {
        let path = fixture.0.join(format!("d{dir:04}"));
        fs::create_dir(&path)?;
        let files = if dir == 0 { 998 } else { 999 };
        for file in 0..files {
            File::create(path.join(format!("f{file:04}")))?;
        }
        if dir % 100 == 0 {
            eprintln!("Fixture: {} directories created", dir + 1);
        }
    }
    let generation_ms = generation.elapsed().as_millis();
    let mut results = Vec::new();
    let mut ids = Vec::new();
    let mut save_ms = Vec::new();
    for pass in 0..2 {
        let data = scan_with_progress(
            &fixture.0,
            &ScanOptions::default(),
            &AtomicBool::new(false),
            |p| {
                eprintln!(
                    "pass={pass} observed={} elapsed_ms={} finished={}",
                    p.entries_seen, p.elapsed_ms, p.finished
                );
            },
        )?;
        if data.entries.len() != count || data.status != ScanStatus::Complete {
            return Err(io::Error::other(format!(
                "benchmark incomplete: {} entries {:?}",
                data.entries.len(),
                data.status
            )));
        }
        let mut nav_us = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            let result = data.query(&Query::default())?;
            std::hint::black_box(result);
            nav_us.push(start.elapsed().as_micros());
        }
        nav_us.sort();
        results.push(json!({"pass":pass,"entries":data.entries.len(),"scan_ms":data.duration_ms,
            "navigation_p95_us":nav_us[18],"entry_struct_bytes":std::mem::size_of::<macscan_engine::Entry>()}));
        if let Some(store) = &store {
            let start = Instant::now();
            // Two 1M-entry histories may require an explicitly larger total
            // budget; production default remains 256 MiB, each file still bounded.
            let budget = if count == 1_000_000 {
                2 * STORE_BUDGET
            } else {
                STORE_BUDGET
            };
            store.save(&data, budget)?;
            save_ms.push(start.elapsed().as_millis());
            ids.push(data.id.clone());
            if pass == 0 {
                fs::write(fixture.0.join("d0000/f0000"), b"x")?;
            }
        }
    }
    let history = if let Some(store) = &store {
        let start = Instant::now();
        let compared = store.compare(
            &ids[0],
            &ids[1],
            &CompareQuery {
                metric: Metric::Logical,
                ..CompareQuery::default()
            },
            &AtomicBool::new(false),
        )?;
        if compared["totals"]["delta_bytes"] != "1" {
            return Err(io::Error::other("unexpected benchmark delta"));
        }
        let compare_ms = start.elapsed().as_millis();
        let mut bytes = 0_u64;
        for id in &ids {
            for file in ["entries.jsonl", "paths.jsonl", "manifest.json"] {
                bytes += fs::metadata(store.path().join(id).join(file))?.len();
            }
        }
        json!({"save_ms":save_ms,"compare_ms":compare_ms,"delta_bytes":"1","retained_metadata_bytes":bytes.to_string(),
            "explicit_total_budget_bytes":if count==1_000_000 {2*STORE_BUDGET}else{STORE_BUDGET}})
    } else {
        serde_json::Value::Null
    };
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the output on success. Darwin maxrss is bytes.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let peak = unsafe { usage.assume_init() }.ru_maxrss;
    println!(
        "{}",
        json!({"benchmark_version":2,"fixture":"owned APFS files, 1000 entries per directory; one file gains one byte if history enabled",
        "cache_note":"post-creation first pass, then warm; not a cold-cache OS benchmark",
        "fixture_generation_ms":generation_ms,"results":results,"process_peak_rss_bytes":peak,
        "includes_fixture_creation":true,"history":history,"rss_gate_passed":peak <= 256 * 1024 * 1024})
    );
    Ok(())
}
