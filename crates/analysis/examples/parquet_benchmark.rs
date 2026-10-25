//! Owned fixture only; measures the Parquet/DuckDB workflow, not user content.
use macscan_analysis::Store;
use macscan_engine::{scan, CompareQuery, Metric, ScanOptions, ScanStatus, STORE_BUDGET};
use serde_json::json;
use std::{
    fs, io,
    mem::MaybeUninit,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        eprintln!("Removing only owned fixture {}", self.0.display());
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
    let path = PathBuf::from(format!(
        "/private/tmp/macscan-parquet-benchmark-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path)?;
    let fixture = Fixture(path);
    eprintln!("Owned fixture {}", fixture.0.display());
    let root = fixture.0.join("data");
    fs::create_dir(&root)?;
    let store = Store::open(&fixture.0.join("store"), true)?;
    let creation = Instant::now();
    for dir in 0..count / 1000 {
        let folder = root.join(format!("d{dir:04}"));
        fs::create_dir(&folder)?;
        for file in 0..if dir == 0 { 998 } else { 999 } {
            fs::File::create(folder.join(format!("f{file:04}")))?;
        }
        if dir % 100 == 0 {
            eprintln!("created {} directories", dir + 1);
        }
    }
    let creation_ms = creation.elapsed().as_millis();
    let mut passes = Vec::new();
    let mut ids = Vec::new();
    for pass in 0..2 {
        let data = scan(&root, &ScanOptions::default(), &AtomicBool::new(false))?;
        if data.status != ScanStatus::Complete || data.entries.len() != count {
            return Err(io::Error::other("benchmark scan is incomplete"));
        }
        let start = Instant::now();
        store.save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))?;
        let save_ms = start.elapsed().as_millis();
        let id = data.id.clone();
        let scan_ms = data.duration_ms;
        drop(data);
        ids.push(id.clone());
        let start = Instant::now();
        let answer=store.query_sql(&id,None,"SELECT path_display,logical_bytes FROM entries WHERE kind='file' ORDER BY logical_bytes DESC,entry_id LIMIT 10",10,0,&AtomicBool::new(false))?;
        let query_ms = start.elapsed().as_millis();
        passes.push(json!({"entries":count,"scan_ms":scan_ms,"save_ms":save_ms,"query_ms":query_ms,"answer_bytes":serde_json::to_vec(&answer)?.len()}));
        eprintln!(
            "pass={pass} scan_ms={} save_ms={save_ms} query_ms={query_ms}",
            scan_ms
        );
        if pass == 0 {
            fs::write(root.join("d0000/f0000"), b"x")?;
        }
    }
    let start = Instant::now();
    let compared = store.compare(
        &ids[0],
        &ids[1],
        &CompareQuery {
            metric: Metric::Logical,
            limit: 10,
            cursor: None,
        },
        &AtomicBool::new(false),
    )?;
    let compare_ms = start.elapsed().as_millis();
    if compared["totals"]["delta_bytes"] != "1" {
        return Err(io::Error::other("comparison returned incorrect delta"));
    }
    let scans = store.scans(10, None)?;
    let bytes: u64 = scans["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["metadata_bytes"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .sum();
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let peak = unsafe { usage.assume_init() }.ru_maxrss;
    println!(
        "{}",
        json!({"benchmark":"rust_parquet_duckdb","duckdb":Store::version(),"creation_ms":creation_ms,"passes":passes,"compare_ms":compare_ms,"comparison_answer_bytes":serde_json::to_vec(&compared)?.len(),"retained_metadata_bytes":bytes.to_string(),"peak_process_rss_bytes":peak.to_string(),"metadata_budget_bytes":STORE_BUDGET,"cache":"post-creation and warm; no global cache changes","scope":"empty APFS files, one gains one byte; not full disk","fixture_removed_on_exit":true})
    );
    Ok(())
}
