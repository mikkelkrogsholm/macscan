//! Bounded batches -> Parquet -> disk-backed aggregation -> immutable snapshot.
use super::*;
use macscan_engine::{walk_with_progress, Progress, ScanOptions, StreamEntry};
use std::os::fd::AsRawFd;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RAW_SCHEMA: &str = "entry_id UBIGINT,parent_id UBIGINT,subtree_end UBIGINT,name_display VARCHAR,path_display VARCHAR,name_bytes BLOB,path_bytes BLOB,path_key BLOB,kind VARCHAR,own_logical UBIGINT,own_allocated UBIGINT,device_id UBIGINT,inode UBIGINT,link_count UBIGINT,mtime_unix_seconds BIGINT,enumeration_complete BOOLEAN,category VARCHAR";
struct Finished<'a>(&'a AtomicBool);
impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Store {
    pub fn scan_to_store(
        &self,
        root: &Path,
        options: &ScanOptions,
        budget: u64,
        cancel: &AtomicBool,
        mut progress: impl FnMut(&Progress),
    ) -> io::Result<String> {
        cancelled(cancel)?;
        scanner_file_budget()?;
        let lock = fs::File::open(&self.root)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // No other publisher holds this store's lock. Reclaim only our own
        // unpublished streaming workspaces after a prior process exit/crash.
        for child in fs::read_dir(&self.root)? {
            let child = child?;
            if !child.file_type()?.is_dir() {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            let Some(previous) = name.strip_suffix(".partial") else {
                continue;
            };
            if validate_id(previous).is_err() {
                continue;
            }
            let marker = child.path().join("stream-stage.json");
            if fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && m.len() < 1024) {
                if let Ok(value) = fs::read(&marker)
                    .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(io::Error::other))
                {
                    if value["owner"] == "macscan-streaming-v1" && value["scan_id"] == previous {
                        fs::remove_dir_all(child.path())?;
                    }
                }
            }
        }
        let old_bytes = directory_bytes(&self.root)?;
        if old_bytes >= budget {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "metadata history budget exhausted",
            ));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!(
            "{nanos}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut stage = Stage {
            path: self.root.join(format!("{id}.partial")),
            published: false,
        };
        fs::DirBuilder::new().mode(0o700).create(&stage.path)?;
        fs::write(
            stage.path.join("stream-stage.json"),
            serde_json::to_vec(&json!({"owner":"macscan-streaming-v1","scan_id":id}))?,
        )?;
        for dir in ["raw", "work", "entries"] {
            fs::create_dir(stage.path.join(dir))?;
        }
        let workspace_limit = budget
            .saturating_sub(old_bytes)
            .saturating_mul(8)
            .clamp(64 * 1024 * 1024, 8 * 1024 * 1024 * 1024);
        let stop = AtomicBool::new(false);
        let done = AtomicBool::new(false);
        let exhausted = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            let _finished = Finished(&done);
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if cancel.load(Ordering::Relaxed) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if directory_bytes(&stage.path).is_ok_and(|n| n > workspace_limit)
                        || available(&stage.path).is_ok_and(|n| n < 512 * 1024 * 1024)
                    {
                        exhausted.store(true, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
            (|| -> io::Result<()> {
                let db = Db::spooling(
                    stage
                        .path
                        .join("work/scan.duckdb")
                        .to_str()
                        .ok_or_else(|| invalid("store must be UTF-8"))?,
                    stage.path.join("work/spill").to_str().unwrap(),
                    workspace_limit,
                )?;
                let mut batch = Vec::new();
                let mut bytes = 0_usize;
                let mut part = 0;
                let mut scan_options = ScanOptions {
                    max_entries: options.max_entries,
                    max_duration: options.max_duration,
                    max_depth: options.max_depth,
                    max_memory_bytes: options.max_memory_bytes,
                    exclusions: options.exclusions.clone(),
                };
                if !scan_options.exclusions.contains(&self.root) {
                    scan_options.exclusions.push(self.root.clone());
                }
                let batch_budget = (options.max_memory_bytes / 4).clamp(64 * 1024, 4 * 1024 * 1024);
                let mut summary = walk_with_progress(
                    root,
                    &scan_options,
                    &stop,
                    |node| {
                        bytes = bytes.saturating_add(
                            node.path.as_os_str().as_bytes().len() * 4
                                + node.entry.name.len()
                                + 256,
                        );
                        batch.push(node);
                        if batch.len() >= 4096 || bytes >= batch_budget {
                            flush(&db, &stage.path, &mut batch, part)?;
                            part += 1;
                            bytes = 0;
                        }
                        Ok(())
                    },
                    |p| progress(p),
                )?;
                if !batch.is_empty() {
                    flush(&db, &stage.path, &mut batch, part)?;
                }
                summary.scan.id = id.clone();
                cancelled(&stop)?;
                let root_valid = !summary
                    .scan
                    .issues
                    .iter()
                    .any(|i| i.code == "VOLUME_CHANGED");
                let totals = finalize(&db, &stage.path, &stop, root_valid)?;
                let scan = &summary.scan;
                let manifest = json!({"storage_format":FORMAT,"schema_version":"2.0","engine_version":ENGINE_VERSION,
                    "scan_id":id,"status":scan.status.name(),"scope":scan.report(0)["scope"],"context":scan.context,
                    "started_at":scan.started_at,"ended_at":scan.ended_at,"duration_ms":scan.duration_ms,
                    "entry_count":summary.entry_count,"error_count":scan.issue_count,"excluded_count":scan.excluded_count,
                    "scan_mode":"streaming_full","workspace_limit_bytes":workspace_limit,
                    "totals":{"logical_bytes":totals["logical_bytes"],"allocated_bytes":totals["allocated_bytes"],"accounted_allocated_bytes":totals["accounted_allocated_bytes"],"private_bytes":null,
                      "observed_lower_bound":{"logical_bytes":totals["observed_logical_bytes"],"allocated_bytes":totals["observed_allocated_bytes"],"accounted_allocated_bytes":totals["observed_accounted_allocated_bytes"]}},
                    "issues":scan.issues.iter().take(8).map(|i|json!({"entry_id":i.entry_id.to_string(),"code":i.code,"operation":i.operation,"detail":i.detail})).collect::<Vec<_>>()});
                drop(db);
                fs::remove_dir_all(stage.path.join("raw"))?;
                fs::remove_dir_all(stage.path.join("work"))?;
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(stage.path.join("manifest.json"))?;
                file.write_all(&serde_json::to_vec(&manifest)?)?;
                file.sync_all()?;
                if old_bytes.saturating_add(directory_bytes(&stage.path)?) > budget {
                    return Err(io::Error::new(
                        io::ErrorKind::StorageFull,
                        "metadata history budget exhausted",
                    ));
                }
                cancelled(&stop)?;
                Ok(())
            })()
        });
        if exhausted.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::StorageFull,"scan workspace budget exhausted or less than 512 MiB free; previous snapshots preserved"));
        }
        if let Err(ref e) = result {
            if e.kind() == io::ErrorKind::Other {
                // One bounded private diagnostic, not an accumulating log.
                let _ = fs::write(
                    self.root.join("last-scan-error.json"),
                    serde_json::to_vec(&json!({"scan_id":id,"error":e.to_string()}))?,
                );
            }
        }
        result?;
        cancelled(cancel)?;
        let a = std::ffi::CString::new(stage.path.as_os_str().as_bytes()).unwrap();
        let b = std::ffi::CString::new(self.location(&id)?.as_os_str().as_bytes()).unwrap();
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        stage.published = true;
        // Keep the ownership marker until after atomic publication. A crash
        // before rename must still leave a recognizable unpublished workspace.
        let _ = fs::remove_file(self.location(&id)?.join("stream-stage.json"));
        Ok(id)
    }
}

