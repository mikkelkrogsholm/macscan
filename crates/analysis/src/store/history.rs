//! Bounded read-only views of the same groups used by metadata retention.
use super::*;

fn row_snapshot(id: &str, m: &Value) -> Value {
    json!({"kind":"snapshot","snapshot_id":id,"scan_id":id,"id_role":"snapshot",
        "root":m["scope"]["root_display"],"status":m["status"],"started_at":m["started_at"],
        "entries":m["entry_count"],"coverage":{"complete":m["status"]=="complete","error_count":m["error_count"],"excluded_count":m["excluded_count"]}})
}
fn member(run: &str, index: usize, volume: &Value, manifest: Option<&Value>) -> Value {
    json!({"kind":"run_member","run_id":run,"member_index":index,"id_role":"snapshot",
        "snapshot_id":volume["scan_id"],"scan_id":volume["scan_id"],"uuid":volume["uuid"],
        "root":volume["root"],"name":volume["name"],"roles":volume["roles"],
        "phase":volume["phase"],"status":manifest.map(|m| &m["status"]),
        "reason":volume.get("error").filter(|v| !v.is_null()).or(volume.get("reason")),
        "entries":manifest.map(|m| &m["entry_count"]),
        "coverage_complete":manifest.map(|m| m["status"]=="complete"),
        "os_used_bytes":volume["os_used_bytes"]})
}
pub(super) fn next_cursor(page: &Value, last: &Value) -> Value {
    match page["view"].as_str() {
        Some("members") => json!(format!(
            "h3:members:{}:{}",
            page["run_id"].as_str().unwrap_or(""),
            last["member_index"]
        )),
        Some("runs") => json!(format!("h3:runs:{}", last["run_id"].as_str().unwrap_or(""))),
        _ => json!(format!(
            "h3:snapshots:{}",
            last["scan_id"].as_str().unwrap_or("")
        )),
    }
}
impl Store {
    pub fn history_page(
        &self,
        limit: usize,
        cursor: Option<&str>,
        run: Option<&str>,
        snapshots: bool,
    ) -> io::Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("limit must be 1..100"));
        }
        if run.is_some() && snapshots {
            return Err(invalid("--run and --view snapshots are mutually exclusive"));
        }
        let catalog = self.history_catalog()?;
        let mut rows = Vec::new();
        let view = if let Some(id) = run {
            if let Some(session) = catalog.machines.get(id) {
                for (index, v) in session["volumes"].as_array().unwrap().iter().enumerate() {
                    rows.push(member(
                        id,
                        index,
                        v,
                        v["scan_id"]
                            .as_str()
                            .and_then(|id| catalog.snapshots.get(id)),
                    ));
                }
            } else if let Some(m) = catalog
                .snapshots
                .get(id)
                .filter(|_| !catalog.grouped.contains(id))
            {
                rows.push(member(
                    id,
                    0,
                    &json!({"phase":"done","scan_id":id,"root":m["scope"]["root_display"]}),
                    Some(m),
                ));
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "run not found; use macscan scans to find run IDs",
                ));
            }
            "members"
        } else if snapshots {
            let mut ids: Vec<_> = catalog.snapshots.keys().collect();
            ids.sort_by_cached_key(|id| super::retention::key(id).unwrap());
            for id in ids.into_iter().rev() {
                let mut row = row_snapshot(id, &catalog.snapshots[id]);
                row["metadata_bytes"] = json!(directory_bytes(&self.location(id)?)?.to_string());
                rows.push(row);
            }
            "snapshots"
        } else {
            for id in &catalog.run_ids {
                let row = if let Some(s) = catalog.machines.get(id) {
                    let volumes = s["volumes"].as_array().unwrap();
                    let saved = volumes.iter().filter(|v| v["scan_id"].is_string()).count();
                    let complete = volumes.iter().all(|v| {
                        v["phase"] == "done"
                            && v["scan_id"]
                                .as_str()
                                .and_then(|id| catalog.snapshots.get(id))
                                .is_some_and(|m| m["status"] == "complete")
                    });
                    json!({"kind":"whole_mac","run_id":id,"id_role":"run","started_at":s["started_at"],"ended_at":s["ended_at"],"status":s["status"],"member_count":volumes.len(),"snapshot_count":saved,
                        "coverage":{"complete":s["status"]=="complete" && complete,"unmeasured_members":volumes.len()-saved},
                        "os_capacity":{"capacity_bytes":s["capacity_bytes"],"used_bytes":s["used_bytes"],"free_bytes":s["free_bytes"]}})
                } else {
                    let m = &catalog.snapshots[id];
                    json!({"kind":"standalone","run_id":id,"id_role":"run","snapshot_id":id,"started_at":m["started_at"],"ended_at":m["ended_at"],"status":m["status"],"member_count":1,"snapshot_count":1,"coverage":{"complete":m["status"]=="complete"}})
                };
                rows.push(row);
            }
            "runs"
        };
        let mut page = json!({"schema_version":"2.0","command":"scans","status":"ok","view":view,"run_id":run,"records":[],"truncated":false,"next_cursor":null,
            "retention":{"keep_runs":3,"read_only":true,"trigger":"saved publication or history-prune --apply"},
            "accounting":"OS capacity is separate from file measurements; no summed unique or reclaimable space"});
        if let Some(id) = run {
            let metadata = catalog
                .machines
                .get(id)
                .or_else(|| catalog.snapshots.get(id))
                .unwrap();
            page["run_status"] = metadata["status"].clone();
            page["run_kind"] = json!(if catalog.machines.contains_key(id) {
                "whole_mac"
            } else {
                "standalone"
            });
        }
        let start = if let Some(cursor) = cursor {
            rows.iter()
                .position(|row| next_cursor(&page, row).as_str() == Some(cursor))
                .map(|i| i + 1)
                .ok_or_else(|| {
                    invalid("unknown cursor or cursor belongs to another history view/run")
                })?
        } else {
            0
        };
        let end = (start + limit).min(rows.len());
        page["records"] = json!(&rows[start..end]);
        page["truncated"] = json!(end < rows.len());
        if end < rows.len() && end > start {
            page["next_cursor"] = next_cursor(&page, &rows[end - 1]);
        }
        compact_response(page, None)
    }
}
