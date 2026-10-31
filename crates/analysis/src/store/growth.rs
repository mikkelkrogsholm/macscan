//! Non-overlapping regular-file contributions between compatible saved measurements.
use super::*;
pub const VERSION: &str = "growth-1";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowthGrouping {
    Area,
    Category,
}
impl GrowthGrouping {
    fn name(self) -> &'static str {
        match self {
            Self::Area => "area",
            Self::Category => "category",
        }
    }
}
#[derive(Clone, Debug)]
pub struct GrowthQuery {
    pub entry: u32,
    pub metric: Metric,
    pub grouping: GrowthGrouping,
    pub files: bool,
    pub group: Option<String>,
    pub change: String,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for GrowthQuery {
    fn default() -> Self {
        Self {
            entry: 0,
            metric: Metric::Allocated,
            grouping: GrowthGrouping::Area,
            files: false,
            group: None,
            change: "all".into(),
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}
impl Store {
    pub fn growth(
        &self,
        before: &str,
        after: &str,
        q: &GrowthQuery,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        self.growth_inner(before, after, q, cancel).map_err(|e| {
            if e.kind() != io::ErrorKind::Other { e } else {
                io::Error::new(e.kind(), format!("{e}; for resource-budget failures choose a smaller saved directory with --entry ID"))
            }
        })
    }
    fn growth_inner(
        &self,
        before: &str,
        after: &str,
        q: &GrowthQuery,
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
        if !["all", "grown", "shrunk", "added", "removed", "unknown"].contains(&q.change.as_str()) {
            return Err(invalid("unknown change filter"));
        }
        // Structured binding avoids delimiter ambiguities in arbitrary group keys.
        let binding = serde_json::to_string(&(
            VERSION,
            insights::VERSION,
            before,
            after,
            q.metric.name(),
            q.entry,
            q.grouping.name(),
            q.files,
            &q.group,
            &q.change,
        ))?;
        let offset = cursor_offset(q.cursor.as_deref(), &binding)?;
        let db = Db::comparison()?;
        let new = self.attach(&db, after, "entries")?;
        let old = self.attach(&db, before, "before_entries")?;
        compatible(&old, &new)?;
        apfs::pair(&old,&new,q.metric)?;
        for source in ["entries", "before_entries"] {
            if db.query(&format!("SELECT count(*) AS missing FROM {source}_saved WHERE path_key IS NULL OR name_bytes IS NULL OR enumeration_complete IS NULL"),Some(cancel))?.row(0)?["missing"] != "0" {
                return Err(io::Error::new(io::ErrorKind::Unsupported,"saved raw/context metadata unavailable; create two current-format measurements"));
            }
        }
        let selected = db.query(&format!("SELECT entry_id,path_display,hex(path_bytes) AS path_bytes_hex,hex(path_key) AS key,coverage_complete FROM entries_saved WHERE entry_id={}",q.entry),Some(cancel))?.row(0)?;
        if selected["entry_id"].is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "AFTER scope entry not found",
            ));
        }
        let key = selected["key"].as_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "saved scope path key unavailable; create two current-format measurements",
            )
        })?;
        // A missing BEFORE anchor is never widened to root. Coverage determines
        // whether its empty region is provably new or simply unobserved.
        let mut prior_rows = db.query(&format!("SELECT entry_id,path_display,hex(path_bytes) AS path_bytes_hex,coverage_complete FROM before_entries_saved WHERE path_key=unhex({})",literal(key)),Some(cancel))?;
        let prior = if prior_rows.len() == 0 {
            json!({"entry_id":null,"path_display":null,"path_bytes_hex":null,"coverage_complete":null})
        } else {
            prior_rows.row(0)?
        };
        let before_complete = if !prior["entry_id"].is_null() {
            prior["coverage_complete"] == true
        } else {
            db.query(&format!("SELECT complete FROM before_entries_coverage WHERE path_key<=unhex({}) ORDER BY path_key DESC LIMIT 1",literal(key)),Some(cancel))?.row(0)?["complete"] == true
        };
        let complete = before_complete && selected["coverage_complete"] == true;
        db.exec(&changes_view(
            old["status"] == "complete",
            new["status"] == "complete", q.metric==Metric::ApfsDataClone,
        ))?;
        self.restrict(&db)?;
        insights::prepare_events(&db, "entries", "growth_after", cancel)?;
        insights::prepare_events(&db, "before_entries", "growth_before", cancel)?;
        let observed = observed_column(q.metric);
        for (source, events, view) in [
            ("entries", "growth_after", "after_categories"),
            ("before_entries", "growth_before", "before_categories"),
        ] {
            db.exec_cancel(&format!("CREATE TEMP VIEW {view} AS SELECT e.path_key,hex(e.path_bytes) AS path_bytes_hex,e.{observed} AS observed_bytes,coalesce(c.context_complete,true) AS context_complete,coalesce(c.bucket,'unclassified') AS category FROM {source}_saved e ASOF LEFT JOIN {events}_events c ON e.path_key>=c.key WHERE e.kind='file'"),cancel)?;
        }
        let metric=match q.metric {Metric::Logical=>"logical",Metric::ApfsDataClone=>"apfs_data_clone",Metric::Allocated=>"allocated",Metric::Private=>return Err(invalid("private metric is unavailable"))};
        let area = format!(
            "concat({},regexp_extract(substr(hex(c.path_key),{}),'^((?:[0-9A-F]{{2}})*?00)',1))",
            literal(key),
            key.len() + 1
        );
        let group = match q.grouping {
            GrowthGrouping::Area => area,
            GrowthGrouping::Category => {
                "CASE WHEN c.kind_after='file' THEN ac.category ELSE bc.category END".into()
            }
        };
        // Type transitions retain only the regular-file side. Directory subtree
        // values from changes are deliberately discarded before subtraction.
        db.exec_cancel(&format!(r#"CREATE TEMP VIEW growth_files AS WITH sides AS (
SELECT c.path_key,c.path_display,c.before_entry_id,c.after_entry_id,
CASE WHEN c.kind_before IS NOT NULL AND c.kind_before<>'file' THEN 0 ELSE c.{metric}_before END AS before_bytes,
CASE WHEN c.kind_after IS NOT NULL AND c.kind_after<>'file' THEN 0 ELSE c.{metric}_after END AS after_bytes,
coalesce(ac.path_bytes_hex,bc.path_bytes_hex) AS path_bytes_hex,
CASE WHEN c.kind_before='file' THEN bc.observed_bytes ELSE 0 END AS observed_before_bytes,
CASE WHEN c.kind_after='file' THEN ac.observed_bytes ELSE 0 END AS observed_after_bytes,
CASE WHEN c.kind_after='file' THEN ac.context_complete ELSE bc.context_complete END AS context_complete,
{group} AS group_key,c.kind_before,c.kind_after
FROM changes c LEFT JOIN before_categories bc ON c.path_key=bc.path_key LEFT JOIN after_categories ac ON c.path_key=ac.path_key
WHERE starts_with(hex(c.path_key),{scope}) AND (c.kind_before='file' OR c.kind_after='file')
), deltas AS (SELECT *,CAST(after_bytes AS HUGEINT)-CAST(before_bytes AS HUGEINT) AS delta_bytes FROM sides)
SELECT *,CASE WHEN delta_bytes IS NULL THEN 'unknown' WHEN kind_before IS DISTINCT FROM 'file' THEN 'added' WHEN kind_after IS DISTINCT FROM 'file' THEN 'removed' WHEN delta_bytes>0 THEN 'grown' ELSE 'shrunk' END AS change
FROM deltas WHERE delta_bytes IS NULL OR delta_bytes<>0 OR kind_before IS DISTINCT FROM 'file' OR kind_after IS DISTINCT FROM 'file';
"#,scope=literal(key)),cancel)?;
        let filters = format!(
            "{}{}",
            q.group
                .as_ref()
                .map(|g| format!(" AND group_key={}", literal(g)))
                .unwrap_or_default(),
            if q.change == "all" {
                String::new()
            } else {
                format!(" AND change={}", literal(&q.change))
            }
        );
        let counts = "count(*) AS file_path_count,count(*) FILTER(WHERE change='added') AS new_count,count(*) FILTER(WHERE change='removed') AS removed_count,count(*) FILTER(WHERE change IN ('grown','shrunk')) AS changed_count,count(*) FILTER(WHERE change='unknown') AS unknown_count";
        let aggregates = format!("CASE WHEN {complete} AND count(*) FILTER(WHERE delta_bytes IS NULL)=0 THEN coalesce(sum(delta_bytes),0) END AS delta_bytes,CASE WHEN {complete} AND count(*) FILTER(WHERE before_bytes IS NULL)=0 THEN coalesce(sum(before_bytes),0) END AS changed_file_before_bytes,CASE WHEN {complete} AND count(*) FILTER(WHERE after_bytes IS NULL)=0 THEN coalesce(sum(after_bytes),0) END AS changed_file_after_bytes,coalesce(sum(observed_before_bytes),0) AS observed_changed_file_before_bytes,coalesce(sum(observed_after_bytes),0) AS observed_changed_file_after_bytes,CASE WHEN coalesce(bool_and(context_complete),true) THEN 'complete' ELSE 'partial' END AS context_coverage,{counts}");
        let sql = if q.files {
            format!("SELECT group_key,path_display,path_bytes_hex,hex(path_key) AS path_key_hex,before_entry_id,after_entry_id,CASE WHEN after_entry_id IS NOT NULL THEN {} ELSE {} END AS drilldown_scan_id,coalesce(after_entry_id,before_entry_id) AS drilldown_entry_id,before_bytes,after_bytes,observed_before_bytes,observed_after_bytes,delta_bytes,change,CASE WHEN context_complete THEN 'complete' ELSE 'partial' END AS context_coverage FROM growth_files WHERE true {filters} ORDER BY (delta_bytes IS NULL) DESC,abs(delta_bytes) DESC,path_key",literal(after),literal(before))
        } else {
            let detail = if q.grouping == GrowthGrouping::Area {
                ",coalesce(a.path_display,b.path_display) AS path_display,coalesce(hex(a.path_bytes),hex(b.path_bytes)) AS path_bytes_hex,a.entry_id AS after_entry_id,b.entry_id AS before_entry_id"
            } else {
                ",g.group_key AS category"
            };
            let joins = if q.grouping == GrowthGrouping::Area {
                " LEFT JOIN entries_saved a ON hex(a.path_key)=g.group_key LEFT JOIN before_entries_saved b ON hex(b.path_key)=g.group_key"
            } else {
                ""
            };
            format!("SELECT g.*{detail} FROM (SELECT group_key,{aggregates} FROM growth_files WHERE true {filters} GROUP BY group_key) g{joins} ORDER BY (delta_bytes IS NULL) DESC,abs(delta_bytes) DESC,g.group_key")
        };
        let mut answer = envelope(&new, "growth", q.metric);
        answer["growth_version"] = json!(VERSION);
        answer["rule_version"] = json!(insights::VERSION);
        answer["before_scan_id"] = json!(before);
        answer["grouping"] = json!(q.grouping.name());
        answer["mode"] = json!(if q.files { "files" } else { "groups" });
        answer["group"] = json!(q.group);
        answer["change"] = json!(q.change);
        answer["query_entry_id"] = json!(q.entry.to_string());
        answer["scope"] = json!({"after":selected,"before":prior});
        answer["coverage"] = json!({"complete":complete,"before_complete":before_complete,"after_complete":selected["coverage_complete"]});
        answer["status"] = json!(if complete { "complete" } else { "partial" });
        let ns = |m: &Value| {
            m["context"]["ended_unix_ns"]
                .as_str()
                .and_then(|s| s.parse::<u128>().ok())
        };
        answer["interval"] = json!({"before_started_at":old["started_at"],"before_measured_at":old["ended_at"],"after_started_at":new["started_at"],"after_measured_at":new["ended_at"],"before_ended_unix_ns":old["context"]["ended_unix_ns"],"after_ended_unix_ns":new["context"]["ended_unix_ns"],"elapsed_ns":ns(&old).zip(ns(&new)).and_then(|(a,b)|b.checked_sub(a)).map(|n|n.to_string())});
        let column = metric_column(q.metric);
        let observed = observed_column(q.metric);
        for (side, source, known) in [
            ("before", "before_entries", before_complete),
            ("after", "entries", selected["coverage_complete"] == true),
        ] {
            answer["totals"][side] = db.query(&format!("SELECT CASE WHEN {known} AND coalesce(bool_and(coverage_complete),true) THEN coalesce(sum({column}),0) END AS bytes,coalesce(sum({observed}),0) AS observed_lower_bound_bytes,count(*) AS file_path_count FROM {source}_saved WHERE kind='file' AND starts_with(hex(path_key),{})",literal(key)),Some(cancel))?.row(0)?;
        }
        // No inherited directory/APFS totals in this regular-file envelope.
        let totals = json!({"before":answer["totals"]["before"],"after":answer["totals"]["after"]});
        answer["totals"] = totals;
        answer["contributions"] = db
            .query(
                &format!("SELECT {aggregates} FROM growth_files WHERE true {filters}"),
                Some(cancel),
            )?
            .row(0)?;
        answer["accounting_note"] = json!("Kun ændrede almindelige filstier; mapper og øvrige typer er udeladt. Før/efterbidrag er ikke fulde gruppetotaler. Hardlink-ejerskab følger hver gemt måling. Flytning er fjernet + ny sti.");
        answer["attribution_note"] = json!("Kategori fra eftermålingen for bevarede/nye filer, fra førmålingen for fjernede filer (insights-1). Ændret kontekst alene skaber ingen bytevækst. Ukendt dækning er ikke nul; metadata beviser ikke proces eller frigørbar plads.");
        run_query(&db, &sql, q.limit, offset, cancel, &mut answer)?;
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
    fn legacy_missing_path_keys_is_explicitly_unsupported() {
        use macscan_engine::{scan, ScanOptions, STORE_BUDGET};
        let temp =
            std::env::temp_dir().join(format!("macscan-growth-legacy-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let root = temp.join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"abc").unwrap();
        let store = Store::open(&temp.join("store"), true).unwrap();
        let mut a = scan(&root, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
        a.id = "401-1".into();
        store
            .save_with_cancel(&a, STORE_BUDGET, &AtomicBool::new(false))
            .unwrap();
        let mut b = scan(&root, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
        b.id = "401-2".into();
        store
            .save_with_cancel(&b, STORE_BUDGET, &AtomicBool::new(false))
            .unwrap();
        let path = store.path().join(&a.id).join("entries/part-00000.parquet");
        let replacement = temp.join("legacy.parquet");
        let db = Db::new().unwrap();
        db.exec(&format!("COPY (SELECT * EXCLUDE(path_key,name_bytes,enumeration_complete) FROM read_parquet({})) TO {} (FORMAT PARQUET)",path_literal(&path).unwrap(),path_literal(&replacement).unwrap())).unwrap();
        fs::rename(replacement, path).unwrap();
        let error = store
            .growth(
                &a.id,
                &b.id,
                &GrowthQuery::default(),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error
            .to_string()
            .contains("saved raw/context metadata unavailable"));
        fs::remove_dir_all(temp).unwrap();
    }
}
