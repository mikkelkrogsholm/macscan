use crate::duck::{invalid, literal, Db};
use macscan_engine::{CompareQuery, EntryKind, Metric, Query, QueryKind, Scan, ENGINE_VERSION};
use serde_json::{json, Value};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_LIMIT: usize = 10;
pub const OUTPUT_BUDGET: usize = 8 * 1024;
const FORMAT: &str = "parquet-1";
// Real simulator paths exhausted the 128 MiB writer with 32k-row batches.
const CHUNK: usize = 8_192;
const ENTRY_SCHEMA:&str="entry_id UBIGINT,parent_id UBIGINT,name_display VARCHAR,path_display VARCHAR,name_bytes BLOB,path_bytes BLOB,path_key BLOB,kind VARCHAR,logical_bytes UBIGINT,allocated_bytes UBIGINT,accounted_allocated_bytes UBIGINT,own_allocated_bytes UBIGINT,observed_logical_bytes UBIGINT,observed_allocated_bytes UBIGINT,observed_accounted_allocated_bytes UBIGINT,device_id UBIGINT,inode UBIGINT,link_count UBIGINT,observed_link_count UBIGINT,charged_to UBIGINT,mtime_unix_seconds BIGINT,coverage_complete BOOLEAN,enumeration_complete BOOLEAN,category VARCHAR,private_bytes UBIGINT";

