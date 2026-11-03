//! Saved, bounded examples with full whole-measurement event totals.
use super::*;
use macscan_engine::coverage::{bounded, next_step, REASONS};
impl Store {
    pub fn coverage(
        &self,
        id: &str,
        reason: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        cancelled(cancel)?;
        if limit == 0 || limit > 100 {
            return Err(invalid("coverage limit must be 1..100"));
        }
        if reason.is_some_and(|r| !REASONS.contains(&r)) {
            return Err(invalid("unknown coverage reason"));
        }
        let m = self.manifest(id)?;
        let d = &m["coverage_diagnostics"];
        let versioned = d["version"] == 1
            && d["quality"] == "event_counts"
            && REASONS.iter().all(|r| d["counts"][r].as_u64().is_some());
        let binding = format!(
            "coverage-v1:{id}:{}:{}",
            if versioned { "1" } else { "legacy" },
            reason.unwrap_or("all")
        );
        let offset = match cursor {
            None => 0,
            Some(c) => c
                .strip_prefix(&format!("{binding}:"))
                .ok_or_else(|| invalid("cursor does not match scan/reason/version"))?
                .parse::<usize>()
                .map_err(|_| invalid("invalid coverage cursor"))?,
        };
        let examples: Vec<Value> = if versioned {
            d["samples"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| reason.is_none_or(|r| s["reason"] == r))
                .cloned()
                .collect()
        } else if reason.is_none() {
            m["issues"].as_array().cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        if offset > examples.len() {
            return Err(invalid("coverage cursor out of range"));
        }
        let db = Db::new()?;
        let effective = self.attach(&db, id, "entries")?;
        self.restrict(&db)?;
        let mut records = Vec::new();
        for mut s in examples.iter().skip(offset).take(limit).cloned() {
            cancelled(cancel)?;
            let entry = s["entry_id"].as_str().and_then(|s| s.parse::<u32>().ok());
            s["saved_entry_id"] = entry.map(|n| json!(n.to_string())).unwrap_or(Value::Null);
            s["path_display"] = Value::Null;
            s["path_bytes_hex"] = Value::Null;
            s["path_available"] = json!(false);
            s["path_truncated"] = json!(false);
            let child = s["child_name_bytes_hex"]
                .as_str()
                .and_then(|h| unhex(h).ok());
            let valid_child = child.as_deref().filter(|c| {
                !c.is_empty()
                    && c.len() <= 255
                    && !c.contains(&0)
                    && !c.contains(&b'/')
                    && *c != b"."
                    && *c != b".."
            });
            if !s["child_name_bytes_hex"].is_null() && valid_child.is_none() {
                s["child_identity_invalid"] = json!(true);
                s["child_name_bytes_hex"] = Value::Null;
                s["path_context"] = json!("saved_entry");
            }
            if let Some(entry) = entry {
                // Bound the database result before it crosses the query budget.
                let mut rows=db.query(&format!("SELECT left(hex(path_bytes),1024) AS raw,octet_length(path_bytes) AS raw_length FROM entries_saved WHERE entry_id={entry}"),Some(cancel))?;
                if rows.len() > 0 {
                    let row = rows.row(0)?;
                    if let Some(raw) = row["raw"].as_str().and_then(|h| unhex(h).ok()) {
                        let parent_truncated = row["raw_length"]
                            .as_str()
                            .and_then(|n| n.parse::<usize>().ok())
                            .is_some_and(|n| n > 512);
                        let mut affected = raw;
                        if !parent_truncated {
                            if let Some(child) = valid_child {
                                if !affected.ends_with(b"/") {
                                    affected.push(b'/');
                                }
                                affected.extend(child);
                            }
                        }
                        let path = String::from_utf8_lossy(&affected);
                        let truncated =
                            parent_truncated || affected.len() > 512 || path.len() > 512;
                        s["path_display"] = json!(bounded(&path, 512));
                        s["path_available"] = json!(true);
                        s["path_truncated"] = json!(truncated);
                        s["path_bytes_hex"] = if !truncated {
                            json!(hex(&affected))
                        } else {
                            Value::Null
                        };
                    }
                }
            }
            if s["path_available"] == false {
                s["path_context"] = json!("unavailable");
            } else if s["path_context"].is_null() {
                s["path_context"] = json!(if valid_child.is_some() {
                    "saved_parent_and_raw_child"
                } else {
                    "saved_entry"
                });
            }
            let detail = s["detail"].as_str().unwrap_or("");
            let detail_truncated = s["detail_truncated"] == true || detail.len() > 160;
            s["detail"] = json!(bounded(detail, 160));
            s["detail_truncated"] = json!(detail_truncated);
            s["next_step_da"] = json!(next_step(s["reason"].as_str().unwrap_or("unknown")));
            if !versioned {
                s["reason"] = json!("unknown_legacy");
            }
            records.push(s);
        }
        let next = offset + records.len();
        let retained = if versioned {
            d["samples"].as_array().map_or(0, Vec::len)
        } else {
            m["issues"].as_array().map_or(0, Vec::len)
        };
        let counts: serde_json::Map<String, Value> = REASONS
            .iter()
            .map(|r| {
                (
                    (*r).into(),
                    json!(d["counts"][r].as_u64().unwrap_or(0).to_string()),
                )
            })
            .collect();
        let answer = json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"command":"coverage","scan_id":id,"status":effective["status"],"measurement_status":effective["status"],
            "read_time_global_invalidation":effective["read_time_global_invalidation"]==true,
            "coverage_note_da":if effective["read_time_global_invalidation"]==true{json!("Gemt volumenidentitet ændrede sig; hele målingens dækning er ukendt.")}else{Value::Null},
            "started_at":m["started_at"],"ended_at":m["ended_at"],"scope":"whole_measurement","quality":if versioned{"event_counts"}else{"legacy_samples_only"},"diagnostics_version":if versioned{json!(1)}else{Value::Null},
            "unit":"events_not_unique_areas","error_count":m["error_count"].as_u64().map(|n|n.to_string()),"excluded_count":m["excluded_count"].as_u64().map(|n|n.to_string()),
            "reason_counts":if versioned{json!(counts)}else{Value::Null},"reason_filter":reason,
            "samples_retained":retained,"sample_limit_per_reason":if versioned{json!(2)}else{Value::Null},
            "samples_truncated":m["error_count"].as_u64().unwrap_or(0)>retained as u64,
            "offset":offset,"next_offset":if next<examples.len(){Some(next)}else{None},"records":records,"truncated":next<examples.len(),"next_cursor":if next<examples.len(){Some(format!("{binding}:{next}"))}else{None}});
        compact_response(answer, Some(&binding))
    }
}
