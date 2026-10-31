//! Read-only age, size and raw-extension profiles of saved regular files.
use super::*;
pub const VERSION: &str = "file-profile-1";
#[derive(Clone, Debug)]
pub struct FileProfileQuery {
    pub entry: u32,
    pub metric: Metric,
    pub min_age_days: Option<u64>,
    pub min_bytes: Option<u64>,
    /// Exact opaque key returned by records: `none` or `ext:HEX`.
    pub type_key: Option<String>,
    pub types: bool,
    pub sort_count: bool,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for FileProfileQuery {
    fn default() -> Self {
        Self {
            entry: 0,
            metric: Metric::Allocated,
            min_age_days: None,
            min_bytes: None,
            type_key: None,
            types: false,
            sort_count: false,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}
impl FileProfileQuery {
    pub fn extension_key(extension: &str) -> io::Result<String> {
        if extension.is_empty()
            || extension
                .as_bytes()
                .iter()
                .any(|b| matches!(b, b'.' | b'/' | 0))
        {
            return Err(invalid(
                "extension must be a nonempty suffix without dot or slash",
            ));
        }
        Ok(format!(
            "ext:{}",
            hex(&extension.as_bytes().to_ascii_lowercase()).to_uppercase()
        ))
    }
}
impl Store {
    pub fn file_profile(
        &self,
        id: &str,
        q: &FileProfileQuery,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        self.file_profile_inner(id, q, cancel).map_err(|e| {
            if e.kind() == io::ErrorKind::Other {
                io::Error::new(e.kind(), format!("{e}; for resource-budget failures choose a smaller saved directory with --entry ID"))
            } else { e }
        })
    }
    fn file_profile_inner(
        &self,
        id: &str,
        q: &FileProfileQuery,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        cancelled(cancel)?;
        if q.metric == Metric::Private {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private metric is unavailable",
            ));
        }
        if !(1..=100).contains(&q.limit) {
            return Err(invalid("limit must be 1..100"));
        }
        if q.sort_count && !q.types {
            return Err(invalid("count sort requires types view"));
        }
        if let Some(key) = &q.type_key {
            if key != "none"
                && !key.strip_prefix("ext:").is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 510
                        && s.len() % 2 == 0
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
                })
            {
                return Err(invalid("invalid raw type_key"));
            }
        }
        let binding = format!(
            "{VERSION}:{id}:{}:{}:{:?}:{:?}:{:?}:{}:{}",
            q.metric.name(),
            q.entry,
            q.min_age_days,
            q.min_bytes,
            q.type_key,
            q.types,
            q.sort_count
        );
        let offset = cursor_offset(q.cursor.as_deref(), &binding)?;
        let db = Db::analysis()?;
        let manifest = self.attach(&db, id, "entries")?;
        apfs::require(&manifest,q.metric)?;
        let selected = db.query(&format!("SELECT entry_id,hex(path_key) AS key,hex(path_bytes) AS path_bytes_hex,path_display,coverage_complete FROM entries_saved WHERE entry_id={}",q.entry),Some(cancel))?.row(0)?;
        if selected["entry_id"].is_null() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "entry not found"));
        }
        self.restrict(&db)?;
        let complete = selected["coverage_complete"] == true;
        let filter = if let Some(key) = selected["key"].as_str() {
            format!("starts_with(hex(path_key),{})", literal(key))
        } else {
            let raw = selected["path_bytes_hex"].as_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::Unsupported, "saved raw scope unavailable")
            })?;
            format!(
                "(hex(path_bytes)={} OR starts_with(hex(path_bytes),{}))",
                literal(raw),
                literal(&if raw == "2F" {
                    raw.to_owned()
                } else {
                    format!("{raw}2F")
                })
            )
        };
        let reference = manifest["context"]["ended_unix_ns"]
            .as_str()
            .and_then(|s| s.parse::<i128>().ok());
        let threshold = q.min_age_days.map(|days| {
            reference.and_then(|r| r.checked_sub(i128::from(days) * 86_400 * 1_000_000_000))
        });
        if threshold == Some(None) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "saved reference time unavailable; age filter cannot be evaluated",
            ));
        }
        let future = reference
            .map(|r| format!("CAST(mtime_unix_seconds AS HUGEINT)*1000000000>{r}"))
            .unwrap_or_else(|| "NULL::BOOLEAN".into());
        // Split hex into aligned byte pairs: ASCII case folding never changes non-UTF8 bytes.
        let cases = (b'A'..=b'Z')
            .map(|b| format!(" WHEN '{b:02X}' THEN '{:02X}'", b + 32))
            .collect::<String>();
        let metric = metric_column(q.metric);
        let observed = observed_column(q.metric);
        db.exec_cancel(&format!(r#"CREATE TEMP VIEW profile_population AS
WITH raw AS (SELECT *,coalesce(CASE WHEN octet_length(name_bytes)>0 THEN name_bytes END,unhex(regexp_extract(hex(path_bytes),'^(?:..)*2F((?:..)*?)$',1))) AS raw_name FROM entries_saved WHERE kind='file' AND {filter}),
suffixes AS (SELECT *,CASE WHEN ends_with(hex(raw_name),'2E') THEN '' ELSE regexp_extract(hex(raw_name),'^((?:..)+)2E((?:..)+)$',2) END AS suffix FROM raw)
SELECT entry_id,path_display,hex(path_bytes) AS path_bytes_hex,mtime_unix_seconds,{metric}::HUGEINT AS bytes,{observed}::HUGEINT AS observed_lower_bound_bytes,coverage_complete,
CASE WHEN raw_name IS NULL THEN NULL WHEN suffix='' THEN 'none' ELSE 'ext:'||array_to_string(list_transform(regexp_extract_all(suffix,'..'),lambda b: CASE b {cases} ELSE b END),'') END AS type_key,
{future} AS future_mtime FROM suffixes"#),cancel)?;
        // Older snapshots have raw paths even without raw basenames. Extract basename at byte boundaries.
        // Empty file-root names also use the saved raw path basename.
        let mut filters = Vec::new();
        if let Some(Some(t)) = threshold {
            filters.push(format!("mtime_unix_seconds IS NOT NULL AND NOT coalesce(future_mtime,true) AND CAST(mtime_unix_seconds AS HUGEINT)*1000000000<={t}"));
        }
        if let Some(n) = q.min_bytes {
            filters.push(format!("bytes>={n}"));
        }
        if let Some(key) = &q.type_key {
            filters.push(format!("type_key={}", literal(key)));
        }
        let filters = if filters.is_empty() {
            "true".into()
        } else {
            filters.join(" AND ")
        };
        db.exec_cancel(&format!("CREATE TEMP VIEW profile_filtered AS SELECT * FROM profile_population WHERE {filters}"),cancel)?;
        let aggregate = format!("CASE WHEN {complete} AND coalesce(bool_and(bytes IS NOT NULL AND coverage_complete),true) THEN coalesce(sum(bytes),0) END AS bytes,coalesce(sum(observed_lower_bound_bytes),0) AS observed_lower_bound_bytes,count(*) AS file_path_count,count(*) FILTER(WHERE bytes IS NULL) AS unknown_size_count,{complete} AND coalesce(bool_and(bytes IS NOT NULL AND coverage_complete),true) AS coverage_complete");
        let order = if q.sort_count {
            "file_path_count DESC,observed_lower_bound_bytes DESC,type_key"
        } else {
            "observed_lower_bound_bytes DESC,type_key"
        };
        let sql = if q.types {
            format!("SELECT type_key,{aggregate} FROM profile_filtered GROUP BY type_key ORDER BY {order}")
        } else {
            format!("SELECT entry_id,entry_id AS drilldown_entry_id,{} AS drilldown_scan_id,path_display,path_bytes_hex,type_key,bytes,observed_lower_bound_bytes,mtime_unix_seconds,future_mtime,coverage_complete FROM profile_filtered ORDER BY observed_lower_bound_bytes DESC NULLS LAST,path_bytes_hex,entry_id",literal(id))
        };
        let mut answer = envelope(&manifest, "profile", q.metric);
        answer["profile_version"] = json!(VERSION);
        answer["mode"] = json!(if q.types { "types" } else { "files" });
        answer["sort"] = json!(if q.sort_count { "count" } else { "bytes" });
        answer["query_entry_id"] = json!(q.entry.to_string());
        answer["scope"] = selected;
        answer["coverage"]["complete"] = json!(complete);
        answer["status"] = json!(if complete { "complete" } else { "partial" });
        answer["measured_at"] = manifest["ended_at"].clone();
        answer["reference_unix_ns"] = json!(reference.map(|n| n.to_string()));
        answer["age_threshold_unix_ns"] = json!(threshold.flatten().map(|n| n.to_string()));
        answer["filters"] = json!({"min_age_days":q.min_age_days.map(|n|n.to_string()),"min_bytes":q.min_bytes.map(|n|n.to_string()),"type_key":q.type_key});
        answer["population"] = db.query("SELECT count(*) AS file_path_count,count(*) FILTER(WHERE mtime_unix_seconds IS NULL) AS unknown_mtime_count,count(*) FILTER(WHERE future_mtime) AS future_mtime_count,count(*) FILTER(WHERE future_mtime IS NULL AND mtime_unix_seconds IS NOT NULL) AS unassessed_mtime_count,count(*) FILTER(WHERE type_key IS NULL) AS unknown_type_count,count(*) FILTER(WHERE bytes IS NULL) AS unknown_size_count FROM profile_population",Some(cancel))?.row(0)?;
        answer["totals"] = db
            .query(
                &format!("SELECT {aggregate} FROM profile_filtered"),
                Some(cancel),
            )?
            .row(0)?;
        answer["accounting_note"] = json!("Kun observerede almindelige filstier, inklusive hardlinks. Allokerede bytes følger den gemte hardlink-ejer; ikke unik APFS-plads eller frigørbar plads. Ufuldstændig dækning giver kun observerede minimummer; ukendte størrelser udelades ved størrelsesfilter.");
        answer["age_note"] = json!("Ikke ændret siden den gemte tidsgrænse; dette beviser ikke, at filen er ubrugt. Ukendt og fremtidig ændringstid udelades ved aldersfilter. Filtype er kun et råt filendelseshint, ikke MIME.");
        run_query(&db, &sql, q.limit, offset, cancel, &mut answer)?;
        // Only this bounded page is decoded for a presentation hint; identity remains raw type_key.
        for row in answer["records"].as_array_mut().unwrap() {
            row["type_display"] = match row["type_key"].as_str() {
                Some("none") => json!("Ingen filendelse"),
                Some(k) => {
                    let bytes = unhex(&k[4..])?;
                    json!(String::from_utf8_lossy(&bytes))
                }
                None => Value::Null,
            };
        }
        if let Some(next) = answer["next_offset"].as_u64() {
            answer["next_cursor"] = json!(format!("{binding}:{next}"));
        }
        compact_response(answer, Some(&binding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_unknown_times_raw_scope_and_size_are_explicit() {
        let root =
            std::env::temp_dir().join(format!("macscan-profile-legacy-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let store = Store::open(&root, false).unwrap();
        let scan = root.join("501-1");
        fs::create_dir(&scan).unwrap();
        fs::create_dir(scan.join("entries")).unwrap();
        let db = Db::new().unwrap();
        db.exec(&format!("CREATE TABLE fixture ({ENTRY_SCHEMA})"))
            .unwrap();
        db.exec("INSERT INTO fixture(entry_id,parent_id,name_display,path_display,name_bytes,path_bytes,path_key,kind,logical_bytes,accounted_allocated_bytes,observed_logical_bytes,observed_accounted_allocated_bytes,coverage_complete,enumeration_complete,category) VALUES
(0,NULL,'root','/synthetic', 'root'::BLOB,'/synthetic'::BLOB,''::BLOB,'directory',0,0,0,0,true,true,'unclassified'),
(1,0,'Cargo.toml','/synthetic/Cargo.toml','Cargo.toml'::BLOB,'/synthetic/Cargo.toml'::BLOB,unhex('436172676F2E746F6D6C00'),'file',1,1,1,1,true,true,'unclassified'),
(2,0,'target','/synthetic/target','target'::BLOB,'/synthetic/target'::BLOB,unhex('74617267657400'),'directory',2,2,2,2,true,true,'unclassified'),
(3,2,'a.b.','/synthetic/target/a.b.','a.b.'::BLOB,'/synthetic/target/a.b.'::BLOB,unhex('74617267657400612E622E00'),'file',2,2,2,2,true,true,'unclassified'),
(4,2,'unknown.pdf','/synthetic/target/unknown.pdf','unknown.pdf'::BLOB,'/synthetic/target/unknown.pdf'::BLOB,unhex('74617267657400756E6B6E6F776E2E70646600'),'file',NULL,NULL,3,3,false,true,'unclassified'),
(5,0,'target-extra.pdf','/synthetic/target-extra.pdf','target-extra.pdf'::BLOB,'/synthetic/target-extra.pdf'::BLOB,unhex('7461726765742D65787472612E70646600'),'file',9,9,9,9,true,true,'unclassified')").unwrap();
        let part = scan.join("entries/part-00000.parquet");
        db.exec(&format!(
            "COPY (SELECT * EXCLUDE(mtime_unix_seconds) FROM fixture) TO {} (FORMAT PARQUET)",
            path_literal(&part).unwrap()
        ))
        .unwrap();
        db.exec(&format!(
            "COPY (SELECT ''::BLOB AS path_key,true AS complete) TO {} (FORMAT PARQUET)",
            path_literal(&scan.join("coverage.parquet")).unwrap()
        ))
        .unwrap();
        fs::write(scan.join("manifest.json"),serde_json::to_vec(&json!({"storage_format":FORMAT,"scan_id":"501-1","status":"complete","context":{"ended_unix_ns":"100000000000000"}})).unwrap()).unwrap();
        let c = AtomicBool::new(false);
        let q = FileProfileQuery {
            entry: 2,
            metric: Metric::Logical,
            ..Default::default()
        };
        let p = store.file_profile("501-1", &q, &c).unwrap();
        assert_eq!(p["population"]["unknown_mtime_count"], "2");
        assert_eq!(p["totals"]["unknown_size_count"], "1");
        assert!(p["totals"]["bytes"].is_null());
        assert_eq!(p["totals"]["observed_lower_bound_bytes"], "5");
        let aged = store
            .file_profile(
                "501-1",
                &FileProfileQuery {
                    min_age_days: Some(1),
                    ..q.clone()
                },
                &c,
            )
            .unwrap();
        assert_eq!(aged["totals"]["file_path_count"], "0");
        let sized = store
            .file_profile(
                "501-1",
                &FileProfileQuery {
                    min_bytes: Some(0),
                    ..q.clone()
                },
                &c,
            )
            .unwrap();
        assert_eq!(sized["totals"]["file_path_count"], "1");
        let insights = store
            .insights(
                "501-1",
                &InsightsQuery {
                    entry: 2,
                    ..Default::default()
                },
                &c,
            )
            .unwrap();
        assert_eq!(insights["context_coverage"], "available");
        assert!(insights["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["category"] == "build_output"));
        let old = root.join("legacy.parquet");
        db.exec(&format!("COPY (SELECT * EXCLUDE(name_bytes,path_key,enumeration_complete,mtime_unix_seconds) FROM fixture) TO {} (FORMAT PARQUET)",path_literal(&old).unwrap())).unwrap();
        fs::rename(old, &part).unwrap();
        let p = store.file_profile("501-1", &q, &c).unwrap();
        assert_eq!(p["population"]["file_path_count"], "2");
        assert!(p["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["type_key"] == "none"));
        assert!(p["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["type_key"] == "ext:706466"));
        assert!(store
            .file_profile("501-1", &FileProfileQuery { entry: 99, ..q }, &c)
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