pub struct Store {
    root: PathBuf,
}
impl Store {
    pub fn open(path: &Path, create: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("store must be an absolute directory"));
        }
        if create {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        let meta = path.symlink_metadata()?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(invalid("store must be a real directory"));
        }
        Ok(Self {
            root: path.canonicalize()?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn version() -> String {
        Db::version()
    }
    fn location(&self, id: &str) -> io::Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join(id))
    }
    pub fn manifest(&self, id: &str) -> io::Result<Value> {
        let path = self.location(id)?.join("manifest.json");
        if path.metadata()?.len() > 64 * 1024 {
            return Err(invalid("oversize scan manifest"));
        }
        let m: Value = serde_json::from_slice(&fs::read(path)?)
            .map_err(|_| invalid("invalid scan manifest"))?;
        if m["storage_format"] != FORMAT || m["scan_id"] != id {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "not a compatible Parquet scan; old JSONL data was not modified",
            ));
        }
        Ok(m)
    }
    pub fn ids(&self) -> io::Result<Vec<String>> {
        let mut ids = Vec::new();
        for item in fs::read_dir(&self.root)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if item.file_type()?.is_dir()
                && validate_id(&name).is_ok()
                && item.path().join("manifest.json").is_file()
            {
                ids.push(name);
            }
        }
        ids.sort();
        Ok(ids)
    }
    pub fn save_with_cancel(
        &self,
        scan: &Scan,
        budget: u64,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        // Serialize this store's publications so simultaneous saves cannot each
        // claim the same metadata budget. File close releases the advisory lock.
        use std::os::fd::AsRawFd;
        let publication_lock = fs::File::open(&self.root)?;
        if unsafe { libc::flock(publication_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let final_path = self.location(&scan.id)?;
        if final_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "scan already exists",
            ));
        }
        let stage_path = self.root.join(format!("{}.partial", scan.id));
        fs::DirBuilder::new().mode(0o700).create(&stage_path)?;
        let mut stage = Stage {
            path: stage_path,
            published: false,
        };
        let old_bytes = directory_bytes(&self.root)?.saturating_sub(directory_bytes(&stage.path)?);
        if old_bytes >= budget {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "metadata budget exhausted",
            ));
        }
        fs::create_dir(stage.path.join("entries"))?;
        let db = Db::new()?;
        let mut categories: Vec<u8> = Vec::with_capacity(scan.entries.len());
        for e in &scan.entries {
            let own = if e.parent.is_none() {
                category(
                    scan.root
                        .file_name()
                        .map(|n| n.as_bytes())
                        .unwrap_or_default(),
                )
            } else {
                category(&e.name)
            };
            categories.push(if own == 0 {
                e.parent.map(|p| categories[p as usize]).unwrap_or(own)
            } else {
                own
            });
        }
        for (part, entries) in scan.entries.chunks(CHUNK).enumerate() {
            cancelled(cancel)?;
            db.exec(&format!("CREATE TABLE batch ({ENTRY_SCHEMA})"))?;
            let mut app = db.appender("batch")?;
            for (offset, e) in entries.iter().enumerate() {
                if offset % 256 == 0 {
                    cancelled(cancel)?;
                }
                let id = part * CHUNK + offset;
                let path = scan.path(id as u32);
                let raw = path.as_os_str().as_bytes();
                app.number(Some(id as u64))?;
                app.number(e.parent.map(u64::from))?;
                app.text(&String::from_utf8_lossy(&e.name))?;
                app.text(&path.to_string_lossy())?;
                app.blob(&e.name)?;
                app.blob(raw)?;
                app.blob(&path_key(scan, id as u32))?;
                app.text(e.kind.name())?;
                app.number(e.complete.then_some(e.logical))?;
                app.number(e.complete.then_some(e.allocated))?;
                app.number(e.complete.then_some(e.accounted))?;
                app.number(Some(e.own_allocated))?;
                app.number(Some(e.logical))?;
                app.number(Some(e.allocated))?;
                app.number(Some(e.accounted))?;
                app.number(Some(e.device))?;
                app.number(Some(e.inode))?;
                app.number(Some(e.links))?;
                app.number(Some(e.observed_links as u64))?;
                app.number(e.charged_to.map(u64::from))?;
                app.signed(e.mtime)?;
                app.boolean(e.complete)?;
                app.boolean(e.enumeration_complete)?;
                app.text(CATEGORIES[categories[id] as usize])?;
                app.number(None)?;
                app.end()?;
            }
            app.finish()?;
            let destination = stage
                .path
                .join("entries")
                .join(format!("part-{part:05}.parquet"));
            db.exec(&format!(
                "COPY batch TO {} (FORMAT PARQUET,COMPRESSION ZSTD)",
                path_literal(&destination)?
            ))?;
            db.exec("DROP TABLE batch")?;
            if old_bytes.saturating_add(directory_bytes(&stage.path)?) > budget {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "metadata budget exhausted",
                ));
            }
        }
        // Coverage change points allow efficient ASOF lookups for missing paths.
        // A complete child may override a partial parent's enumeration status.
        db.exec("CREATE TABLE coverage_batch(path_key BLOB,complete BOOLEAN)")?;
        let mut cov = db.appender("coverage_batch")?;
        cov.blob(&[])?;
        cov.boolean(scan.entries[0].enumeration_complete)?;
        cov.end()?;
        for (id, e) in scan.entries.iter().enumerate().skip(1) {
            if id % 256 == 0 {
                cancelled(cancel)?;
            }
            if e.kind != EntryKind::Directory {
                continue;
            }
            let parent = scan.entries[e.parent.unwrap() as usize].enumeration_complete;
            if parent != e.enumeration_complete {
                let mut key = path_key(scan, id as u32);
                cov.blob(&key)?;
                cov.boolean(e.enumeration_complete)?;
                cov.end()?;
                *key.last_mut().unwrap() = 1;
                cov.blob(&key)?;
                cov.boolean(parent)?;
                cov.end()?;
            }
        }
        cov.finish()?;
        db.exec(&format!("COPY (SELECT * FROM coverage_batch ORDER BY path_key) TO {} (FORMAT PARQUET,COMPRESSION ZSTD)",path_literal(&stage.path.join("coverage.parquet"))?))?;
        let report = scan.report(0);
        let manifest = json!({"storage_format":FORMAT,"schema_version":"2.0","engine_version":ENGINE_VERSION,
            "scan_id":scan.id,"status":scan.status.name(),"scope":report["scope"],"context":scan.context,
            "started_at":scan.started_at,"ended_at":scan.ended_at,"duration_ms":scan.duration_ms,
            "entry_count":scan.entries.len(),"error_count":scan.issue_count,"excluded_count":scan.excluded_count,
            "totals":compact_totals(&report["totals"]),
            "issues":scan.issues.iter().take(8).map(|i|json!({"entry_id":i.entry_id.to_string(),"code":i.code,"operation":i.operation,"detail":i.detail})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&manifest)?;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stage.path.join("manifest.json"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        if old_bytes.saturating_add(directory_bytes(&stage.path)?) > budget {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "metadata budget exhausted",
            ));
        }
        cancelled(cancel)?;
        let a = std::ffi::CString::new(stage.path.as_os_str().as_bytes()).unwrap();
        let b = std::ffi::CString::new(final_path.as_os_str().as_bytes()).unwrap();
        // RENAME_EXCL atomically publishes only this operation's new directory.
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        stage.published = true;
        Ok(())
    }

    fn attach(&self, db: &Db, id: &str, table: &str) -> io::Result<Value> {
        let manifest = self.manifest(id)?;
        let location = self.location(id)?;
        let entries = location.join("entries/*.parquet");
        let coverage = location.join("coverage.parquet");
        db.exec(&format!(
            "CREATE VIEW {table} AS SELECT *,{} AS scan_id FROM read_parquet({})",
            literal(id),
            path_literal(&entries)?
        ))?;
        db.exec(&format!(
            "CREATE VIEW {table}_coverage AS SELECT * FROM read_parquet({})",
            path_literal(&coverage)?
        ))?;
        Ok(manifest)
    }
    fn restrict(&self, db: &Db) -> io::Result<()> {
        db.exec(&format!("SET allowed_directories=[{}]; SET enable_external_access=false; SET lock_configuration=true",path_literal(&self.root)?))
    }
    pub fn query_sql(
        &self,
        id: &str,
        before: Option<&str>,
        sql: &str,
        limit: usize,
        offset: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        let db = if before.is_some() {
            Db::comparison()?
        } else {
            Db::analysis()?
        };
        let after = self.attach(&db, id, "entries")?;
        let before_manifest = if let Some(before) = before {
            let old = self.attach(&db, before, "before_entries")?;
            compatible(&old, &after)?;
            db.exec(&changes_view(
                old["status"] == "complete",
                after["status"] == "complete",
            ))?;
            Some(old)
        } else {
            None
        };
        self.restrict(&db)?;
        let mut answer = envelope(&after, "query", Metric::Allocated);
        answer["metric"] = json!("sql_selected_columns");
        if let Some(old) = before_manifest {
            answer["before_scan_id"] = old["scan_id"].clone();
            answer["coverage"]["complete"] =
                json!(old["status"] == "complete" && after["status"] == "complete");
            answer["coverage"]["before_status"] = old["status"].clone();
            answer["coverage"]["after_status"] = after["status"].clone();
            if answer["coverage"]["complete"] == false {
                answer["status"] = json!("partial");
            }
        }
        run_query(&db, sql, limit, offset, cancel, &mut answer)?;
        compact_response(answer, None)
    }
    pub fn report(&self, id: &str, q: &Query, cancel: &AtomicBool) -> io::Result<Value> {
        if q.metric == Metric::Private {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private metric is unavailable",
            ));
        }
        let db = Db::analysis()?;
        let manifest = self.attach(&db, id, "entries")?;
        self.restrict(&db)?;
        let command = match q.kind {
            QueryKind::Summary => "summary",
            QueryKind::Children => "children",
            QueryKind::Largest => "largest",
            QueryKind::Inspect => "inspect",
        };
        let binding = format!(
            "p1:{id}:{command}:{}:{}:{}:{}:{}",
            q.entry,
            q.metric.name(),
            hex(q.search.as_bytes()),
            q.only_kind.map(|k| k.name()).unwrap_or("all"),
            q.min_bytes
        );
        let offset = cursor_offset(q.cursor.as_deref(), &binding)?;
        let metric = metric_column(q.metric);
        let observed = observed_column(q.metric);
        let selected=db.query(&format!("SELECT entry_id,logical_bytes,allocated_bytes,accounted_allocated_bytes,observed_logical_bytes,observed_accounted_allocated_bytes,coverage_complete,hex(path_key) AS key FROM entries WHERE entry_id={}",q.entry),None)?.row(0)?;
        if selected["entry_id"].is_null() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "entry not found"));
        }
        let mut answer = envelope(&manifest, command, q.metric);
        answer["query_entry_id"] = json!(q.entry.to_string());
        if q.entry != 0 {
            answer["totals"] = selected.clone();
        }
        if q.kind == QueryKind::Summary {
            answer["records"] = json!([]);
            answer["error_samples"] = manifest["issues"].clone();
            answer["scan_metadata_bytes"] =
                json!(directory_bytes(&self.location(id)?)?.to_string());
            answer["capacity_at_scan"] = json!({"free_bytes":manifest["context"]["volume_after"]["free_bytes"],"captured_at":manifest["context"]["volume_after"]["captured_at"]});
            return fit(answer);
        }
        let sql = if q.kind == QueryKind::Inspect {
            format!("SELECT entry_id,parent_id,name_display,path_display,hex(path_bytes) AS path_bytes_hex,kind,logical_bytes,allocated_bytes,accounted_allocated_bytes,own_allocated_bytes,private_bytes,device_id,inode,link_count,observed_link_count,charged_to,mtime_unix_seconds,coverage_complete,enumeration_complete,category FROM entries WHERE entry_id={}",q.entry)
        } else {
            let mut filter = if q.kind == QueryKind::Children {
                format!("parent_id={}", q.entry)
            } else {
                let key = unhex(selected["key"].as_str().unwrap_or(""))?;
                let range = if key.is_empty() {
                    "entry_id<>0".into()
                } else {
                    let mut end = key.clone();
                    *end.last_mut().unwrap() = 1;
                    format!(
                        "path_key>unhex({}) AND path_key<unhex({})",
                        literal(&hex(&key)),
                        literal(&hex(&end))
                    )
                };
                format!("kind='file' AND ({range})")
            };
            if !q.search.is_empty() {
                filter.push_str(&format!(
                    " AND contains(path_display,{})",
                    literal(&q.search)
                ));
            }
            if let Some(kind) = q.only_kind {
                filter.push_str(&format!(" AND kind={}", literal(kind.name())));
            }
            if q.min_bytes > 0 {
                filter.push_str(&format!(
                    " AND COALESCE({metric},{observed})>={}",
                    q.min_bytes
                ));
            }
            let path = if q.kind == QueryKind::Largest {
                ",path_display"
            } else {
                ""
            };
            format!("SELECT entry_id,name_display,kind,{metric} AS bytes,CASE WHEN NOT coverage_complete THEN {observed} END AS lower_bound_bytes,coverage_complete,category,mtime_unix_seconds{path} FROM entries WHERE {filter} ORDER BY COALESCE({metric},{observed}) DESC,entry_id")
        };
        run_query(
            &db,
            &sql,
            if q.kind == QueryKind::Inspect {
                1
            } else {
                q.limit
            },
            offset,
            cancel,
            &mut answer,
        )?;
        if let Some(next) = answer["next_offset"].as_u64() {
            answer["next_cursor"] = json!(format!("{binding}:{next}"));
        }
        compact_response(answer, Some(&binding))
    }
    /// Human UI projection, independently bounded; does not relax agent output limits.
    pub fn sunburst(
        &self,
        id: &str,
        entry: u32,
        metric: Metric,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        if metric == Metric::Private {
            return Err(invalid("private metric is unavailable"));
        }
        let db = Db::analysis()?;
        let manifest = self.attach(&db, id, "entries")?;
        self.restrict(&db)?;
        let column = metric_column(metric);
        let observed = observed_column(metric);
        let fields = format!("entry_id,parent_id,name_display,path_display,kind,{column} AS bytes,{observed} AS observed_bytes,logical_bytes,allocated_bytes,accounted_allocated_bytes,coverage_complete,link_count,category");
        let selected = db
            .query(
                &format!("SELECT {fields} FROM entries WHERE entry_id={entry}"),
                Some(cancel),
            )?
            .row(0)?;
        let trail_fields = "entry_id,parent_id,name_display,path_display";
        let mut trail = db.query(&format!("WITH RECURSIVE trail AS (SELECT {trail_fields},0 AS hop FROM entries WHERE entry_id={entry} UNION ALL SELECT e.entry_id,e.parent_id,e.name_display,e.path_display,t.hop+1 FROM entries e JOIN trail t ON e.entry_id=t.parent_id WHERE t.hop<64) SELECT {trail_fields} FROM trail ORDER BY hop DESC"),Some(cancel))?;
        let mut ancestors = Vec::new();
        for i in 0..trail.len() {
            ancestors.push(trail.row(i)?);
        }
        let count = db
            .query(
                &format!("SELECT count(*) AS count FROM entries WHERE parent_id={entry}"),
                Some(cancel),
            )?
            .row(0)?;
        let mut nodes = vec![selected.clone()];
        let mut parents = vec![entry.to_string()];
        for depth in 1..=3 {
            if parents.is_empty() || nodes.len() >= 320 {
                break;
            }
            let remaining = 320 - nodes.len();
            let mut rows = db.query(&format!("SELECT {fields} FROM (SELECT *,row_number() OVER (PARTITION BY parent_id ORDER BY COALESCE({column},{observed}) DESC,entry_id) AS rank FROM entries WHERE parent_id IN ({})) WHERE rank<=12 ORDER BY rank,parent_id,entry_id LIMIT {remaining}", parents.join(",")),Some(cancel))?;
            parents.clear();
            for i in 0..rows.len() {
                let mut row = rows.row(i)?;
                if row["kind"] == "directory" && depth < 3 {
                    parents.push(row["entry_id"].as_str().unwrap().to_owned());
                }
                row["depth"] = json!(depth);
                nodes.push(row);
            }
        }
        let mut issues = manifest["issues"].as_array().cloned().unwrap_or_default();
        let issue_ids = issues
            .iter()
            .filter_map(|i| i["entry_id"].as_str()?.parse::<u32>().ok())
            .map(|n| n.to_string())
            .collect::<Vec<_>>();
        if !issue_ids.is_empty() {
            let mut paths = db.query(
                &format!(
                    "SELECT entry_id,path_display FROM entries WHERE entry_id IN ({})",
                    issue_ids.join(",")
                ),
                Some(cancel),
            )?;
            for i in 0..paths.len() {
                let row = paths.row(i)?;
                for issue in &mut issues {
                    if issue["entry_id"] == row["entry_id"] {
                        issue["path_display"] = row["path_display"].clone();
                    }
                }
            }
        }
        Ok(
            json!({"scan_id":id,"metric":metric.name(),"selected":selected,"ancestors":ancestors,"nodes":nodes,
          "child_count":count["count"],"status":manifest["status"],"started_at":manifest["started_at"],
          "scope":manifest["scope"],"entry_count":manifest["entry_count"],"error_count":manifest["error_count"],
          "issues":issues,"capacity":manifest["context"]["volume_after"],
          "render_limits":{"nodes":320,"levels":3,"children_per_branch":12}}),
        )
    }
    pub fn compare(
        &self,
        before: &str,
        after: &str,
        q: &CompareQuery,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        if q.metric == Metric::Private {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private metric is unavailable",
            ));
        }
        let metric = if q.metric == Metric::Logical {
            "logical"
        } else {
            "allocated"
        };
        let binding = format!("pc1:{before}:{after}:{metric}");
        let offset = cursor_offset(q.cursor.as_deref(), &binding)?;
        let sql=format!("SELECT before_entry_id,after_entry_id,path_display,kind,{metric}_before AS before_bytes,{metric}_after AS after_bytes,{metric}_delta AS delta_bytes,CASE WHEN {metric}_delta IS NULL THEN 'not_observed_or_incomplete' WHEN before_entry_id IS NULL THEN 'added' WHEN after_entry_id IS NULL THEN 'removed' WHEN {metric}_delta>0 THEN 'grown' WHEN {metric}_delta<0 THEN 'shrunk' ELSE 'type_changed' END AS change FROM changes WHERE path_key<>''::BLOB AND ({metric}_delta IS NULL OR {metric}_delta<>0 OR before_entry_id IS NULL OR after_entry_id IS NULL OR kind_before<>kind_after) ORDER BY ({metric}_delta IS NULL) DESC,abs({metric}_delta) DESC,path_key");
        let mut result = self.query_sql(after, Some(before), &sql, q.limit, offset, cancel)?;
        result["command"] = json!("compare");
        result["warnings"].as_array_mut().unwrap().push(json!(
            "nested_changes_overlap_do_not_sum; metadata_only_not_process_attribution"
        ));
        result["metric"] = json!(q.metric.name());
        let old = self.manifest(before)?;
        let new = self.manifest(after)?;
        let field = metric_column(q.metric);
        let a = old["totals"][field]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok());
        let b = new["totals"][field]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok());
        result["totals"] = json!({"before_bytes":a.map(|n|n.to_string()),"after_bytes":b.map(|n|n.to_string()),"delta_bytes":a.zip(b).map(|(a,b)|(b as i128-a as i128).to_string())});
        if let Some(next) = result["next_offset"].as_u64() {
            result["next_cursor"] = json!(format!("{binding}:{next}"));
        }
        compact_response(result, Some(&binding))
    }
    pub fn scans(&self, limit: usize, cursor: Option<&str>) -> io::Result<Value> {
        let ids = self.ids()?;
        let start = if let Some(c) = cursor {
            ids.iter()
                .position(|id| id == c)
                .map(|n| n + 1)
                .ok_or_else(|| invalid("unknown scans cursor"))?
        } else {
            0
        };
        let mut answer = json!({"schema_version":"2.0","command":"scans","status":"ok","records":[],"truncated":false,"next_cursor":null});
        let mut next = start;
        for (index, id) in ids.iter().enumerate().skip(start) {
            let Ok(m) = self.manifest(id) else {
                next = index + 1;
                continue;
            };
            if answer["records"].as_array().unwrap().len() >= limit {
                break;
            }
            let row = json!({"scan_id":id,"root":m["scope"]["root_display"],"status":m["status"],"started_at":m["started_at"],"entries":m["entry_count"],"metadata_bytes":directory_bytes(&self.location(id)?)?.to_string()});
            answer["records"].as_array_mut().unwrap().push(row);
            if serde_json::to_vec(&answer)?.len() + 512 > OUTPUT_BUDGET {
                answer["records"].as_array_mut().unwrap().pop();
                break;
            }
            next = index + 1;
        }
        answer["truncated"] = json!(next < ids.len());
        if next < ids.len() && next > start {
            answer["next_cursor"] = json!(ids[next - 1]);
        }
        compact_response(answer, None)
    }
}

