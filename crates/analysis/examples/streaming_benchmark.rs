//! Explicit owned synthetic benchmark of the production streaming pipeline.
use macscan_analysis::{Store, StreamingProfile, FULL_HISTORY_BUDGET};
use macscan_engine::ScanOptions;
use serde_json::{json, Value};
use std::{
    fs, io,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Ok(()) => eprintln!("owned_fixture_cleanup=complete"),
            Err(e) => eprintln!(
                "owned_fixture_cleanup=FAILED path={} error={e}",
                self.0.display()
            ),
        }
    }
}
fn interrupted() -> io::Result<()> {
    if STOP.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"))
    } else {
        Ok(())
    }
}
fn fixture(files: usize) -> io::Result<(Fixture, PathBuf, usize)> {
    let path = std::env::temp_dir().canonicalize()?.join(format!(
        "macscan-stream-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path)?;
    let owned = Fixture(path);
    let root = owned.0.join("data");
    fs::create_dir(&root)?;
    let broad = 32;
    for d in 0..broad {
        fs::create_dir(root.join(format!("d{d:02}")))?;
    }
    for f in 0..files {
        interrupted()?;
        fs::write(
            root.join(format!("d{:02}/f{f:07}", f % broad)),
            vec![b'x'; 1 + f % 257],
        )?;
    }
    let mut deep = root.clone();
    for _ in 0..24 {
        deep = deep.join("deep");
        fs::create_dir(&deep)?;
    }
    for n in 0..32 {
        fs::hard_link(
            root.join("d00/f0000000"),
            if n == 0 {
                deep.join("hardlink")
            } else {
                root.join(format!("d{n:02}/hardlink"))
            },
        )?;
    }
    // root + 32 broad dirs + 24 nested dirs + files + 32 aliases.
    Ok((owned, root, 1 + broad + 24 + files + 32))
}
fn bytes(path: &Path) -> io::Result<u64> {
    let mut total = 0;
    for e in fs::read_dir(path)? {
        let e = e?;
        let m = e.metadata()?;
        total += if m.is_dir() {
            bytes(&e.path())?
        } else {
            m.len()
        };
    }
    Ok(total)
}
fn stage_bytes(store: &Path) -> io::Result<(u64, u64)> {
    let mut stage = 0;
    let mut work = 0;
    for e in fs::read_dir(store)? {
        let e = e?;
        if e.file_name().to_string_lossy().ends_with(".partial") && e.file_type()?.is_dir() {
            // Publication/cleanup may race this observer; caller counts failed samples.
            stage += bytes(&e.path())?;
            let p = e.path().join("work");
            if p.is_dir() {
                work += bytes(&p)?;
            }
        }
    }
    Ok((stage, work))
}
fn fds() -> io::Result<usize> {
    // One bounded 8192-entry buffer; a higher inherited limit may overflow it.
    // Overflow is reported as a failed sample, never a silently truncated count.
    let mut buffer = vec![0u64; 8192];
    let n = unsafe {
        libc::proc_pidinfo(
            std::process::id() as i32,
            libc::PROC_PIDLISTFDS,
            0,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as i32,
        )
    };
    if n <= 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize == buffer.len() * 8 {
        return Err(io::Error::other("FD sample buffer full"));
    }
    Ok(n as usize / std::mem::size_of::<libc::proc_fdinfo>())
}
fn rss() -> io::Result<i64> {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { usage.assume_init() }.ru_maxrss)
}
#[derive(Default)]
struct Samples {
    count: u64,
    errors: u64,
    fd_peak: usize,
    stage_peak: u64,
    work_peak: u64,
    overhead: Duration,
}
fn sampled<T>(store: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<(T, Samples)> {
    let done = AtomicBool::new(false);
    struct Finish<'a>(&'a AtomicBool);
    impl Drop for Finish<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    std::thread::scope(|scope| {
        let observer = scope.spawn(|| {
            let mut s = Samples::default();
            while !done.load(Ordering::Acquire) {
                let start = Instant::now();
                s.count += 1;
                match fds() {
                    Ok(n) => s.fd_peak = s.fd_peak.max(n),
                    Err(_) => s.errors += 1,
                }
                match stage_bytes(store) {
                    Ok((stage, work)) => {
                        s.stage_peak = s.stage_peak.max(stage);
                        s.work_peak = s.work_peak.max(work);
                    }
                    Err(_) => s.errors += 1,
                }
                s.overhead += start.elapsed();
                std::thread::park_timeout(Duration::from_millis(25));
            }
            s
        });
        let result = {
            let _finish = Finish(&done);
            operation()
        };
        observer.thread().unpark();
        let samples = observer
            .join()
            .map_err(|_| io::Error::other("observer panicked"))?;
        Ok((result?, samples))
    })
}
fn profile(p: &StreamingProfile) -> Value {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let t = |v: &macscan_engine::SyscallTiming| json!({"calls":v.calls,"ms":ms(v.elapsed)});
    json!({"total_ms":ms(p.total), "walk_inclusive_ms":ms(p.walk_inclusive),
        "walk_excluding_flush_ms":ms(p.walk_only()), "flush_inside_walk_ms":ms(p.flush_during_walk),
        "flush_tail_ms":ms(p.flush_tail),"flush_count":p.flush_count,"finalize_ms":ms(p.finalize),
        "watcher_join_ms":ms(p.watcher_join),"publish_ms":ms(p.publish),"other_ms":ms(p.other),
        "accounted_ms":ms(p.accounted()),"bulk_enumeration_metadata":t(&p.walk.bulk_enumeration_metadata),
        "readdir":t(&p.walk.readdir),"fstatat":t(&p.walk.fstatat),"forced_fallbacks":p.walk.forced_fallbacks,
        "walk_uninstrumented_residual_ms":ms(p.walk_only().saturating_sub(p.walk.bulk_enumeration_metadata.elapsed + p.walk.readdir.elapsed + p.walk.fstatat.elapsed))})
}
fn parity(store: &Store, before: &str, after: &str, expected: usize) -> io::Result<Value> {
    let old = store.manifest(before)?;
    let new = store.manifest(after)?;
    for field in [
        "totals",
        "entry_count",
        "error_count",
        "excluded_count",
        "coverage_diagnostics",
        "status",
    ] {
        if old[field] != new[field] {
            return Err(io::Error::other(format!("manifest mismatch {field}")));
        }
    }
    if new["entry_count"] != expected || new["status"] != "complete" || new["error_count"] != 0 {
        return Err(io::Error::other("unexpected fixture coverage/count"));
    }
    // Compare every saved value except traversal IDs. Resolve relational IDs to raw paths.
    let fields = [
        "name_display",
        "path_display",
        "name_bytes",
        "path_bytes",
        "path_key",
        "kind",
        "logical_bytes",
        "allocated_bytes",
        "accounted_allocated_bytes",
        "own_allocated_bytes",
        "observed_logical_bytes",
        "observed_allocated_bytes",
        "observed_accounted_allocated_bytes",
        "device_id",
        "inode",
        "link_count",
        "observed_link_count",
        "mtime_unix_seconds",
        "coverage_complete",
        "enumeration_complete",
        "category",
        "private_bytes",
    ];
    let row = |alias: &str| {
        fields
            .iter()
            .map(|f| format!("{alias}.{f}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let sql = format!("SELECT count(*) AS compared, count(*) FILTER (WHERE a.path_bytes IS NULL OR b.path_bytes IS NULL OR ROW({},ap.path_bytes,ao.path_bytes) IS DISTINCT FROM ROW({},bp.path_bytes,bo.path_bytes)) AS mismatches FROM entries_saved a FULL OUTER JOIN before_entries_saved b ON a.path_bytes=b.path_bytes LEFT JOIN entries_saved ap ON a.parent_id=ap.entry_id LEFT JOIN before_entries_saved bp ON b.parent_id=bp.entry_id LEFT JOIN entries_saved ao ON a.charged_to=ao.entry_id LEFT JOIN before_entries_saved bo ON b.charged_to=bo.entry_id", row("a"),row("b"));
    let result = store.query_sql(after, Some(before), &sql, 1, 0, &STOP)?;
    let r = &result["records"][0];
    if r["mismatches"] != "0" || r["compared"] != expected.to_string() {
        return Err(io::Error::other(format!("saved parity failure: {result}")));
    }
    Ok(r.clone())
}
fn command(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|r| r.status.success())
        .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}
fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let files = match args.as_slice() {
        [small] if small == "--run-small" => 1920,
        [large, count]
            if large == "--allow-large" && ["100000", "1000000"].contains(&count.as_str()) =>
        {
            count.parse().unwrap()
        }
        _ => {
            return Err(io::Error::other(
                "explicit opt-in required: --run-small OR --allow-large 100000|1000000",
            ))
        }
    };
    if cfg!(debug_assertions) {
        return Err(io::Error::other("benchmark requires --release"));
    }
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let creation = Instant::now();
    let (owned, root, expected) = fixture(files)?;
    let creation_ms = creation.elapsed().as_secs_f64() * 1000.0;
    let store = Store::open(&owned.0.join("store"), true)?;
    let options = ScanOptions::full();
    // Warm both shared paths, keeping their saved results for parity.
    let baseline = store.scan_to_store(&root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {})?;
    let (fallback, _) =
        store.scan_to_store_profiled(&root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {}, true)?;
    parity(&store, &baseline, &fallback, expected)?;
    let mut runs = Vec::new();
    for force in [false, true] {
        for repeat in 0..3 {
            interrupted()?;
            let rss_before = rss()?;
            let ((id, p), s) = sampled(store.path(), || {
                store.scan_to_store_profiled(
                    &root,
                    &options,
                    FULL_HISTORY_BUDGET,
                    &STOP,
                    |_| {},
                    force,
                )
            })?;
            let rss_after = rss()?;
            if p.accounted() != p.total || p.flush_during_walk > p.walk_inclusive {
                return Err(io::Error::other("invalid phase accounting"));
            }
            if (force
                && (p.walk.forced_fallbacks == 0
                    || p.walk.bulk_enumeration_metadata.calls != 0
                    || p.walk.fstatat.calls < files as u64))
                || (!force
                    && (p.walk.forced_fallbacks != 0
                        || p.walk.bulk_enumeration_metadata.calls == 0))
            {
                return Err(io::Error::other("expected metadata path was not exercised"));
            }
            let validation = Instant::now();
            let equal = parity(&store, &baseline, &id, expected)?;
            runs.push(json!({"mode":if force {"forced_stat_fallback"} else {"baseline"},"repeat":repeat,
                "entries":expected,"entries_per_second":expected as f64 / p.total.as_secs_f64(),"timing":profile(&p),
                "validation_ms":validation.elapsed().as_secs_f64()*1000.0,"parity":equal,
                "process_peak_rss_before_bytes":rss_before,"process_peak_rss_after_bytes":rss_after,
                "samples":s.count,"sample_errors":s.errors,"sampled_fd_peak":s.fd_peak,
                "sampled_stage_logical_bytes_peak":s.stage_peak,"sampled_work_logical_bytes_peak":s.work_peak,
                "sampler_observation_ms":s.overhead.as_secs_f64()*1000.0}));
        }
    }
    let manifest = store.manifest(&baseline)?;
    let report = json!({"benchmark":"production_streaming_v1","files_created":files,"hardlink_aliases":32,
        "broad_directories":32,"nested_depth":24,"fixture_creation_ms":creation_ms,"measured_repeats_per_mode":3,
        "conditions":"post-creation; one warmup per mode; shared process and OS caches; no cold-cache claim",
        "resource_method":"RUSAGE_SELF Darwin ru_maxrss bytes is lifetime process high-water incl fixture, warmups and prior validations; observer samples current PID FDs and owned .partial logical file lengths every 25ms plus observation time; work is a subset of stage; sampled maxima only, races counted, no allocated-byte claim",
        "environment":{"hardware":command("/usr/sbin/sysctl", &["-n","hw.model"]),"cpu":command("/usr/sbin/sysctl", &["-n","machdep.cpu.brand_string"]),"memory_bytes":command("/usr/sbin/sysctl", &["-n","hw.memsize"]),"os":command("/usr/bin/sw_vers", &[]),"kernel":command("/usr/bin/uname", &["-rsm"]),"rustc":command(&std::env::var("RUSTC").unwrap_or_else(|_|"rustc".into()), &["--version"]),"filesystem":manifest["context"]["volume_before"]["filesystem"],"deployment_target":std::env::var("MACOSX_DEPLOYMENT_TARGET").ok(),"duckdb":Store::version()},
        "totals":manifest["totals"],"runs":runs});
    drop(store);
    drop(owned);
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_pipeline_roundtrips_fallback_and_timings() {
        let (owned, root, expected) = fixture(128).unwrap();
        let store = Store::open(&owned.0.join("store"), true).unwrap();
        let options = ScanOptions {
            max_memory_bytes: 256 * 1024,
            ..ScanOptions::full()
        };
        let normal = store
            .scan_to_store(&root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {})
            .unwrap();
        let (baseline, p) = store
            .scan_to_store_profiled(&root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {}, false)
            .unwrap();
        assert_eq!(p.accounted(), p.total);
        assert!(p.walk.bulk_enumeration_metadata.calls > 0);
        assert_eq!(p.walk.forced_fallbacks, 0);
        assert!(p.flush_count > 1);
        assert!(p.flush_during_walk > Duration::ZERO);
        assert!(p.flush_tail > Duration::ZERO);
        let (fallback, p) = store
            .scan_to_store_profiled(&root, &options, FULL_HISTORY_BUDGET, &STOP, |_| {}, true)
            .unwrap();
        assert_eq!(p.accounted(), p.total);
        assert!(p.walk.forced_fallbacks > 0);
        assert_eq!(p.walk.bulk_enumeration_metadata.calls, 0);
        assert!(p.walk.fstatat.calls >= 128);
        parity(&store, &normal, &baseline, expected).unwrap();
        parity(&store, &normal, &fallback, expected).unwrap();
        assert!(!store.path().read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")));
    }
}