fn scanner_file_budget()->io::Result<()> {
    // Finder-launched apps inherit a 256-FD soft limit. DuckDB can cache one
    // descriptor per raw Parquet part (a full disk has thousands of parts).
    // Raise only this process's soft limit, never the hard/system-wide limit.
    let mut limit=std::mem::MaybeUninit::<libc::rlimit>::uninit();
    if unsafe{libc::getrlimit(libc::RLIMIT_NOFILE,limit.as_mut_ptr())}!=0{return Err(io::Error::last_os_error());}
    let mut limit=unsafe{limit.assume_init()};let target=8192.min(limit.rlim_max);
    if limit.rlim_cur<target {
        limit.rlim_cur=target;
        if unsafe{libc::setrlimit(libc::RLIMIT_NOFILE,&limit)}!=0{return Err(io::Error::other(format!("cannot raise scanner process file-handle budget: {}",io::Error::last_os_error())));}
    }
    Ok(())
}

fn finalize(db: &Db, stage: &Path, stop: &AtomicBool, root_valid: bool) -> io::Result<Value> {
    db.exec(&format!(
        "CREATE VIEW raw AS SELECT * REPLACE(enumeration_complete AND (entry_id<>0 OR {root_valid}) AS enumeration_complete) FROM read_parquet({})",
        path_literal(&stage.join("raw/*.parquet"))?
    ))?;
    // Only hardlinked regular files enter this disk-backed lookup table.
    db.exec_cancel("CREATE TABLE links AS SELECT device_id,inode,arg_min(entry_id,path_bytes) AS owner,count(*)::UBIGINT AS observed FROM raw WHERE kind='file' AND link_count>1 GROUP BY device_id,inode",stop)?;
    db.exec("CREATE VIEW charged AS SELECT r.*,COALESCE(l.observed,1)::UBIGINT AS observed_link_count,CASE WHEN l.owner IS NOT NULL AND l.owner<>r.entry_id THEN l.owner END AS charged_to,CASE WHEN l.owner IS NOT NULL AND l.owner<>r.entry_id THEN 0 ELSE r.own_allocated END::UBIGINT AS own_accounted FROM raw r LEFT JOIN links l ON r.kind='file' AND r.link_count>1 AND r.device_id=l.device_id AND r.inode=l.inode")?;
    // DFS IDs make each subtree a contiguous interval. Three prefix
    // sums and one coverage count replace an in-memory directory tree.
    db.exec_cancel("CREATE TABLE prefix AS SELECT entry_id,sum(own_logical) OVER w AS logical_sum,sum(own_allocated) OVER w AS allocated_sum,sum(own_accounted) OVER w AS accounted_sum,sum(CASE WHEN enumeration_complete THEN 0 ELSE 1 END) OVER w AS unknown_sum FROM charged WINDOW w AS (ORDER BY entry_id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)",stop)?;
    // Only directories need subtree sums. Keep the large prefix
    // joins out of the wide, all-file Parquet output pipeline.
    db.exec_cancel("CREATE TABLE endpoints AS SELECT entry_id AS directory_id,subtree_end::BIGINT AS lookup_id,true AS is_end FROM raw WHERE kind='directory' UNION ALL SELECT entry_id,entry_id::BIGINT-1,false FROM raw WHERE kind='directory' AND entry_id>0",stop)?;
    db.exec_cancel("CREATE TABLE endpoint_values AS SELECT e.directory_id,e.is_end,p.logical_sum,p.allocated_sum,p.accounted_sum,p.unknown_sum FROM endpoints e JOIN prefix p ON p.entry_id=e.lookup_id",stop)?;
    db.exec_cancel("CREATE TABLE directory_totals AS SELECT directory_id AS entry_id,sum(CASE WHEN is_end THEN logical_sum ELSE -logical_sum END)::UBIGINT AS logical_total,sum(CASE WHEN is_end THEN allocated_sum ELSE -allocated_sum END)::UBIGINT AS allocated_total,sum(CASE WHEN is_end THEN accounted_sum ELSE -accounted_sum END)::UBIGINT AS accounted_total,sum(CASE WHEN is_end THEN unknown_sum ELSE -unknown_sum END)=0 AS complete FROM endpoint_values GROUP BY directory_id",stop)?;
    db.exec("CREATE VIEW sized AS SELECT r.*,CASE WHEN r.kind='directory' THEN d.logical_total ELSE r.own_logical END AS logical_total,CASE WHEN r.kind='directory' THEN d.allocated_total ELSE r.own_allocated END AS allocated_total,CASE WHEN r.kind='directory' THEN d.accounted_total ELSE r.own_accounted END AS accounted_total,CASE WHEN r.kind='directory' THEN d.complete ELSE r.enumeration_complete END AS complete FROM charged r LEFT JOIN directory_totals d ON d.entry_id=r.entry_id")?;
    db.exec("CREATE VIEW final AS SELECT entry_id,parent_id,name_display,path_display,name_bytes,path_bytes,path_key,kind,CASE WHEN complete THEN logical_total END AS logical_bytes,CASE WHEN complete THEN allocated_total END AS allocated_bytes,CASE WHEN complete THEN accounted_total END AS accounted_allocated_bytes,own_allocated AS own_allocated_bytes,logical_total AS observed_logical_bytes,allocated_total AS observed_allocated_bytes,accounted_total AS observed_accounted_allocated_bytes,device_id,inode,link_count,observed_link_count,charged_to,mtime_unix_seconds,complete AS coverage_complete,enumeration_complete,category,NULL::UBIGINT AS private_bytes FROM sized")?;
    db.exec_cancel(&format!("COPY (SELECT * FROM final) TO {} (FORMAT PARQUET,COMPRESSION ZSTD,ROW_GROUP_SIZE 8192,FILE_SIZE_BYTES '32MB',PER_THREAD_OUTPUT true,FILENAME_PATTERN 'part-{{i}}')",path_literal(&stage.join("entries"))?),stop)?;
    // Root state plus directory coverage transitions, including the
    // end boundary that restores the parent's enumeration status.
    db.exec("CREATE VIEW directories AS SELECT entry_id,parent_id,path_key,enumeration_complete FROM raw WHERE kind='directory'")?;
    db.exec_cancel(&format!("COPY (SELECT path_key,enumeration_complete AS complete FROM raw WHERE entry_id=0 UNION ALL SELECT e.path_key,e.enumeration_complete FROM directories e JOIN directories p ON p.entry_id=e.parent_id WHERE e.enumeration_complete<>p.enumeration_complete UNION ALL SELECT unhex(left(hex(e.path_key),length(hex(e.path_key))-2)||'01'),p.enumeration_complete FROM directories e JOIN directories p ON p.entry_id=e.parent_id WHERE e.enumeration_complete<>p.enumeration_complete ORDER BY path_key) TO {} (FORMAT PARQUET,COMPRESSION ZSTD,ROW_GROUP_SIZE 8192)",path_literal(&stage.join("coverage.parquet"))?),stop)?;
    let totals=db.query("SELECT logical_bytes,allocated_bytes,accounted_allocated_bytes,observed_logical_bytes,observed_allocated_bytes,observed_accounted_allocated_bytes FROM final WHERE entry_id=0",Some(stop))?.row(0)?;
    Ok(totals)
}

