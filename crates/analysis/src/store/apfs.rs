//! Persisted, deliberately limited full-DATA sharing correction. No filesystem calls.
use super::*;
pub const VERSION: &str = "apfs-data-clone-1";
pub const LABEL: &str = "Begrænset DATA-klonkorrigeret allokering";
pub const FIELDS: [(&str, &str); 5] = [
    ("apfs_data_clone_bytes", "UBIGINT"),
    ("observed_apfs_data_clone_bytes", "UBIGINT"),
    ("own_apfs_data_clone_bytes", "UBIGINT"),
    ("apfs_data_clone_owner", "UBIGINT"),
    ("apfs_data_clone_reason", "VARCHAR"),
];
pub fn prepare(db: &Db, stop: &AtomicBool, valid: bool) -> io::Result<()> {
    // Eligibility is deliberately evaluated after hardlink ownership. The same
    // raw-path ordering is used by both stages; hardlink nonowners stay zero.
    db.exec_cancel("CREATE TABLE clone_inode_observations AS SELECT device_id,inode,coalesce(bool_and(clone_status='observed' AND clone_id IS NOT NULL AND clone_extended_flags IS NOT NULL AND clone_data_allocated_bytes IS NOT NULL AND clone_device_id IS NOT NULL AND clone_device_id=device_id AND clone_data_allocated_bytes<=own_allocated),false) AND min(clone_id)=max(clone_id) AND min(clone_extended_flags)=max(clone_extended_flags) AND min(clone_data_allocated_bytes)=max(clone_data_allocated_bytes) AND min(own_logical)=max(own_logical) AND min(own_allocated)=max(own_allocated) AS coherent FROM charged WHERE kind='file' AND (device_id,inode) IN (SELECT device_id,inode FROM charged WHERE kind='file' AND link_count>1) GROUP BY device_id,inode",stop)?;
    db.exec_cancel(&format!(r#"CREATE TABLE clone_candidates AS SELECT c.*,coalesce(h.coherent,true) AS inode_observations_coherent,
      ({valid} AND c.kind='file' AND c.charged_to IS NULL AND coalesce(h.coherent,true) AND clone_status='observed'
       AND clone_id>0 AND (clone_extended_flags & 64)=64
       AND clone_device_id=c.device_id AND clone_device_id=(SELECT device_id FROM raw WHERE entry_id=0)
       AND clone_data_allocated_bytes<=own_allocated) AS eligible
      FROM charged c LEFT JOIN clone_inode_observations h ON c.kind='file' AND c.device_id=h.device_id AND c.inode=h.inode"#), stop)?;
    db.exec_cancel("CREATE TABLE clone_groups AS SELECT clone_device_id,clone_id,arg_min(entry_id,path_bytes) AS owner,min(clone_data_allocated_bytes)=max(clone_data_allocated_bytes) AND min(own_logical)=max(own_logical) AS coherent FROM clone_candidates WHERE eligible GROUP BY clone_device_id,clone_id",stop)?;
    db.exec_cancel(&format!(r#"CREATE VIEW clone_charged AS SELECT c.*,
      CASE WHEN c.eligible AND g.coherent AND c.entry_id<>g.owner THEN c.own_accounted-c.clone_data_allocated_bytes ELSE c.own_accounted END::UBIGINT AS own_apfs_data_clone_bytes,
      CASE WHEN c.eligible AND g.coherent THEN g.owner END AS apfs_data_clone_owner,
      CASE WHEN c.kind<>'file' THEN 'nonfile_retained'
        WHEN NOT c.inode_observations_coherent THEN 'hardlink_observation_fallback'
        WHEN c.charged_to IS NOT NULL THEN 'hardlink_nonowner'
        WHEN NOT {valid} THEN 'filesystem_or_volume_fallback'
        WHEN c.clone_status IS DISTINCT FROM 'observed' THEN 'metadata_unavailable_fallback'
        WHEN c.clone_device_id IS NULL OR c.clone_device_id<>c.device_id OR c.clone_device_id<>(SELECT device_id FROM raw WHERE entry_id=0)
          OR c.clone_id IS NULL OR c.clone_extended_flags IS NULL OR c.clone_data_allocated_bytes IS NULL OR c.clone_data_allocated_bytes>c.own_allocated THEN 'invalid_metadata_fallback'
        WHEN c.eligible AND NOT g.coherent THEN 'inconsistent_group_fallback'
        WHEN c.eligible AND c.entry_id<>g.owner THEN 'full_data_nonowner'
        WHEN c.eligible THEN 'full_data_owner'
        ELSE 'limited_retained' END AS apfs_data_clone_reason
      FROM clone_candidates c LEFT JOIN clone_groups g ON c.eligible AND c.clone_device_id=g.clone_device_id AND c.clone_id=g.clone_id"#),stop)
}
pub fn metadata(db: &Db, stop: &AtomicBool, valid: bool) -> io::Result<Value> {
    let mut rows=db.query("SELECT apfs_data_clone_reason AS reason,count(*) AS count FROM clone_charged WHERE kind='file' GROUP BY apfs_data_clone_reason ORDER BY reason",Some(stop))?;
    let mut counts = serde_json::Map::new();
    let mut fallback = !valid;
    for i in 0..rows.len() {
        let row = rows.row(i)?;
        let reason = row["reason"].as_str().unwrap();
        fallback |= reason.ends_with("fallback");
        counts.insert(reason.into(), row["count"].clone());
    }
    // All regular rows, including hardlink aliases, must have coherent raw
    // observations for comparison. Aliases do not hide unavailable capture.
    let missing=db.query("SELECT count(*) AS n FROM charged WHERE kind='file' AND (clone_status IS DISTINCT FROM 'observed' OR clone_id IS NULL OR clone_extended_flags IS NULL OR clone_data_allocated_bytes IS NULL OR clone_device_id IS NULL OR clone_device_id<>device_id OR clone_data_allocated_bytes>own_allocated)",Some(stop))?.row(0)?;
    fallback |= missing["n"] != "0";
    Ok(
        json!({"algorithm_version":VERSION,"capture_version":macscan_engine::CLONE_CAPTURE_VERSION,"label":LABEL,
      "status":if fallback {"fallback"}else{"limited"},"comparison_ready":!fallback,
      "count_universe":"emitted_regular_file_rows_including_hardlink_aliases","reason_counts":counts,
      "unavailable_or_invalid_observations":missing["n"],
      "limitations":"Within measurement root; full DATA sharing only; partial sharing retained; non-DATA retained; global raw-path owners may lie outside selected scope; not unique, private or reclaimable space; live non-atomic observation"}),
    )
}
pub fn require(m: &Value, metric: Metric) -> io::Result<()> {
    if metric == Metric::ApfsDataClone
        && (m["context"]["clone_capture"]["requested"] != true
            || m["context"]["clone_capture"]["version"] != macscan_engine::CLONE_CAPTURE_VERSION
            || m["apfs_data_clone"]["algorithm_version"] != VERSION
            || m["apfs_data_clone"]["capture_version"] != macscan_engine::CLONE_CAPTURE_VERSION
            || m["apfs_data_clone"]["columns_available"] == false)
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported,"apfs-data-clone unavailable: rescan with --capture-apfs-clones; use logical or allocated for existing measurements"));
    }
    Ok(())
}
pub fn pair(a: &Value, b: &Value, metric: Metric) -> io::Result<()> {
    require(a, metric)?;
    require(b, metric)?;
    if metric == Metric::ApfsDataClone
        && [a, b].iter().any(|m| {
            m["status"] != "complete"
                || m["read_time_global_invalidation"] == true
                || m["apfs_data_clone"]["comparison_ready"] != true
        })
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported,"apfs-data-clone comparison requires complete compatible observations without fallback; use logical or allocated"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Db {
        let db = Db::new().unwrap();
        db.exec("CREATE TABLE charged(entry_id UBIGINT,kind VARCHAR,path_bytes BLOB,device_id UBIGINT,own_allocated UBIGINT,own_logical UBIGINT,own_accounted UBIGINT,charged_to UBIGINT,clone_status VARCHAR,clone_id UBIGINT,clone_extended_flags UBIGINT,clone_data_allocated_bytes UBIGINT,clone_device_id UBIGINT,inode UBIGINT,link_count UBIGINT);CREATE VIEW raw AS SELECT * FROM charged;INSERT INTO charged VALUES(0,'directory',''::BLOB,1,4096,0,4096,NULL,'missing',NULL,NULL,NULL,NULL,0,1)").unwrap();
        db
    }
    fn add(
        db: &Db,
        id: u64,
        path: &str,
        device: u64,
        allocated: u64,
        logical: u64,
        status: &str,
        key: &str,
        flags: &str,
        data: &str,
        clone_device: &str,
        hardlink: bool,
    ) {
        let accounted = if hardlink { 0 } else { allocated };
        let charge = if hardlink { "1" } else { "NULL" };
        let inode = if hardlink { 1 } else { id };
        let links = if hardlink || id == 1 { 2 } else { 1 };
        db.exec(&format!("INSERT INTO charged VALUES({id},'file',encode('{path}'),{device},{allocated},{logical},{accounted},{charge},'{status}',{key},{flags},{data},{clone_device},{inode},{links})")).unwrap();
    }
    fn get(db: &Db, id: u64) -> Value {
        db.query(&format!("SELECT own_apfs_data_clone_bytes AS bytes,apfs_data_clone_owner AS owner,apfs_data_clone_reason AS reason FROM clone_charged WHERE entry_id={id}"),None).unwrap().row(0).unwrap()
    }
    #[test]
    fn hardlinks_first_full_data_only_raw_path_owner_and_exact_numbers() {
        let db = db();
        let n = 9_007_199_254_741_001;
        add(
            &db,
            1,
            "z",
            1,
            n + 65536,
            n,
            "observed",
            "7",
            "65",
            &n.to_string(),
            "1",
            false,
        );
        add(
            &db,
            2,
            "a",
            1,
            n,
            n,
            "observed",
            "7",
            "65",
            &n.to_string(),
            "1",
            false,
        );
        add(
            &db,
            3,
            "0-alias",
            1,
            n + 65536,
            n,
            "observed",
            "7",
            "65",
            &n.to_string(),
            "1",
            true,
        );
        add(
            &db,
            4,
            "partial",
            1,
            n,
            n,
            "observed",
            "7",
            "1",
            &n.to_string(),
            "1",
            false,
        );
        add(
            &db, 5, "sparse", 1, 4096, 16777216, "observed", "8", "16", "4096", "1", false,
        );
        prepare(&db, &AtomicBool::new(false), true).unwrap();
        assert_eq!(get(&db, 1)["bytes"], "65536");
        assert_eq!(get(&db, 1)["owner"], "2");
        assert_eq!(get(&db, 2)["bytes"], n.to_string());
        assert_eq!(get(&db, 3)["bytes"], "0");
        assert_eq!(get(&db, 4)["bytes"], n.to_string());
        assert_eq!(get(&db, 4)["reason"], "limited_retained");
        assert_eq!(get(&db, 5)["bytes"], "4096");
        let m = metadata(&db, &AtomicBool::new(false), true).unwrap();
        assert_eq!(m["status"], "limited");
        assert_eq!(m["reason_counts"]["hardlink_nonowner"], "1");
    }
    #[test]
    fn invalid_unavailable_zero_cross_device_and_inconsistent_groups_are_retained() {
        let db = db();
        for (id, status, key, flags, data, device, clone_device, logical) in [
            (1, "observed", "0", "65", "4096", 1, "1", 4096),
            (2, "missing", "NULL", "NULL", "NULL", 1, "NULL", 4096),
            (3, "unsupported", "NULL", "NULL", "NULL", 1, "NULL", 4096),
            (4, "invalid", "NULL", "NULL", "NULL", 1, "NULL", 4096),
            (5, "observed", "8", "65", "4096", 2, "2", 4096),
            (6, "observed", "8", "65", "4096", 1, "1", 4096),
            (7, "observed", "9", "65", "4096", 1, "1", 4096),
            (8, "observed", "9", "65", "2048", 1, "1", 4096),
            (9, "observed", "10", "65", "4096", 1, "1", 4096),
            (10, "observed", "10", "65", "4096", 1, "1", 8192),
            (11, "observed", "8", "65", "4097", 1, "1", 4096),
            (12, "observed", "8", "NULL", "4096", 1, "1", 4096),
            (13, "observed", "8", "65", "NULL", 1, "1", 4096),
            (14, "observed", "8", "65", "4096", 1, "2", 4096),
        ] {
            add(
                &db,
                id,
                &id.to_string(),
                device,
                4096,
                logical,
                status,
                key,
                flags,
                data,
                clone_device,
                false,
            );
        }
        prepare(&db, &AtomicBool::new(false), true).unwrap();
        for id in 1..=14 {
            assert_eq!(get(&db, id)["bytes"], "4096", "id {id}");
        }
        for id in [7, 8, 9, 10] {
            assert_eq!(get(&db, id)["reason"], "inconsistent_group_fallback");
        }
        assert_eq!(get(&db, 5)["reason"], "invalid_metadata_fallback");
        assert_eq!(
            metadata(&db, &AtomicBool::new(false), true).unwrap()["status"],
            "fallback"
        );
    }
    #[test]
    fn contradictory_or_missing_hardlink_alias_disqualifies_inode_before_clones() {
        for (status, key) in [("observed", "99"), ("missing", "NULL")] {
            let db = db();
            add(
                &db, 1, "a", 1, 4096, 4096, "observed", "7", "65", "4096", "1", false,
            );
            add(
                &db, 2, "b", 1, 4096, 4096, "observed", "7", "65", "4096", "1", false,
            );
            add(
                &db, 3, "z-alias", 1, 4096, 4096, status, key, "65", "4096", "1", true,
            );
            prepare(&db, &AtomicBool::new(false), true).unwrap();
            assert_eq!(get(&db, 1)["bytes"], "4096");
            assert_eq!(get(&db, 2)["bytes"], "4096");
            assert_eq!(get(&db, 3)["bytes"], "0");
            assert_eq!(get(&db, 1)["reason"], "hardlink_observation_fallback");
            assert_eq!(
                metadata(&db, &AtomicBool::new(false), true).unwrap()["comparison_ready"],
                false
            );
        }
    }

    #[test]
    fn unknown_filesystem_and_volume_invalidation_never_deduplicate() {
        let db = db();
        for id in [1, 2] {
            add(
                &db,
                id,
                &id.to_string(),
                1,
                4096,
                4096,
                "observed",
                "1",
                "65",
                "4096",
                "1",
                false,
            );
        }
        prepare(&db, &AtomicBool::new(false), false).unwrap();
        assert_eq!(get(&db, 2)["bytes"], "4096");
        assert_eq!(get(&db, 2)["reason"], "filesystem_or_volume_fallback");
        assert_eq!(
            metadata(&db, &AtomicBool::new(false), false).unwrap()["comparison_ready"],
            false
        );
    }
    #[test]
    fn versions_raw_only_and_fallback_pairs_require_rescan_or_primary() {
        let mut m = json!({"status":"complete","context":{"clone_capture":{"requested":true,"version":1}},"apfs_data_clone":{"algorithm_version":VERSION,"capture_version":1,"comparison_ready":true}});
        require(&m, Metric::ApfsDataClone).unwrap();
        pair(&m, &m, Metric::ApfsDataClone).unwrap();
        for field in [
            "algorithm_version",
            "capture_version",
            "columns_available",
            "comparison_ready",
        ] {
            let old = m["apfs_data_clone"][field].clone();
            m["apfs_data_clone"][field] = json!(false);
            assert!(pair(&m, &m, Metric::ApfsDataClone).is_err());
            pair(&m, &m, Metric::Allocated).unwrap();
            m["apfs_data_clone"][field] = old;
        }
        m["status"] = json!("partial");
        assert!(pair(&m, &m, Metric::ApfsDataClone).is_err());
        assert!(require(
            &json!({"context":{"clone_capture":{"requested":true,"version":1}}}),
            Metric::ApfsDataClone
        )
        .is_err());
        assert!(require(&json!({}), Metric::ApfsDataClone).is_err());
    }
}
