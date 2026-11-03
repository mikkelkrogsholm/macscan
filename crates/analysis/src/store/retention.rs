//! Fixed three-run retention. Only recognised immutable metadata is deletable.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::AsRawFd,
};

pub(super) fn key(id: &str) -> io::Result<Vec<u128>> {
    validate_id(id)?;
    let parts: Vec<_> = id
        .split('-')
        .map(str::parse::<u128>)
        .collect::<Result<_, _>>()
        .map_err(|_| invalid("invalid numerical history ID"))?;
    if !(2..=3).contains(&parts.len()) {
        return Err(invalid("unrecognised history ID"));
    }
    Ok(parts)
}
fn regular_json(path: &Path, max: u64) -> io::Result<Value> {
    let m = path.symlink_metadata()?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > max {
        return Err(invalid("unsafe history metadata"));
    }
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}
// Refuse extra files, nested directories and symlinks, even inside owned snapshots.
fn owned_snapshot(path: &Path, id: &str) -> io::Result<()> {
    let m = regular_json(&path.join("manifest.json"), 64 * 1024)?;
    if m["storage_format"] != FORMAT
        || m["scan_id"] != id
        || !matches!(m["status"].as_str(), Some("complete" | "partial"))
    {
        return Err(invalid("unrecognised snapshot"));
    }
    let mut entries = false;
    let mut coverage = false;
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        let kind = item.file_type()?;
        match name.to_str() {
            Some("manifest.json") if kind.is_file() => {}
            Some("coverage.parquet") if kind.is_file() => {
                coverage = true;
            }
            Some("entries") if kind.is_dir() => {
                let mut n = 0;
                for part in fs::read_dir(item.path())? {
                    let part = part?;
                    let name = part.file_name().to_string_lossy().into_owned();
                    let owned = name
                        .strip_prefix("part-")
                        .and_then(|s| s.strip_suffix(".parquet"))
                        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
                    if !part.file_type()?.is_file() || !owned {
                        return Err(invalid("unrecognised snapshot entries"));
                    }
                    n += 1;
                }
                entries = n > 0;
            }
            _ => return Err(invalid("unrecognised snapshot content")),
        }
    }
    if !entries || !coverage {
        return Err(invalid("incomplete snapshot"));
    }
    Ok(())
}
/// Validated immutable history shared by CLI, app and retention.
pub struct HistoryCatalog {
    pub snapshots: BTreeMap<String, Value>,
    pub machines: BTreeMap<String, Value>,
    pub run_ids: Vec<String>,
    pub grouped: BTreeSet<String>,
    references: BTreeMap<String, BTreeSet<String>>,
}
struct Plan {
    kept: Vec<String>,
    removed: Vec<String>,
    snapshots: Vec<String>,
    machines: Vec<String>,
}
impl Store {
    pub(super) fn lock_run(&self) -> io::Result<Option<fs::File>> {
        if self.run_lock.is_some() {
            return Ok(None);
        }
        let f = fs::File::open(&self.root)?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err(io::Error::new(io::ErrorKind::WouldBlock,
                    "another scan or history cleanup is active"));
            }
            return Err(error);
        }
        Ok(Some(f))
    }
    /// Hold across all children and final session publication; nested writes reuse it.
    pub fn begin_run(&self) -> io::Result<Self> {
        let lock = match self.lock_run()? {
            Some(f) => std::sync::Arc::new(f),
            None => self.run_lock.as_ref().unwrap().clone(),
        };
        Ok(Self {
            root: self.root.clone(),
            run_lock: Some(lock),
        })
    }
    pub(super) fn reclaim_stages(&self) -> io::Result<()> {
        // No other publisher holds this store's lock. Reclaim only our own
        // unpublished streaming workspaces after a prior process exit/crash.
        for child in fs::read_dir(&self.root)? {
            let child = child?;
            if !child.file_type()?.is_dir() {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            let Some(previous) = name.strip_suffix(".partial") else {
                continue;
            };
            if validate_id(previous).is_err() {
                continue;
            }
            let marker = child.path().join("stream-stage.json");
            if fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && m.len() < 1024) {
                if let Ok(value) = fs::read(&marker)
                    .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(io::Error::other))
                {
                    if value["owner"] == "macscan-streaming-v1" && value["scan_id"] == previous {
                        fs::remove_dir_all(child.path())?;
                    }
                }
            }
        }
        Ok(())
    }
    /// Default admission: existing metadata plus a checked allowance for this scan.
    /// Explicit-budget Store APIs continue to enforce a combined total budget.
    pub fn measurement_budget(&self, allowance: u64) -> io::Result<u64> {
        if self.run_lock.is_none() {
            return Err(invalid("measurement allowance requires a held run guard"));
        }
        // Reclaim before accounting so removed abandoned workspaces cannot
        // accidentally enlarge the new-measurement allowance.
        self.reclaim_stages()?;
        directory_bytes(&self.root)?
            .checked_add(allowance)
            .ok_or_else(|| invalid("metadata allowance overflow"))
    }
    /// Read-only: no locks, directory creation, stage reclamation or pruning.
    pub fn history_catalog(&self) -> io::Result<HistoryCatalog> {
        self.history_catalog_for(false)
    }
    fn history_catalog_for(&self, pruning: bool) -> io::Result<HistoryCatalog> {
        let mut snapshots = BTreeMap::new();
        let mut machines = BTreeMap::new();
        let mut references = BTreeMap::<String, BTreeSet<String>>::new();
        for item in fs::read_dir(&self.root)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            let kind = item.file_type()?;
            if let Some(id) = name.strip_suffix(".machine.json") {
                key(id)?;
                let s = regular_json(&item.path(), 256 * 1024)?;
                if s["kind"] != "whole_mac_v1"
                    || s["session_id"] != id
                    || !matches!(
                        s["status"].as_str(),
                        Some("complete" | "partial" | "cancelled")
                    )
                {
                    return Err(invalid(
                        "unrecognised machine reference state; history preserved",
                    ));
                }
                let rows = s["volumes"]
                    .as_array()
                    .ok_or_else(|| invalid("missing machine references"))?;
                let mut refs = BTreeSet::new();
                for row in rows {
                    let phase = row["phase"]
                        .as_str()
                        .ok_or_else(|| invalid("missing volume state"))?;
                    // Legacy partial/cancelled sessions may preserve pending or
                    // future phases without snapshots. Reads expose unknown data;
                    // cleanup still requires the original terminal-only contract.
                    if !matches!(
                        phase,
                        "done" | "error" | "cancelled" | "not_scanned" | "unmounted"
                    ) && (pruning || s["status"] == "complete" || !row["scan_id"].is_null()) {
                        return Err(invalid("incomplete machine references; history preserved"));
                    }
                    if let Some(id) = row["scan_id"].as_str() {
                        if phase != "done" {
                            return Err(invalid("inconsistent volume reference state"));
                        }
                        key(id)?;
                        let p = self.root.join(id);
                        if !p.symlink_metadata()?.is_dir()
                            || p.symlink_metadata()?.file_type().is_symlink()
                        {
                            return Err(invalid("unsafe volume reference"));
                        }
                        owned_snapshot(&p, id)?;
                        let manifest = self.manifest(id)?;
                        if (s["status"] == "complete" && manifest["status"] != "complete")
                            || (!row["scan_status"].is_null() && row["scan_status"] != manifest["status"])
                        {
                            return Err(invalid("inconsistent saved volume status; history preserved"));
                        }
                        refs.insert(id.to_owned());
                    } else if phase == "done" || !row["scan_id"].is_null() || s["status"] == "complete" {
                        return Err(invalid("invalid volume reference"));
                    }
                }
                if refs.is_empty() && s["status"] == "complete" {
                    return Err(invalid("empty machine reference state; history preserved"));
                }
                references.insert(id.to_owned(), refs);
                machines.insert(id.to_owned(), s);
            } else if kind.is_dir()
                && key(&name).is_ok()
                && owned_snapshot(&item.path(), &name).is_ok()
            {
                let manifest = self.manifest(&name)?;
                snapshots.insert(name, manifest);
            }
        }
        let grouped: BTreeSet<_> = references.values().flatten().cloned().collect();
        let mut runs: Vec<_> = snapshots.keys()
            .filter(|id| !grouped.contains(*id))
            .cloned()
            .chain(machines.keys().cloned())
            .collect();
        runs.sort_by_cached_key(|id| key(id).unwrap());
        runs.reverse();
        Ok(HistoryCatalog { snapshots, machines, run_ids: runs, grouped, references })
    }
    fn retention_plan(&self, current: Option<&str>) -> io::Result<Plan> {
        let catalog = self.history_catalog_for(true)?;
        let mut runs = catalog.run_ids;
        let machines = catalog.references;
        // Clock corrections/imported future IDs must not evict the result
        // the caller has just published and is about to display.
        if let Some(current) = current {
            let at = runs
                .iter()
                .position(|id| id == current)
                .ok_or_else(|| invalid("published run is not a validated history group"))?;
            let id = runs.remove(at);
            runs.insert(0, id);
        }
        let kept = runs.iter().take(3).cloned().collect::<Vec<_>>();
        let removed = runs.iter().skip(3).cloned().collect::<Vec<_>>();
        let protected: BTreeSet<_> = kept
            .iter()
            .flat_map(|id| {
                machines
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| BTreeSet::from([id.clone()]))
            })
            .collect();
        let discard: BTreeSet<_> = removed
            .iter()
            .flat_map(|id| {
                machines
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| BTreeSet::from([id.clone()]))
            })
            .filter(|id| !protected.contains(id))
            .collect();
        Ok(Plan {
            kept,
            removed: removed.clone(),
            snapshots: discard.into_iter().collect(),
            machines: removed
                .into_iter()
                .filter(|id| machines.contains_key(id))
                .collect(),
        })
    }
    /// Dry-run is the default at the command boundary; both paths share this plan.
    pub fn prune_history(&self, apply: bool) -> io::Result<Value> {
        self.prune_history_for(apply, None)
    }
    fn prune_history_for(&self, apply: bool, current: Option<&str>) -> io::Result<Value> {
        let _lock = self.lock_run()?;
        let plan = self.retention_plan(current)?;
        if apply {
            // Revalidate every target before the first deletion.
            for id in &plan.snapshots {
                owned_snapshot(&self.root.join(id), id)?;
            }
            for id in &plan.machines {
                regular_json(&self.root.join(format!("{id}.machine.json")), 256 * 1024)?;
            }
            for id in &plan.machines {
                fs::remove_file(self.root.join(format!("{id}.machine.json")))?;
            }
            for id in &plan.snapshots {
                fs::remove_dir_all(self.root.join(id))?;
            }
        }
        Ok(
            json!({"command":"history-prune","applied":apply,"keep":3,"kept_runs":plan.kept.len(),"removed_runs":plan.removed.len(),"removed_snapshots":plan.snapshots.len(),"removed_machine_sessions":plan.machines.len(),"removed_snapshot_summary":plan.snapshots.iter().take(8).collect::<Vec<_>>(),"snapshot_summary_truncated":plan.snapshots.len()>8,"kept_summary":plan.kept.iter().take(3).collect::<Vec<_>>(),"removed_summary":plan.removed.iter().take(8).collect::<Vec<_>>(),"summary_truncated":plan.removed.len()>8}),
        )
    }
    /// Return the existing planner receipt after publication; cleanup failure remains a warning.
    pub fn retention_result(&self, current: &str) -> (Option<Value>, Option<String>) {
        match self.prune_history_for(true, Some(current)) {
            Ok(report) => (Some(report), None),
            Err(e) => (None, Some(format!("Målingen er gemt, men historikken kunne ikke ryddes: {e}"))),
        }
    }
    /// A cleanup error must never turn a published measurement into scan failure.
    pub fn retention_warning(&self, current: &str) -> Option<String> {
        self.retention_result(current).1
    }
}
