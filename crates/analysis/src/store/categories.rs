//! Read-time rules. Saved categories are legacy hints; never rewrite measurements.
use crate::duck::Db;
use std::io;

pub(super) const FIELDS: &str = "category,category_rule_id,category_rule_version,category_reason,category_coverage,category_evidence_source";

pub(super) fn attach(db: &Db, table: &str, context_available: bool) -> io::Result<()> {
    // Keep contexts as lazy views of candidate directories, not a recursive
    // expansion of every entry/ancestor pair. Only metadata columns are read.
    let sql = format!(
        r#"
CREATE VIEW {table}_markers AS
SELECT parent_id,
 min(entry_id) FILTER (WHERE name_bytes='Cargo.toml'::BLOB) AS cargo,
 min(entry_id) FILTER (WHERE name_bytes='package.json'::BLOB) AS node
FROM {table}_saved WHERE kind='file' AND name_bytes IN ('Cargo.toml'::BLOB,'package.json'::BLOB)
GROUP BY parent_id;
CREATE VIEW {table}_scopes AS
SELECT d.entry_id,d.path_key,d.coverage_complete,d.enumeration_complete,p.enumeration_complete AS parent_complete,
 CASE
 WHEN own.cargo IS NOT NULL OR own.node IS NOT NULL THEN 'project_boundary'
 WHEN d.name_bytes='target'::BLOB AND sibling.cargo IS NOT NULL THEN 'rust_target'
 WHEN d.name_bytes='node_modules'::BLOB AND sibling.node IS NOT NULL THEN 'node_dependencies'
 WHEN d.name_bytes IN ('target'::BLOB,'node_modules'::BLOB) THEN 'unverified_name'
 WHEN d.name_bytes IN ('.next'::BLOB,'build'::BLOB,'DerivedData'::BLOB) THEN 'build_hint'
 WHEN d.name_bytes='.venv'::BLOB THEN 'dependency_hint'
 WHEN d.name_bytes IN ('.cache'::BLOB,'Caches'::BLOB,'__pycache__'::BLOB) THEN 'cache_hint'
 WHEN d.name_bytes='.git'::BLOB THEN 'git_hint'
 WHEN d.name_bytes='.codex'::BLOB THEN 'agent_hint'
 END AS rule,
 CASE WHEN own.cargo IS NOT NULL OR own.node IS NOT NULL THEN coalesce(own.cargo,own.node)
 WHEN d.name_bytes='target'::BLOB THEN sibling.cargo
 WHEN d.name_bytes='node_modules'::BLOB THEN sibling.node END AS marker
FROM {table}_saved d
LEFT JOIN {table}_markers own ON own.parent_id=d.entry_id
LEFT JOIN {table}_markers sibling ON sibling.parent_id=d.parent_id
LEFT JOIN {table}_saved p ON p.entry_id=d.parent_id
WHERE d.kind='directory' AND (own.parent_id IS NOT NULL OR d.name_bytes IN
 ('target'::BLOB,'node_modules'::BLOB,'.next'::BLOB,'build'::BLOB,'DerivedData'::BLOB,'.venv'::BLOB,'.cache'::BLOB,'Caches'::BLOB,'__pycache__'::BLOB,'.git'::BLOB,'.codex'::BLOB));
CREATE VIEW {table} AS
SELECT e.* EXCLUDE(category), e.category AS saved_category,
 CASE chosen.rule WHEN 'rust_target' THEN 'build_output' WHEN 'node_dependencies' THEN 'dependencies'
 WHEN 'build_hint' THEN 'build_output' WHEN 'dependency_hint' THEN 'dependencies'
 WHEN 'cache_hint' THEN 'cache' WHEN 'git_hint' THEN 'git_data' WHEN 'agent_hint' THEN 'agent_data'
 WHEN 'docker_hint' THEN 'docker_disk' ELSE 'unclassified' END AS category,
 coalesce(chosen.rule,'unknown') AS category_rule_id,
 'context-1' AS category_rule_version,
 CASE chosen.rule
 WHEN 'rust_target' THEN 'Probable Rust output: directory beside regular Cargo.toml.'
 WHEN 'node_dependencies' THEN 'Probable dependencies: directory beside regular package.json.'
 WHEN 'project_boundary' THEN 'Project boundary: outer category does not apply.'
 WHEN 'unverified_name' THEN 'Name alone lacks a saved regular sibling marker.'
 WHEN 'docker_hint' THEN 'Regular Docker.raw name hint; purpose unverified.'
 WHEN 'build_hint' THEN 'Directory name hints at build output; purpose unverified.'
 WHEN 'dependency_hint' THEN 'Directory name hints at dependencies; purpose unverified.'
 WHEN 'cache_hint' THEN 'Directory name hints at cache; purpose unverified.'
 WHEN 'git_hint' THEN 'Directory name hints at Git data; contents unverified.'
 WHEN 'agent_hint' THEN 'Directory name hints at agent data; contents unverified.'
 ELSE 'No applicable saved metadata evidence.' END AS category_reason,
 CASE WHEN NOT {context_available} THEN 'context_unavailable'
 WHEN e.parent_id IS NULL AND scope_id IS NULL THEN 'context_unavailable'
 WHEN NOT e.coverage_complete OR ep.enumeration_complete=false OR scope_complete=false OR parent_complete=false THEN 'partial'
 WHEN chosen.rule='unverified_name' AND parent_complete IS NULL THEN 'context_unavailable'
 ELSE 'complete' END AS category_coverage,
 CASE WHEN chosen.rule='docker_hint' THEN concat('saved_name:entry=',e.entry_id)
 WHEN scope_id IS NOT NULL THEN concat('saved_metadata:entry=',scope_id,CASE WHEN marker IS NOT NULL THEN concat(',marker=',marker) ELSE '' END)
 ELSE 'saved_metadata:no_match' END AS category_evidence_source
FROM {table}_saved e
LEFT JOIN {table}_saved ep ON ep.entry_id=e.parent_id
LEFT JOIN LATERAL (
 SELECT s.entry_id AS scope_id,s.coverage_complete AS scope_complete,s.parent_complete,s.marker,s.rule
 FROM {table}_scopes s
 WHERE starts_with(hex(e.path_key),hex(s.path_key))
 AND (s.rule<>'project_boundary' OR NOT EXISTS (
 SELECT 1 FROM {table}_scopes dependency WHERE dependency.rule='node_dependencies'
 AND starts_with(hex(s.path_key),hex(dependency.path_key))))
 ORDER BY octet_length(s.path_key) DESC,s.entry_id LIMIT 1
) context ON true
LEFT JOIN LATERAL (SELECT CASE WHEN e.kind='file' AND e.name_bytes='Docker.raw'::BLOB THEN 'docker_hint' ELSE context.rule END AS rule) chosen ON true;
"#
    );
    let delimiter = format!("CREATE VIEW {table} AS\n");
    let projection = sql
        .split_once(&delimiter)
        .unwrap()
        .1
        .trim_end_matches(';')
        .trim();
    db.exec(&sql)?;
    // Native navigation supplies IDs chosen before classification. The explicit
    // materialization fence prevents DuckDB decorrelation from expanding a
    // point lookup into classification of the full snapshot.
    let bounded = projection.replace(&format!("FROM {table}_saved e\n"), "FROM picked e\n");
    db.exec(&format!("CREATE MACRO {table}_classified(ids) AS TABLE WITH picked AS MATERIALIZED (SELECT * FROM {table}_saved WHERE entry_id IN (SELECT unnest(ids))) {bounded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use macscan_engine::{scan, Query, QueryKind, ScanOptions, STORE_BUDGET};
    use std::{fs, sync::atomic::AtomicBool};

    #[test]
    fn legacy_snapshot_without_optional_context_is_readable_and_unverified() {
        let temp =
            std::env::temp_dir().join(format!("macscan-legacy-context-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let root = temp.join("root");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("Cargo.toml"), b"x").unwrap();
        let data = scan(&root, &ScanOptions::default(), &AtomicBool::new(false)).unwrap();
        let store = Store::open(&temp.join("store"), true).unwrap();
        store
            .save_with_cancel(&data, STORE_BUDGET, &AtomicBool::new(false))
            .unwrap();
        let part = store
            .path()
            .join(&data.id)
            .join("entries/part-00000.parquet");
        let full_bytes = fs::read(&part).unwrap();
        let replacement = temp.join("old.parquet");
        let db = Db::new().unwrap();
        db.exec(&format!("COPY (SELECT * EXCLUDE(name_bytes,enumeration_complete) FROM read_parquet({})) TO {} (FORMAT PARQUET)",crate::duck::literal(part.to_str().unwrap()),crate::duck::literal(replacement.to_str().unwrap()))).unwrap();
        fs::rename(replacement, &part).unwrap();
        let bytes = fs::read(&part).unwrap();
        let result = store
            .report(
                &data.id,
                &Query {
                    kind: QueryKind::Inspect,
                    ..Query::default()
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(result["records"][0]["category"], "unclassified");
        assert_eq!(
            result["records"][0]["category_coverage"],
            "context_unavailable"
        );
        assert_eq!(fs::read(&part).unwrap(), bytes);
        fs::write(&part, full_bytes).unwrap();
        store.attach(&db, &data.id, "entries").unwrap();
        let profile = temp.join("point-profile.json");
        db.exec(&format!(
            "PRAGMA enable_profiling='json'; SET profiling_output={}",
            crate::duck::literal(profile.to_str().unwrap())
        ))
        .unwrap();
        db.query("SELECT category FROM entries_classified((SELECT list(entry_id) FROM (SELECT entry_id FROM entries_saved ORDER BY entry_id LIMIT 1 OFFSET 1)))",None).unwrap().row(0).unwrap();
        db.exec("PRAGMA disable_profiling").unwrap();
        let plan = fs::read_to_string(profile).unwrap();
        fn picked_rows(value: &serde_json::Value) -> Option<u64> {
            if value["extra_info"]["CTE Name"] == "picked" {
                return value["children"][0]["intermediate_rows"].as_u64();
            }
            value.as_object()?.values().find_map(|v| {
                if let Some(a) = v.as_array() {
                    a.iter().find_map(picked_rows)
                } else {
                    picked_rows(v)
                }
            })
        }
        let profile: serde_json::Value = serde_json::from_str(&plan).unwrap();
        assert_eq!(picked_rows(&profile), Some(1));
        eprintln!(
            "Bounded point classification: picked cardinality = {:?}; profile:\n{plan}",
            picked_rows(&profile)
        );
        fs::remove_dir_all(temp).unwrap();
    }
}