fn run_query(
    db: &Db,
    sql: &str,
    limit: usize,
    offset: usize,
    cancel: &AtomicBool,
    answer: &mut Value,
) -> io::Result<()> {
    if limit == 0 || limit > 100 {
        return Err(invalid("limit must be 1..100"));
    }
    db.validate_select(sql)?;
    let sql = sql.trim().trim_end_matches(';');
    let wrapped = format!(
        "SELECT * FROM (\n{sql}\n) AS macscan_answer LIMIT {} OFFSET {offset}",
        limit + 1
    );
    let mut result = db.query(&wrapped, Some(cancel))?;
    answer["offset"] = json!(offset);
    let total = result.len();
    let mut emitted = 0;
    answer["records"] = json!([]);
    for row in 0..total.min(limit) {
        let record = result.row(row)?;
        answer["records"].as_array_mut().unwrap().push(record);
        if serde_json::to_vec(answer)?.len() + 512 > OUTPUT_BUDGET {
            answer["records"].as_array_mut().unwrap().pop();
            if emitted == 0 {
                return Err(invalid("one record exceeds 8 KiB; select fewer fields"));
            }
            break;
        }
        emitted += 1;
    }
    let truncated = total > emitted;
    answer["truncated"] = json!(truncated);
    answer["next_offset"] = if truncated {
        json!(offset + emitted)
    } else {
        Value::Null
    };
    answer["next_cursor"] = Value::Null;
    Ok(())
}
fn envelope(m: &Value, command: &str, metric: Metric) -> Value {
    json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"query_engine":"DuckDB v2.0.0","scan_id":m["scan_id"],"command":command,"status":m["status"],"metric":metric.name(),
        "scope":{"root":m["scope"]["root_display"]},"started_at":m["started_at"],"coverage":{"complete":m["status"]=="complete","error_count":m["error_count"],"excluded_count":m["excluded_count"]},
        "totals":m["totals"],"records":[],"truncated":false,"next_cursor":null,
        "warnings":["allocated_is_hardlink_accounted_not_unique_APFS_or_reclaimable","null_means_unknown; lower_bounds_are_not_complete_totals","categories_are_name_based_hints_not_deletion_approval"]})
}
fn compact_totals(v: &Value) -> Value {
    json!({"logical_bytes":v["logical_bytes"],"allocated_bytes":v["allocated_bytes"],"accounted_allocated_bytes":v["accounted_allocated_bytes"],"private_bytes":null,"observed_lower_bound":v["observed_lower_bound"]})
}
fn compatible(a: &Value, b: &Value) -> io::Result<()> {
    let fail = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "scans have different or unknown root, policy, OS or volume identity",
        )
    };
    for field in ["root_path_bytes_hex", "policy_fingerprint"] {
        if a["scope"][field] != b["scope"][field] {
            return Err(fail());
        }
    }
    if !a["context"]["os_identity"].is_string()
        || a["context"]["os_identity"] != b["context"]["os_identity"]
    {
        return Err(fail());
    }
    let uuid = &a["context"]["volume_before"]["volume_uuid"];
    if !uuid.is_string()
        || [a, b].iter().any(|m| {
            m["context"]["volume_before"]["volume_uuid"] != *uuid
                || m["context"]["volume_after"]["volume_uuid"] != *uuid
        })
    {
        return Err(fail());
    }
    let time = |m: &Value| {
        m["context"]["started_unix_ns"]
            .as_str()
            .and_then(|s| s.parse::<u128>().ok())
    };
    if !time(a).zip(time(b)).is_some_and(|(a, b)| a < b) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "AFTER must start after BEFORE",
        ));
    }
    Ok(())
}
fn changes_view(before_complete: bool, after_complete: bool) -> String {
    let mut joins = String::new();
    let before = if before_complete {
        "true"
    } else {
        joins.push_str(" ASOF LEFT JOIN before_entries_coverage bc ON p.path_key>=bc.path_key");
        "COALESCE(bc.complete,false)"
    };
    let after = if after_complete {
        "true"
    } else {
        joins.push_str(" ASOF LEFT JOIN entries_coverage ac ON p.path_key>=ac.path_key");
        "COALESCE(ac.complete,false)"
    };
    let covered = format!(
        "SELECT p.*,{before} AS before_known,{after} AS after_known FROM relevant p {joins}"
    );
    format!("CREATE VIEW changes AS WITH pairs AS (
      SELECT COALESCE(b.path_key,a.path_key) AS path_key,b.entry_id AS before_entry_id,a.entry_id AS after_entry_id,
      COALESCE(a.path_display,b.path_display) AS path_display,COALESCE(a.kind,b.kind) AS kind,b.kind AS kind_before,a.kind AS kind_after,
      b.logical_bytes AS lb,a.logical_bytes AS la,b.accounted_allocated_bytes AS ab,a.accounted_allocated_bytes AS aa
      FROM before_entries b FULL OUTER JOIN entries a ON b.path_key=a.path_key
    ), relevant AS (
      SELECT * FROM pairs WHERE before_entry_id IS NULL OR after_entry_id IS NULL
      OR lb IS NULL OR la IS NULL OR ab IS NULL OR aa IS NULL
      OR lb IS DISTINCT FROM la OR ab IS DISTINCT FROM aa OR kind_before IS DISTINCT FROM kind_after
    ), covered AS ({covered}), computed AS (
      SELECT *,CASE WHEN before_entry_id IS NULL AND before_known THEN 0 ELSE lb END AS logical_before,
      CASE WHEN after_entry_id IS NULL AND after_known THEN 0 ELSE la END AS logical_after,
      CASE WHEN before_entry_id IS NULL AND before_known THEN 0 ELSE ab END AS allocated_before,
      CASE WHEN after_entry_id IS NULL AND after_known THEN 0 ELSE aa END AS allocated_after FROM covered
    ) SELECT *,CAST(logical_after AS HUGEINT)-CAST(logical_before AS HUGEINT) AS logical_delta,
      CAST(allocated_after AS HUGEINT)-CAST(allocated_before AS HUGEINT) AS allocated_delta FROM computed")
}
fn metric_column(m: Metric) -> &'static str {
    if m == Metric::Logical {
        "logical_bytes"
    } else {
        "accounted_allocated_bytes"
    }
}
fn observed_column(m: Metric) -> &'static str {
    if m == Metric::Logical {
        "observed_logical_bytes"
    } else {
        "observed_accounted_allocated_bytes"
    }
}
fn path_key(scan: &Scan, id: u32) -> Vec<u8> {
    let path = scan.path(id);
    let relative = path
        .strip_prefix(&scan.root)
        .unwrap()
        .as_os_str()
        .as_bytes();
    if relative.is_empty() {
        return Vec::new();
    }
    let mut key = relative.to_vec();
    for byte in &mut key {
        if *byte == b'/' {
            *byte = 0;
        }
    }
    key.push(0);
    key
}
const CATEGORIES: [&str; 7] = [
    "unclassified",
    "build_output",
    "dependencies",
    "cache",
    "git_data",
    "agent_data",
    "docker_disk",
];
fn category(name: &[u8]) -> u8 {
    match name {
        b"target" | b".next" | b"build" | b"DerivedData" => 1,
        b"node_modules" | b".venv" => 2,
        b".cache" | b"Caches" | b"__pycache__" => 3,
        b".git" => 4,
        b".codex" => 5,
        b"Docker.raw" => 6,
        _ => 0,
    }
}
fn path_literal(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(literal)
        .ok_or_else(|| invalid("metadata store path must be UTF-8"))
}
fn validate_id(id: &str) -> io::Result<()> {
    if id.is_empty() || id.len() > 100 || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        Err(invalid("invalid scan ID"))
    } else {
        Ok(())
    }
}
fn cancelled(c: &AtomicBool) -> io::Result<()> {
    if c.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    } else {
        Ok(())
    }
}
fn cursor_offset(cursor: Option<&str>, binding: &str) -> io::Result<usize> {
    if let Some(c) = cursor {
        let rest = c
            .strip_prefix(&format!("{binding}:"))
            .ok_or_else(|| invalid("cursor belongs to another query"))?;
        rest.parse().map_err(|_| invalid("invalid cursor offset"))
    } else {
        Ok(0)
    }
}
fn fit(v: Value) -> io::Result<Value> {
    if serde_json::to_vec(&v)?.len() + 1 > OUTPUT_BUDGET {
        Err(invalid("response exceeds 8 KiB"))
    } else {
        Ok(v)
    }
}