fn flush(db: &Db, stage: &Path, batch: &mut Vec<StreamEntry>, part: usize) -> io::Result<()> {
    db.exec(&format!("CREATE TEMP TABLE batch ({RAW_SCHEMA})"))?;
    let mut app = db.appender("batch")?;
    for node in batch.iter() {
        let e = &node.entry;
        let raw = node.path.as_os_str().as_bytes();
        let relative = node.relative.as_os_str().as_bytes();
        let root = node
            .path
            .ancestors()
            .nth(node.relative.components().count())
            .unwrap();
        let mut key = relative.to_vec();
        for byte in &mut key {
            if *byte == b'/' {
                *byte = 0;
            }
        }
        if !key.is_empty() {
            key.push(0);
        }
        let own_category = relative
            .split(|&b| b == b'/')
            .rev()
            .map(category)
            .find(|&c| c != 0)
            .unwrap_or_else(|| {
                category(root.file_name().map(|n| n.as_bytes()).unwrap_or_default())
            });
        app.number(Some(node.id as u64))?;
        app.number(e.parent.map(u64::from))?;
        app.number(Some(node.subtree_end as u64))?;
        app.text(&String::from_utf8_lossy(&e.name))?;
        app.text(&node.path.to_string_lossy())?;
        app.blob(&e.name)?;
        app.blob(raw)?;
        app.blob(&key)?;
        app.text(e.kind.name())?;
        app.number(Some(e.logical))?;
        app.number(Some(e.own_allocated))?;
        app.number(Some(e.device))?;
        app.number(Some(e.inode))?;
        app.number(Some(e.links))?;
        app.signed(e.mtime)?;
        app.boolean(e.enumeration_complete)?;
        app.text(CATEGORIES[own_category as usize])?;
        app.end()?;
    }
    app.finish()?;
    db.exec(&format!(
        "COPY batch TO {} (FORMAT PARQUET,COMPRESSION ZSTD)",
        path_literal(&stage.join(format!("raw/part-{part:07}.parquet")))?
    ))?;
    db.exec("DROP TABLE batch")?;
    batch.clear();
    Ok(())
}
fn available(path: &Path) -> io::Result<u64> {
    let path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid("NUL in store"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(test)]
mod large_tests {
    use super::*;

    #[test]
    #[ignore = "Ten-million-row disk-backed finalization acceptance; run explicitly"]
    fn ten_million_rows_finalize_with_bounded_memory() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let stage = Stage {
            path: PathBuf::from(format!(
                "/private/tmp/macscan-ten-million-{}-{nanos}",
                std::process::id()
            )),
            published: false,
        };
        fs::create_dir(&stage.path).unwrap();
        for name in ["raw", "work", "entries"] {
            fs::create_dir(stage.path.join(name)).unwrap();
        }
        let db = Db::spooling(
            stage.path.join("work/test.duckdb").to_str().unwrap(),
            stage.path.join("work/spill").to_str().unwrap(),
            4 * 1024 * 1024 * 1024,
        )
        .unwrap();
        let prefix = "segment/".repeat(20);
        let raw = format!(
            r#"
          WITH n AS (SELECT range AS id FROM range(10000000)),
          tree AS (SELECT id,id=0 OR id%11=1 AS is_dir,
            CASE WHEN id=0 THEN NULL WHEN id%11=1 THEN 0 ELSE ((id-2)//11)*11+1 END::UBIGINT AS parent_id FROM n),
          names AS (SELECT *,CASE WHEN id=0 THEN '' WHEN is_dir THEN 'd'||id ELSE 'f'||id END AS name,
            CASE WHEN id=0 THEN '' WHEN is_dir THEN 'd'||id ELSE 'd'||parent_id||'/f'||id END AS relative FROM tree)
          SELECT id::UBIGINT AS entry_id,parent_id,
            CASE WHEN id=0 THEN 9999999 WHEN is_dir THEN least(id+10,9999999) ELSE id END::UBIGINT AS subtree_end,
            name AS name_display,'/test/{prefix}'||relative AS path_display,
            encode(name) AS name_bytes,encode('/test/{prefix}'||relative) AS path_bytes,
            encode(CASE WHEN id=0 THEN '' ELSE replace(relative,'/',chr(0))||chr(0) END) AS path_key,
            CASE WHEN is_dir THEN 'directory' ELSE 'file' END AS kind,
            CASE WHEN is_dir THEN 0 ELSE 1 END::UBIGINT AS own_logical,
            CASE WHEN is_dir THEN 0 ELSE 4096 END::UBIGINT AS own_allocated,
            1::UBIGINT AS device_id,(id+1)::UBIGINT AS inode,
            CASE WHEN NOT is_dir AND id%5=0 THEN 2 ELSE 1 END::UBIGINT AS link_count,
            0::BIGINT AS mtime_unix_seconds,true AS enumeration_complete,'unclassified' AS category FROM names
        "#
        );
        db.exec(&format!("COPY ({raw}) TO {} (FORMAT PARQUET,COMPRESSION ZSTD,ROW_GROUP_SIZE 8192,FILE_SIZE_BYTES '32MB',PER_THREAD_OUTPUT true)",path_literal(&stage.path.join("raw")).unwrap())).unwrap();
        println!("ten million raw rows ready");
        let totals = finalize(&db, &stage.path, &AtomicBool::new(false), true).unwrap();
        assert_eq!(totals["logical_bytes"], "9090908");
        let count = db
            .query(
                &format!(
                    "SELECT count(*) AS n FROM read_parquet({})",
                    path_literal(&stage.path.join("entries/*.parquet")).unwrap()
                ),
                None,
            )
            .unwrap()
            .row(0)
            .unwrap();
        assert_eq!(count["n"], "10000000");
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        let peak = unsafe { usage.assume_init() }.ru_maxrss;
        println!(
            "ten_million_peak_rss_bytes={peak} output_bytes={}",
            directory_bytes(&stage.path.join("entries")).unwrap()
        );
        assert!(peak < 1536 * 1024 * 1024, "RSS exceeded 1.5 GiB: {peak}");
    }
}
