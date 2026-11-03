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
    let bounded = projection
        .replace(&format!("FROM {table}_saved e\n"), "FROM picked e\n")
        .replace(&format!("LEFT JOIN {table}_saved ep"), "LEFT JOIN picked_parents ep")
        .replace(&format!("{table}_scopes"), "picked_scopes");
    // Classification only needs the picked rows' ancestor directories and the
    // regular project markers in/beside those directories. Bound both joins
    // before building scopes: a full saved-parent hash table can exceed the
    // navigation budget even for a single root lookup.
    let scopes = sql.split_once(&format!("CREATE VIEW {table}_scopes AS\n")).unwrap().1
        .split_once(&delimiter).unwrap().0.trim().trim_end_matches(';')
        .replace(&format!("FROM {table}_saved d"), "FROM picked_directories d")
        .replace(&format!("{table}_markers"), "picked_markers")
        .replace(&format!("LEFT JOIN {table}_saved p"), "LEFT JOIN scope_parents p");
    db.exec(&format!(r#"CREATE MACRO {table}_classified(ids) AS TABLE WITH
picked AS MATERIALIZED (SELECT * FROM {table}_saved WHERE entry_id IN (SELECT unnest(ids))),
ancestor_keys AS MATERIALIZED (
 SELECT DISTINCT unhex(substr(hex(path_key),1,i*2)) AS path_key
 FROM picked,range(0,octet_length(path_key)::BIGINT+1) offsets(i)
 WHERE i=0 OR substr(hex(path_key),i*2-1,2)='00'),
picked_directories AS MATERIALIZED (SELECT entry_id,parent_id,path_key,name_bytes,kind,coverage_complete,enumeration_complete
 FROM {table}_saved WHERE kind='directory' AND path_key IN (SELECT path_key FROM ancestor_keys)),
picked_parents AS MATERIALIZED (SELECT entry_id,enumeration_complete FROM {table}_saved WHERE entry_id IN (SELECT parent_id FROM picked)),
scope_parents AS MATERIALIZED (SELECT entry_id,enumeration_complete FROM {table}_saved WHERE entry_id IN (SELECT parent_id FROM picked_directories)),
picked_markers AS MATERIALIZED (SELECT parent_id,
 min(entry_id) FILTER (WHERE name_bytes='Cargo.toml'::BLOB) AS cargo,
 min(entry_id) FILTER (WHERE name_bytes='package.json'::BLOB) AS node
 FROM {table}_saved WHERE kind='file' AND name_bytes IN ('Cargo.toml'::BLOB,'package.json'::BLOB)
 AND parent_id IN (SELECT entry_id FROM picked_directories UNION SELECT parent_id FROM picked_directories) GROUP BY parent_id),
picked_scopes AS MATERIALIZED ({scopes})
{bounded}"#))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use macscan_engine::{scan, Query, QueryKind, ScanOptions, STORE_BUDGET};
    use std::{fs, sync::atomic::AtomicBool};

    #[test]
    fn point_classification_bounds_context_and_matches_full_rules() {
        let temp=std::env::temp_dir().join(format!("macscan-point-context-{}",std::process::id()));
        fs::create_dir(&temp).unwrap();let root=temp.join("root");
        // Irrelevant contexts must not enter a point classification's joins.
        for n in 0..40 {let dir=root.join(format!("other-{n}/target"));fs::create_dir_all(&dir).unwrap();fs::write(dir.parent().unwrap().join("Cargo.toml"),b"x").unwrap();}
        for path in ["rust/target/a", "node/node_modules/pkg/build", "cache/.cache/project/plain", "unmarked/target", "linked/target", "wrong-kind/target"] {fs::create_dir_all(root.join(path)).unwrap();}
        for path in ["rust/Cargo.toml","node/package.json","node/node_modules/pkg/package.json","cache/.cache/project/package.json","rust/target/a/object","node/node_modules/pkg/build/object","cache/.cache/project/plain/file","unmarked/target/object"] {fs::write(root.join(path),b"x").unwrap();}
        std::os::unix::fs::symlink("missing",root.join("linked/Cargo.toml")).unwrap();fs::create_dir(root.join("wrong-kind/Cargo.toml")).unwrap();
        let mut data=scan(&root,&ScanOptions::default(),&AtomicBool::new(false)).unwrap();
        let raw=data.entries.iter().position(|e|e.name==b"object").unwrap();data.entries[raw].name=b"object\xff\x10\x0b".to_vec();
        let partial=data.entries.iter().position(|e|e.name==b"unmarked").unwrap();data.entries[partial].enumeration_complete=false;data.entries[partial].complete=false;
        let store=Store::open(&temp.join("store"),true).unwrap();store.save_with_cancel(&data,STORE_BUDGET,&AtomicBool::new(false)).unwrap();
        let db=Db::analysis().unwrap();store.attach(&db,&data.id,"entries").unwrap();
        let fields=format!("entry_id,{FIELDS}");
        // Compare every saved entry with the existing full-context rule view,
        // including nested dependency project boundaries and partial markers.
        let mut expected=db.query(&format!("SELECT {fields} FROM entries ORDER BY entry_id"),None).unwrap();
        for i in 0..expected.len() {
            let row=expected.row(i).unwrap();let id=row["entry_id"].as_str().unwrap();
            let actual=db.query(&format!("SELECT {fields} FROM entries_classified([{id}])"),None).unwrap().row(0).unwrap();
            assert_eq!(actual,row,"entry {id}");
        }
        let ids=(0..data.entries.len()).map(|i|i.to_string()).collect::<Vec<_>>().join(",");
        let mut batch=db.query(&format!("SELECT {fields} FROM entries_classified([{ids}]) ORDER BY entry_id"),None).unwrap();
        assert_eq!(batch.len(),expected.len());
        for i in 0..batch.len(){assert_eq!(batch.row(i).unwrap(),expected.row(i).unwrap());}
        let profile=temp.join("root-profile.json");
        db.exec(&format!("PRAGMA enable_profiling='json'; SET profiling_output={}",crate::duck::literal(profile.to_str().unwrap()))).unwrap();
        db.query(&format!("SELECT {fields} FROM entries_classified([0])"),None).unwrap().row(0).unwrap();db.exec("PRAGMA disable_profiling").unwrap();
        let plan:serde_json::Value=serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
        fn materialized_rows(v:&serde_json::Value,name:&str)->Option<u64>{
            if v["extra_info"]["CTE Name"]==name {return v["children"][0]["intermediate_rows"].as_u64();}
            v.as_object()?.values().find_map(|v|if let Some(a)=v.as_array(){a.iter().find_map(|v|materialized_rows(v,name))}else{materialized_rows(v,name)})
        }
        // This guards the actual OOM fault: joins used to build parent/context
        // state for the entire snapshot despite a single root being picked.
        assert_eq!(materialized_rows(&plan,"picked_directories"),Some(1));
        assert_eq!(materialized_rows(&plan,"scope_parents"),Some(0));
        assert_eq!(materialized_rows(&plan,"picked_parents"),Some(0));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn legacy_snapshot_without_optional_context_is_readable_and_unverified() {
        let temp =
            std::env::temp_dir().join(format!("macscan-legacy-context-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let root = temp.join("root");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("Cargo.toml"), b"x").unwrap();
        fs::write(root.join("target/object"), b"abc").unwrap();
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
        db.exec(&format!("COPY (SELECT * EXCLUDE(name_bytes,enumeration_complete,path_key) FROM read_parquet({})) TO {} (FORMAT PARQUET)",crate::duck::literal(part.to_str().unwrap()),crate::duck::literal(replacement.to_str().unwrap()))).unwrap();
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
        let insights=store.insights(&data.id,&super::super::InsightsQuery::default(),&AtomicBool::new(false)).unwrap();
        assert_eq!(insights["context_coverage"],"context_unavailable");
        let target=data.entries.iter().position(|e| e.name==b"target").unwrap() as u32;
        let scoped=store.insights(&data.id,&super::super::InsightsQuery{entry:target,metric:macscan_engine::Metric::Logical,..Default::default()},&AtomicBool::new(false)).unwrap();
        assert_eq!(scoped["totals"]["bytes"],"3");assert_eq!(scoped["totals"]["file_path_count"],"1");
        assert!(insights["records"].as_array().unwrap().iter().all(|r|r["category"]=="unclassified"));
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