pub fn encode_response(mut value: Value, ndjson: bool) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if ndjson {
        let records = value
            .as_object_mut()
            .and_then(|m| m.remove("records"))
            .unwrap_or(json!([]));
        value["type"] = json!("start");
        serde_json::to_writer(&mut bytes, &value)?;
        bytes.push(b'\n');
        for record in records.as_array().into_iter().flatten() {
            serde_json::to_writer(&mut bytes, &json!({"type":"record","record":record}))?;
            bytes.push(b'\n');
        }
        serde_json::to_writer(
            &mut bytes,
            &json!({"type":"end","scan_id":value["scan_id"],"status":value["status"],"truncated":value["truncated"],"next_cursor":value["next_cursor"],"next_offset":value["next_offset"]}),
        )?;
        bytes.push(b'\n');
    } else {
        serde_json::to_writer(&mut bytes, &value)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
pub fn compact_response(mut value: Value, binding: Option<&str>) -> io::Result<Value> {
    while encode_response(value.clone(), false)?
        .len()
        .max(encode_response(value.clone(), true)?.len())
        + 128
        > OUTPUT_BUDGET
    {
        let records = value["records"]
            .as_array_mut()
            .ok_or_else(|| invalid("response metadata exceeds 8 KiB"))?;
        if records.pop().is_none() {
            return Err(invalid(
                "one record/metadata exceeds 8 KiB; request fewer details",
            ));
        }
        if records.is_empty() {
            return Err(invalid("one record exceeds 8 KiB; request fewer details"));
        }
        let count = records.len();
        let last = records.last().unwrap().clone();
        let next = value["offset"].as_u64().unwrap_or(0) + count as u64;
        value["truncated"] = json!(true);
        value["next_offset"] = json!(next);
        if let Some(binding) = binding {
            value["next_cursor"] = json!(format!("{binding}:{next}"));
        } else if value["command"] == "scans" {
            value["next_cursor"] = last["scan_id"].clone();
        } else if value["command"] == "volumes" {
            value["next_cursor"] = json!(format!(
                "volumes-v1:{}",
                last["mount_path_bytes_hex"].as_str().unwrap_or("")
            ));
        }
    }
    Ok(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> io::Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(invalid("invalid hex"));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let s = std::str::from_utf8(p).map_err(|_| invalid("invalid hex"))?;
            u8::from_str_radix(s, 16).map_err(|_| invalid("invalid hex"))
        })
        .collect()
}
fn directory_bytes(path: &Path) -> io::Result<u64> {
    let mut bytes = 0_u64;
    for child in fs::read_dir(path)? {
        let child = child?;
        let kind = child.file_type()?;
        let size = if kind.is_dir() {
            directory_bytes(&child.path())?
        } else if kind.is_file() {
            child.metadata()?.len()
        } else {
            0
        };
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| invalid("metadata size overflow"))?;
    }
    Ok(bytes)
}
struct Stage {
    path: PathBuf,
    published: bool,
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
