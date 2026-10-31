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
        "Sandsynlige Xcode-builddata ud fra den gemte mappesti.",
        "Gennemgå Derived Data i Xcodes indstillinger.",
    ),
    (
        "xcode_archives",
        "/Library/Developer/Xcode/Archives",
        "Gemt område med Xcode-arkiver; arkiver kan være værdifulde udgivelser.",
        "Gennemgå arkiver i Xcode Organizer.",
    ),
    (
        "simulator_devices",
        "/Library/Developer/CoreSimulator/Devices",
        "Gemte simulatordata med app- og enhedstilstand.",
        "Gennemgå enheder i Xcodes Devices and Simulators.",
    ),
    (
        "docker_disk",
        "/Library/Containers/com.docker.docker/Data/vms",
        "Gemt lagerområde til Docker-VM; værtsallokering er forskellig fra internt forbrug.",
        "Gennemgå diskforbruget i Docker Desktop.",
    ),
    (
        "orbstack_disk",
        "/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data",
        "Gemte OrbStack-diskdata med VM- og containertilstand.",
        "Gennemgå lageret i OrbStack.",
    ),
    (
        "ollama_models",
        "/.ollama/models",
        "Gemt Ollama-modelområde; modeller kan være bevaret med vilje.",
        "Gennemgå installerede modeller i Ollama.",
    ),
    (
        "huggingface_models",
        "/.cache/huggingface/hub",
        "Gemt Hugging Face-modelområde, særskilt fra generelle cachehints.",
        "Gennemgå modeller med Hugging Faces cacheværktøj.",
    ),
    (
        "lmstudio_models",
        "/.lmstudio/models",
        "Gemt LM Studio-modelområde; modeller kan være bevaret med vilje.",
        "Gennemgå hentede modeller i LM Studio.",
    ),
    (
        "lmstudio_models",
        "/.cache/lm-studio/models",
        "Gemt LM Studio-modelområde inde i en cachemappe.",
        "Gennemgå hentede modeller i LM Studio.",
    ),
    (
        "package_cache",
        "/.npm/_cacache",
        "Gemt kontekst for npm-pakkecache.",
        "Gennemgå cacheforbruget med npm.",
    ),
    (
        "package_cache",
        "/.cargo/registry/cache",
        "Gemt kontekst for Cargos pakkearkivcache.",
        "Gennemgå behovet for pakkecache med Cargo.",
    ),
    (
        "package_cache",
        "/.cargo/git/db",
        "Gemt kontekst for Cargos Git-afhængighedscache.",
        "Gennemgå behovet for Git-afhængighedscache med Cargo.",
    ),
    (
        "package_cache",
        "/Library/Caches/pip",
        "Gemt kontekst for pip-pakkecache.",
        "Gennemgå cacheforbruget med pip.",
    ),
    (
        "package_cache",
        "/.cache/pip",
        "Gemt kontekst for pip-pakkecache.",
        "Gennemgå cacheforbruget med pip.",
    ),
    (
        "package_cache",
        "/Library/Caches/Homebrew",
        "Gemt kontekst for Homebrews downloadcache.",
        "Gennemgå cacheforbruget med Homebrew.",
    ),
    (
        "package_cache",
        "/.bun/install/cache",
        "Gemt kontekst for Bun-pakkecache.",
        "Gennemgå cacheforbruget med Bun.",
    ),
    (
        "package_cache",
        "/.cache/uv",
        "Gemt kontekst for uvs pakke- og miljøcache.",
        "Gennemgå cacheforbruget med uv.",
    ),
    (
        "package_cache",
        "/Library/pnpm/store",
        "Gemt kontekst for pnpm-pakkelager.",
        "Gennemgå pakkelageret med pnpm.",
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
        prepare_events(&db, "entries", "insight", cancel)?;
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
coalesce(c.explanation,'Ingen underbygget kontekstklassifikation; personlige data og sikkerhedskopier er ikke affald.') AS explanation,
coalesce(c.guidance,'Gennemgå de gemte stier og deres formål manuelt.') AS guidance,
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
            ",CASE WHEN count(DISTINCT rule)=1 THEN min(rule) ELSE 'multiple' END AS rule_id,CASE WHEN category='package_cache' THEN 'Gemte pakkecacher; stidetaljerne angiver det enkelte værktøj.' ELSE min(explanation) END AS explanation,CASE WHEN category='package_cache' THEN 'Gennemgå cacheforbruget med den pakkehåndtering, der nævnes i stidetaljerne.' ELSE min(guidance) END AS guidance"
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
        answer["count_note"] = json!("Antallet tæller stier til almindelige filer, inklusive hardlinks, ikke fysiske filidentiteter. Allokerede bytes beholder målingens hardlink-ejer, også uden for dette udvalg.");
        answer["attribution_note"] = json!("Metadata beviser ikke, hvilken proces der skabte data, eller om de kan slettes. Forslag udfører ingen handlinger. Observerede bytes er ikke et mål for frigørbar plads.");
        run_query(&db, &sql, q.limit, offset, cancel, &mut answer)?;
        if let Some(next) = answer["next_offset"].as_u64() {
            answer["next_cursor"] = json!(format!("{binding}:{next}"));
        }
        compact_response(answer, Some(&binding))
    }
}

