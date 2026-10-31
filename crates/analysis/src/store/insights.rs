//! Versioned, read-only aggregation of saved regular-file metadata.
use super::*;

pub const VERSION: &str = "insights-1";
#[derive(Clone, Debug)]
pub struct InsightsQuery {
    pub entry: u32,
    pub metric: Metric,
    pub category: Option<String>,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for InsightsQuery {
    fn default() -> Self {
        Self {
            entry: 0,
            metric: Metric::Allocated,
            category: None,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}
// Exact byte suffixes include a slash boundary. No display-string matching,
// contents, live filesystem lookup, or saved legacy category is used.
const CATALOG: &[(&str, &str, &str, &str)] = &[
    (
        "xcode_derived_data",
        "/Library/Developer/Xcode/DerivedData",
        "Probable Xcode build data from its saved directory context.",
        "Review Derived Data in Xcode settings.",
    ),
    (
        "xcode_archives",
        "/Library/Developer/Xcode/Archives",
        "Saved Xcode archive area; archives may be valuable release artifacts.",
        "Review archives in Xcode Organizer.",
    ),
    (
        "simulator_devices",
        "/Library/Developer/CoreSimulator/Devices",
        "Saved simulator device data; includes app and device state.",
        "Review devices in Xcode Devices and Simulators.",
    ),
    (
        "docker_disk",
        "/Library/Containers/com.docker.docker/Data/vms",
        "Saved Docker VM storage area; host allocation differs from internal usage.",
        "Review disk usage in Docker Desktop.",
    ),
    (
        "orbstack_disk",
        "/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data",
        "Saved OrbStack disk data area; includes VM and container state.",
        "Review storage in OrbStack.",
    ),
    (
        "ollama_models",
        "/.ollama/models",
        "Saved Ollama model area; models may be intentionally retained.",
        "Review installed models in Ollama.",
    ),
    (
        "huggingface_models",
        "/.cache/huggingface/hub",
        "Saved Hugging Face model hub area, distinct from generic cache hints.",
        "Review models with the Hugging Face cache manager.",
    ),
    (
        "lmstudio_models",
        "/.lmstudio/models",
        "Saved LM Studio model area; models may be intentionally retained.",
        "Review downloaded models in LM Studio.",
    ),
    (
        "lmstudio_models",
        "/.cache/lm-studio/models",
        "Saved LM Studio model area inside a cache directory.",
        "Review downloaded models in LM Studio.",
    ),
    (
        "package_cache",
        "/.npm/_cacache",
        "Saved npm package cache context.",
        "Review cache usage with npm tooling.",
    ),
    (
        "package_cache",
        "/.cargo/registry/cache",
        "Saved Cargo package archive cache context.",
        "Review package cache needs with Cargo tooling.",
    ),
    (
        "package_cache",
        "/.cargo/git/db",
        "Saved Cargo Git dependency cache context.",
        "Review Git dependency cache needs with Cargo tooling.",
    ),
    (
        "package_cache",
        "/Library/Caches/pip",
        "Saved pip package cache context.",
        "Review cache usage with pip tooling.",
    ),
    (
        "package_cache",
        "/.cache/pip",
        "Saved pip package cache context.",
        "Review cache usage with pip tooling.",
    ),
    (
        "package_cache",
        "/Library/Caches/Homebrew",
        "Saved Homebrew download cache context.",
        "Review cache usage with Homebrew tooling.",
    ),
    (
        "package_cache",
        "/.bun/install/cache",
        "Saved Bun package cache context.",
        "Review cache usage with Bun tooling.",
    ),
    (
        "package_cache",
        "/.cache/uv",
        "Saved uv package and environment cache context.",
        "Review cache usage with uv tooling.",
    ),
    (
        "package_cache",
        "/Library/pnpm/store",
        "Saved pnpm package store context.",
        "Review store usage with pnpm tooling.",
    ),
];
impl Store {
    pub fn insights(&self, id: &str, q: &InsightsQuery, cancel: &AtomicBool) -> io::Result<Value> {
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
        let known = [
            "build_output",
            "dependencies",
            "build_hint",
            "dependency_hint",
            "cache_hint",
            "unclassified",
        ];
        if q.category
            .as_deref()
            .is_some_and(|c| !known.contains(&c) && !CATALOG.iter().any(|r| r.0 == c))
        {
            return Err(invalid("unknown insights category"));
        }
        let mode = if q.category.is_some() {
            "candidates"
        } else {
            "categories"
        };
        let binding = format!(
            "{VERSION}:{id}:{}:{}:{mode}:{}",
            q.metric.name(),
            q.entry,
            q.category.as_deref().unwrap_or("all")
        );
        let offset = cursor_offset(q.cursor.as_deref(), &binding)?;
        let db = Db::analysis()?;
        let manifest = self.attach(&db, id, "entries")?;
        let selected = db.query(&format!("SELECT entry_id,hex(path_key) AS key,coverage_complete,path_display,hex(path_bytes) AS raw_path,(name_bytes IS NOT NULL AND path_key IS NOT NULL AND enumeration_complete IS NOT NULL) AS context_available FROM entries_saved WHERE entry_id={}",q.entry),Some(cancel))?.row(0)?;
        if selected["entry_id"].is_null() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "entry not found"));
        }
        self.restrict(&db)?;
        // Materialize only candidate directories and their interval boundaries.
        // An ASOF join assigns each leaf once, without expanding file/ancestor pairs.
        let mut catalog = String::new();
        for (i, (bucket, suffix, reason, guidance)) in CATALOG.iter().enumerate() {
            catalog.push_str(&format!(" UNION ALL SELECT entry_id,path_key,{} AS bucket,{} AS rule,{} AS explanation,{} AS guidance,100 AS priority,coverage_complete AND coalesce(enumeration_complete,false) AS context_complete FROM entries_saved WHERE kind='directory' AND ends_with(hex(path_bytes),{})",literal(bucket),literal(&format!("catalog-{i}")),literal(reason),literal(guidance),literal(&hex(suffix.as_bytes()).to_uppercase())));
        }
        db.exec_cancel(&format!(r#"CREATE TEMP TABLE insight_scopes AS
SELECT s.entry_id,s.path_key,
 CASE s.rule WHEN 'rust_target' THEN 'build_output' WHEN 'node_dependencies' THEN 'dependencies' WHEN 'cache_hint' THEN 'cache_hint' WHEN 'build_hint' THEN 'build_hint' WHEN 'dependency_hint' THEN 'dependency_hint' ELSE 'unclassified' END AS bucket,
 s.rule, CASE s.rule WHEN 'rust_target' THEN 'Probable Rust output beside a saved regular Cargo.toml.' WHEN 'node_dependencies' THEN 'Probable dependencies beside a saved regular package.json.' WHEN 'build_hint' THEN 'Directory name is only a build output hint; purpose unverified.' WHEN 'dependency_hint' THEN 'Directory name is only a dependency hint; purpose unverified.' WHEN 'cache_hint' THEN 'Directory name is only a cache hint; purpose unverified.' ELSE 'No supported contextual classification; personal data and backups are not junk.' END AS explanation,
 CASE s.rule WHEN 'rust_target' THEN 'Review build artifacts with Cargo tooling.' WHEN 'node_dependencies' THEN 'Review dependencies with the project package manager.' ELSE 'Review the saved paths and their purpose manually.' END AS guidance,0 AS priority,coalesce(s.coverage_complete AND s.enumeration_complete AND coalesce(s.parent_complete,true),false) AS context_complete
FROM entries_scopes s WHERE s.rule IN ('rust_target','node_dependencies','project_boundary','unverified_name','cache_hint','build_hint','dependency_hint')
AND (s.rule<>'project_boundary' OR NOT EXISTS (SELECT 1 FROM entries_scopes d WHERE d.rule='node_dependencies' AND starts_with(hex(s.path_key),hex(d.path_key))))
{catalog};
CREATE TEMP TABLE insight_events AS
WITH boundaries AS (SELECT path_key AS key FROM insight_scopes UNION SELECT unhex(substr(hex(path_key),1,length(hex(path_key))-2)||'01') FROM insight_scopes WHERE octet_length(path_key)>0)
SELECT b.key,s.entry_id AS candidate_id,s.bucket,s.rule,s.explanation,s.guidance,s.context_complete
FROM boundaries b LEFT JOIN LATERAL (SELECT * FROM insight_scopes s WHERE starts_with(hex(b.key),hex(s.path_key)) ORDER BY priority DESC,octet_length(s.path_key) DESC,entry_id,rule LIMIT 1) s ON true;
"#),cancel)?;
        let scope = selected["key"].as_str().unwrap_or("");
        let filter = if selected["key"].is_null() {
            let raw = selected["raw_path"]
                .as_str()
                .ok_or_else(|| invalid("saved raw scope path is unavailable"))?;
            format!(
                "(hex(e.path_bytes)={} OR starts_with(hex(e.path_bytes),{}))",
                literal(raw),
                literal(&if raw == "2F" {
                    raw.to_owned()
                } else {
                    format!("{raw}2F")
                })
            )
        } else if scope.is_empty() {
            "true".into()
        } else {
            format!("starts_with(hex(e.path_key),{})", literal(scope))
        };
        let metric = metric_column(q.metric);
        let observed = observed_column(q.metric);
        db.exec_cancel(&format!(r#"CREATE TEMP VIEW insight_leaves AS
SELECT e.entry_id,coalesce(c.candidate_id,{entry}) AS candidate_id,coalesce(c.bucket,'unclassified') AS category,coalesce(c.rule,'unknown') AS rule,
coalesce(c.explanation,'No supported contextual classification; personal data and backups are not junk.') AS explanation,
coalesce(c.guidance,'Review the saved paths and their purpose manually.') AS guidance,
coalesce(c.context_complete,true) AS context_complete,e.{metric} AS bytes,e.{observed} AS observed,e.coverage_complete
FROM entries_saved e ASOF LEFT JOIN insight_events c ON e.path_key>=c.key WHERE e.kind='file' AND {filter};"#,entry=q.entry),cancel)?;
        let context_available = selected["context_available"] == true;
        let complete = selected["coverage_complete"] == true;
        let category_filter = q
            .category
            .as_ref()
            .map(|c| format!("WHERE category={}", literal(c)))
            .unwrap_or_default();
        let groups = if q.category.is_some() {
            "category,candidate_id,rule,explanation,guidance"
        } else {
            "category"
        };
        let detail = if q.category.is_some() {
            ",candidate_id AS entry_id,candidate_id AS candidate_entry_id,rule AS rule_id,explanation,guidance"
        } else {
            ",CASE WHEN count(DISTINCT rule)=1 THEN min(rule) ELSE 'multiple' END AS rule_id,CASE WHEN category='package_cache' THEN 'Saved package-manager cache contexts; candidate details identify each tool.' ELSE min(explanation) END AS explanation,CASE WHEN category='package_cache' THEN 'Review cache usage with the package managers identified in candidate details.' ELSE min(guidance) END AS guidance"
        };
        let aggregate = format!(
            r#"SELECT category{detail},CASE WHEN {complete} AND bool_and(coverage_complete) THEN sum(bytes) END AS bytes,
sum(observed) AS observed_lower_bound_bytes,count(*) AS file_path_count,
{complete} AND bool_and(coverage_complete) AS coverage_complete,CASE WHEN NOT {context_available} THEN 'context_unavailable' WHEN NOT bool_and(context_complete) THEN 'partial' ELSE 'complete' END AS context_coverage FROM insight_leaves {category_filter} GROUP BY {groups}"#
        );
        let sql = if q.category.is_some() {
            format!("SELECT a.*,CASE WHEN starts_with(hex(d.path_key),{scope_literal}) THEN a.entry_id ELSE {query_entry} END AS drilldown_entry_id,'insights-1' AS rule_version,d.path_display,hex(d.path_bytes) AS path_bytes_hex,concat('saved_metadata:entry=',a.entry_id) AS evidence FROM ({aggregate}) a JOIN entries_saved d ON d.entry_id=a.entry_id ORDER BY observed_lower_bound_bytes DESC,a.entry_id,rule_id",scope_literal=literal(scope),query_entry=q.entry)
        } else {
            format!("SELECT *, 'insights-1' AS rule_version,'saved_metadata:category_candidates' AS evidence FROM ({aggregate}) ORDER BY observed_lower_bound_bytes DESC,category")
        };
        let mut answer = envelope(&manifest, "insights", q.metric);
        answer["insights_version"] = json!(VERSION);
        answer["context_coverage"] = json!(if context_available {
            "available"
        } else {
            "context_unavailable"
        });
        answer["mode"] = json!(mode);
        answer["query_entry_id"] = json!(q.entry.to_string());
        answer["category"] = json!(q.category);
        answer["measured_at"] = manifest["ended_at"].clone();
        answer["coverage"]["complete"] = json!(complete);
        answer["scope"]["entry_id"] = json!(q.entry.to_string());
        answer["scope"]["path_display"] = selected["path_display"].clone();
        if !complete {
            answer["status"] = json!("partial");
        }
        answer["totals"] = db.query(&format!("SELECT CASE WHEN {complete} AND coalesce(bool_and(coverage_complete),true) THEN coalesce(sum(bytes),0) END AS bytes,coalesce(sum(observed),0) AS observed_lower_bound_bytes,count(*) AS file_path_count FROM insight_leaves"),Some(cancel))?.row(0)?;
        answer["count_note"] = json!("file_path_count counts regular-file paths, including hardlinks; it is not a count of physical identities. Allocated bytes keep the snapshot hardlink owner, even outside this scope.");
        answer["attribution_note"] = json!("Metadata does not prove process attribution or deletability. Guidance is non-executing; observed bytes are not reclaimable bytes.");
        run_query(&db, &sql, q.limit, offset, cancel, &mut answer)?;
        if let Some(next) = answer["next_offset"].as_u64() {
            answer["next_cursor"] = json!(format!("{binding}:{next}"));
        }
        compact_response(answer, Some(&binding))
    }
}