// Two consumers share the exact versioned catalog and contextual precedence.
pub(super) fn prepare_events(
    db: &Db,
    source: &str,
    output: &str,
    cancel: &AtomicBool,
) -> io::Result<()> {
    // Materialize only candidate directories and their interval boundaries.
    // An ASOF join assigns each leaf once, without expanding file/ancestor pairs.
    let mut catalog = String::new();
    for (i, (bucket, suffix, reason, guidance)) in CATALOG.iter().enumerate() {
        catalog.push_str(&format!(" UNION ALL SELECT entry_id,path_key,{} AS bucket,{} AS rule,{} AS explanation,{} AS guidance,100 AS priority,coverage_complete AND coalesce(enumeration_complete,false) AS context_complete FROM {source}_saved WHERE kind='directory' AND ends_with(hex(path_bytes),{})",literal(bucket),literal(&format!("catalog-{i}")),literal(reason),literal(guidance),literal(&hex(suffix.as_bytes()).to_uppercase())));
    }
    let sql = format!(
        r#"CREATE TEMP TABLE {output}_scopes AS
SELECT s.entry_id,s.path_key,
 CASE s.rule WHEN 'rust_target' THEN 'build_output' WHEN 'node_dependencies' THEN 'dependencies' WHEN 'cache_hint' THEN 'cache_hint' WHEN 'build_hint' THEN 'build_hint' WHEN 'dependency_hint' THEN 'dependency_hint' ELSE 'unclassified' END AS bucket,
 s.rule, CASE s.rule WHEN 'rust_target' THEN 'Sandsynligt Rust-buildoutput ved en gemt almindelig Cargo.toml-fil.' WHEN 'node_dependencies' THEN 'Sandsynlige afhængigheder ved en gemt almindelig package.json-fil.' WHEN 'build_hint' THEN 'Mappenavnet antyder buildoutput; formålet er ubekræftet.' WHEN 'dependency_hint' THEN 'Mappenavnet antyder afhængigheder; formålet er ubekræftet.' WHEN 'cache_hint' THEN 'Mappenavnet antyder cache; formålet er ubekræftet.' ELSE 'Ingen underbygget kontekstklassifikation; personlige data og sikkerhedskopier er ikke affald.' END AS explanation,
 CASE s.rule WHEN 'rust_target' THEN 'Gennemgå buildoutput med Cargo.' WHEN 'node_dependencies' THEN 'Gennemgå afhængigheder med projektets pakkehåndtering.' ELSE 'Gennemgå de gemte stier og deres formål manuelt.' END AS guidance,0 AS priority,coalesce(s.coverage_complete AND s.enumeration_complete AND coalesce(s.parent_complete,true),false) AS context_complete
FROM {source}_scopes s WHERE s.rule IN ('rust_target','node_dependencies','project_boundary','unverified_name','cache_hint','build_hint','dependency_hint')
AND (s.rule<>'project_boundary' OR NOT EXISTS (SELECT 1 FROM {source}_scopes d WHERE d.rule='node_dependencies' AND starts_with(hex(s.path_key),hex(d.path_key))))
{catalog};
CREATE TEMP TABLE {output}_events AS
WITH boundaries AS (SELECT path_key AS key FROM {output}_scopes UNION SELECT unhex(substr(hex(path_key),1,length(hex(path_key))-2)||'01') FROM {output}_scopes WHERE octet_length(path_key)>0)
SELECT b.key,s.entry_id AS candidate_id,s.bucket,s.rule,s.explanation,s.guidance,s.context_complete
FROM boundaries b LEFT JOIN LATERAL (SELECT * FROM {output}_scopes s WHERE starts_with(hex(b.key),hex(s.path_key)) ORDER BY priority DESC,octet_length(s.path_key) DESC,entry_id,rule LIMIT 1) s ON true;
"#
    );
    db.exec_cancel(&sql, cancel)
}
